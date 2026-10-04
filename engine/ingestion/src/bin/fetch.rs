//! Source fetcher: downloads and verifies the PDFs listed in the source
//! manifest (`data/sources.toml`).
//!
//! Usage:
//! ```text
//! fetch [--manifest <sources.toml>] [--raw-dir <dir>] [--only <ID>]... [--pin]
//! ```
//!
//! For each entry (or only those named with `--only`), the PDF lives at
//! `<raw-dir>/<id>.pdf`:
//!
//! * **Present locally** → its SHA-256 is checked against the pin.
//! * **Absent, `download_url` set** → downloaded, required to be a real PDF
//!   (a bot-challenge HTML page is rejected), checked against the pin, and
//!   written atomically.
//! * **Absent, no `download_url`** → reported as a manual download: the host
//!   blocks automated access (dole.gov.ph sits behind a Cloudflare
//!   challenge), so the PDF is saved by hand from `source_url` and verified
//!   on the next run.
//!
//! A hash mismatch is always an error and never overwrites a file. Entries
//! without a pin are reported with their hash; `--pin` writes those hashes
//! into the manifest, preserving its comments and layout.
//!
//! The exit code is non-zero when any entry failed. Pending manual downloads
//! do not fail the run.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing::info;

use labor_ingestion::manifest::{
    find_upward, sha256_file, sha256_hex, Manifest, SourceDocument, DEFAULT_MANIFEST_PATH,
};

const USAGE: &str =
    "Usage: fetch [--manifest <sources.toml>] [--raw-dir <dir>] [--only <ID>]... [--pin]";

/// Identifies the tool to the hosts it downloads from.
const USER_AGENT: &str = concat!(
    "open-labor-ph-fetch/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/jcyrus/open-labor-ph)"
);

/// Government PDFs can be large and the hosts slow.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

struct Cli {
    manifest: Option<PathBuf>,
    raw_dir: Option<PathBuf>,
    only: Vec<String>,
    pin: bool,
}

/// Parse CLI arguments. `Ok(None)` means help was requested.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Cli>> {
    let mut args = args.into_iter();
    let mut cli = Cli {
        manifest: None,
        raw_dir: None,
        only: Vec::new(),
        pin: false,
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                cli.manifest = Some(PathBuf::from(
                    args.next().context("--manifest requires a value")?,
                ));
            }
            "--raw-dir" => {
                cli.raw_dir = Some(PathBuf::from(
                    args.next().context("--raw-dir requires a value")?,
                ));
            }
            "--only" => cli.only.push(args.next().context("--only requires an id")?),
            "--pin" => cli.pin = true,
            "-h" | "--help" => return Ok(None),
            other => bail!("unexpected argument {other:?}\n{USAGE}"),
        }
    }
    Ok(Some(cli))
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// What happened to one manifest entry.
enum Status {
    /// Local file matches the pin.
    Verified,
    /// Downloaded and matches the pin.
    Downloaded,
    /// File present (downloaded now or earlier) but the entry has no pin.
    Unpinned(String),
    /// No local file and no `download_url`: save it by hand.
    Manual(PathBuf),
}

/// Returns `Ok(true)` when every selected entry succeeded.
fn run() -> Result<bool> {
    let Some(cli) = parse_args(std::env::args().skip(1))? else {
        println!("{USAGE}");
        return Ok(true);
    };

    let manifest_path = match cli.manifest {
        Some(path) => path,
        None => find_upward(Path::new(DEFAULT_MANIFEST_PATH)).with_context(|| {
            format!(
                "{DEFAULT_MANIFEST_PATH} not found above the current directory; pass --manifest"
            )
        })?,
    };
    let manifest = Manifest::load(&manifest_path)?;

    // Default raw dir sits next to the manifest: data/sources.toml → data/raw.
    let raw_dir = match cli.raw_dir {
        Some(dir) => dir,
        None => manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("raw"),
    };

    for id in &cli.only {
        if manifest.find(id).is_none() {
            bail!("--only {id}: no such entry in {}", manifest_path.display());
        }
    }
    let selected: Vec<&SourceDocument> = manifest
        .documents
        .iter()
        .filter(|d| cli.only.is_empty() || cli.only.contains(&d.id))
        .collect();

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;

    let mut to_pin: Vec<(String, String)> = Vec::new();
    let mut failures = 0usize;
    for doc in selected {
        match process(doc, &raw_dir, &client) {
            Ok(Status::Verified) => println!("verified    {}", doc.id),
            Ok(Status::Downloaded) => println!("downloaded  {}", doc.id),
            Ok(Status::Unpinned(hash)) => {
                println!("unpinned    {}  sha256={hash}", doc.id);
                to_pin.push((doc.id.clone(), hash));
            }
            Ok(Status::Manual(path)) => println!(
                "manual      {}  download from {} and save as {}",
                doc.id,
                doc.source_url,
                path.display()
            ),
            Err(e) => {
                failures += 1;
                println!("FAILED      {}  {e:#}", doc.id);
            }
        }
    }

    if !to_pin.is_empty() {
        if cli.pin {
            let raw = std::fs::read_to_string(&manifest_path)?;
            let pinned = pin_hashes(&raw, &to_pin)?;
            // Never write a manifest the loader would reject.
            Manifest::parse(&pinned)
                .map_err(|e| anyhow::anyhow!("pinning produced an invalid manifest: {e}"))?;
            std::fs::write(&manifest_path, pinned)
                .with_context(|| format!("failed to write {}", manifest_path.display()))?;
            info!(count = to_pin.len(), manifest = %manifest_path.display(), "pinned hashes");
        } else {
            println!(
                "{} entr{} unpinned; review the files, then rerun with --pin to record their hashes",
                to_pin.len(),
                if to_pin.len() == 1 { "y is" } else { "ies are" }
            );
        }
    }

    Ok(failures == 0)
}

/// Bring one entry's local PDF up to date and verify it.
fn process(
    doc: &SourceDocument,
    raw_dir: &Path,
    client: &reqwest::blocking::Client,
) -> Result<Status> {
    let path = doc.pdf_path(raw_dir);

    if path.exists() {
        let hash = sha256_file(&path)?;
        return match &doc.sha256 {
            Some(pin) if *pin == hash => Ok(Status::Verified),
            Some(pin) => bail!(
                "{} has sha256 {hash}, but the manifest pins {pin}; the file is not the \
                 pinned document (delete it to re-download, or fix the pin if the source \
                 was officially replaced)",
                path.display()
            ),
            None => Ok(Status::Unpinned(hash)),
        };
    }

    let Some(url) = doc.download_url.as_deref() else {
        return Ok(Status::Manual(path));
    };

    info!(id = %doc.id, %url, "downloading");
    let bytes = download(client, url, &path)?;
    let hash = sha256_hex(&bytes);
    if let Some(pin) = &doc.sha256 {
        if *pin != hash {
            bail!(
                "{url} served sha256 {hash}, but the manifest pins {pin}; nothing was written \
                 (the host may have replaced the document)"
            );
        }
    }

    std::fs::create_dir_all(raw_dir)
        .with_context(|| format!("failed to create {}", raw_dir.display()))?;
    // Write to a sibling temp file and rename, so an interrupted run never
    // leaves a truncated PDF that would later fail verification confusingly.
    let partial = partial_path(&path);
    std::fs::write(&partial, &bytes)
        .with_context(|| format!("failed to write {}", partial.display()))?;
    std::fs::rename(&partial, &path)
        .with_context(|| format!("failed to move {} into place", partial.display()))?;

    Ok(match &doc.sha256 {
        Some(_) => Status::Downloaded,
        None => Status::Unpinned(hash),
    })
}

/// GET `url` and require a PDF body.
fn download(client: &reqwest::blocking::Client, url: &str, save_as: &Path) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("request to {url} failed"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();
    let bytes = response
        .bytes()
        .with_context(|| format!("failed to read the body of {url}"))?;

    if !status.is_success() || !is_pdf(&bytes) {
        bail!(
            "{url} did not return a PDF (HTTP {status}, {content_type}); the host likely blocks \
             automated downloads. Download it in a browser, save it as {}, and rerun fetch",
            save_as.display()
        );
    }
    Ok(bytes.to_vec())
}

/// PDF files start with the `%PDF-` magic bytes.
fn is_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Set `sha256` on the named entries, leaving every other byte of the
/// manifest — comments, ordering, formatting — untouched.
fn pin_hashes(raw: &str, pins: &[(String, String)]) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = raw.parse().context("manifest is not valid TOML")?;
    let tables = doc
        .get_mut("document")
        .and_then(|item| item.as_array_of_tables_mut())
        .context("manifest has no [[document]] entries")?;

    for table in tables.iter_mut() {
        let Some(id) = table.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        if let Some((_, hash)) = pins.iter().find(|(pin_id, _)| *pin_id == id) {
            table["sha256"] = toml_edit::value(hash.as_str());
        }
    }
    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_hashes_preserves_comments_and_other_entries() {
        let raw = "# Source manifest\n\n\
                   [[document]]\n\
                   id = \"DO-174-17\" # contracting\n\
                   kind = \"dole_order\"\n\n\
                   [[document]]\n\
                   id = \"labor-code-renumbered\"\n\
                   kind = \"labor_code\"\n";
        let hash = "b".repeat(64);
        let pinned =
            pin_hashes(raw, &[("labor-code-renumbered".to_string(), hash.clone())]).unwrap();
        assert!(pinned.starts_with("# Source manifest\n"));
        assert!(pinned.contains("id = \"DO-174-17\" # contracting"));
        assert!(pinned.contains(&format!("sha256 = \"{hash}\"")));
        assert_eq!(pinned.matches("sha256").count(), 1);
    }

    #[test]
    fn test_is_pdf_rejects_challenge_pages() {
        assert!(is_pdf(b"%PDF-1.7\n..."));
        assert!(!is_pdf(b"<!DOCTYPE html><title>Just a moment...</title>"));
    }

    #[test]
    fn test_partial_path_keeps_dotted_ids() {
        assert_eq!(
            partial_path(Path::new("data/raw/DO-174-17.pdf")),
            PathBuf::from("data/raw/DO-174-17.pdf.part")
        );
    }

    #[test]
    fn test_args() {
        let cli = parse_args(
            ["--only", "DO-174-17", "--pin", "--raw-dir", "/tmp/raw"].map(str::to_string),
        )
        .unwrap()
        .unwrap();
        assert_eq!(cli.only, ["DO-174-17"]);
        assert!(cli.pin);
        assert_eq!(cli.raw_dir, Some(PathBuf::from("/tmp/raw")));
        assert!(parse_args(["--help".to_string()]).unwrap().is_none());
        assert!(parse_args(["--bogus".to_string()]).is_err());
    }
}
