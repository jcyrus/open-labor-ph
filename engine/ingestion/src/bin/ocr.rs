//! OCR for scanned source PDFs: writes the text of each manifest entry marked
//! `ocr` to `ocr/<id>.txt` beside the manifest, one form feed between pages.
//!
//! Usage:
//! ```text
//! ocr [--manifest <sources.toml>] [--raw-dir <dir>] [--only <ID>]... [--force]
//! ```
//!
//! The text files are committed and hand-corrected, and OCR output changes
//! between Tesseract versions, so an existing file is never replaced without
//! `--force`. Rebuilding the dataset reads those files and needs no OCR
//! engine. Requires `pdftoppm` (Poppler) and `tesseract` on the PATH.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{bail, Context, Result};
use tracing::info;

use labor_ingestion::manifest::{resolve_manifest, sha256_file, Manifest, SourceDocument};

const USAGE: &str =
    "Usage: ocr [--manifest <sources.toml>] [--raw-dir <dir>] [--only <ID>]... [--force]";

/// Page images are rendered at this resolution, the usual minimum for OCR.
const OCR_DPI: &str = "300";

struct Cli {
    manifest: Option<PathBuf>,
    raw_dir: Option<PathBuf>,
    only: Vec<String>,
    force: bool,
}

/// Parse CLI arguments. `Ok(None)` means help was requested.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Cli>> {
    let mut args = args.into_iter();
    let mut cli = Cli {
        manifest: None,
        raw_dir: None,
        only: Vec::new(),
        force: false,
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
            "--force" => cli.force = true,
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
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let Some(cli) = parse_args(std::env::args().skip(1))? else {
        println!("{USAGE}");
        return Ok(());
    };

    let manifest_path =
        resolve_manifest(cli.manifest.as_deref()).context("data/sources.toml was not found")?;
    let manifest = Manifest::load(&manifest_path)?;
    let raw_dir = cli
        .raw_dir
        .unwrap_or_else(|| manifest_path.parent().unwrap_or(Path::new(".")).join("raw"));

    for id in &cli.only {
        if !manifest.documents.iter().any(|d| d.id == *id && d.ocr) {
            bail!(
                "{id} is not an entry marked ocr in {}",
                manifest_path.display()
            );
        }
    }

    let selected = manifest
        .documents
        .iter()
        .filter(|d| d.ocr && (cli.only.is_empty() || cli.only.contains(&d.id)));
    for doc in selected {
        let output = doc.ocr_text_path(&manifest_path);
        if output.exists() && !cli.force {
            info!(id = %doc.id, "text exists, keeping it");
            continue;
        }
        let pdf = doc.pdf_path(&raw_dir);
        let sha256 = sha256_file(&pdf)?;
        if doc.sha256.as_deref() != Some(sha256.as_str()) {
            bail!("{}: PDF does not match the pinned SHA-256", doc.id);
        }
        info!(id = %doc.id, "running OCR");
        let text = ocr_pdf(doc, &pdf)?;
        if let Some(dir) = output.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&output, text)
            .with_context(|| format!("failed to write {}", output.display()))?;
    }
    Ok(())
}

/// Render a PDF's pages and run Tesseract on each, joining them with form
/// feeds.
fn ocr_pdf(doc: &SourceDocument, pdf: &Path) -> Result<String> {
    let scratch = tempfile::tempdir()?;
    let prefix = scratch.path().join("page");
    let first = doc.ocr_first_page.unwrap_or(1).to_string();
    run_tool(
        Command::new("pdftoppm")
            .args(["-r", OCR_DPI, "-gray", "-png", "-f", &first])
            .arg(pdf)
            .arg(&prefix),
    )?;

    let mut images: Vec<PathBuf> = std::fs::read_dir(scratch.path())?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    images.sort();

    let pages = images
        .iter()
        .map(|image| {
            run_tool(
                Command::new("tesseract")
                    .arg(image)
                    .args(["stdout", "-l", "eng", "--psm", "6"]),
            )
        })
        .collect::<Result<Vec<String>>>()?;
    Ok(pages.join("\u{c}"))
}

/// Run an external tool, returning its standard output.
fn run_tool(command: &mut Command) -> Result<String> {
    let name = command.get_program().to_string_lossy().into_owned();
    let output = command
        .output()
        .with_context(|| format!("failed to run {name}; is it installed?"))?;
    if !output.status.success() {
        bail!(
            "{name} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args_collects_flags() {
        let cli = parse_args(["--only", "DO-174-17", "--force"].map(String::from))
            .unwrap()
            .unwrap();
        assert_eq!(cli.only, ["DO-174-17"]);
        assert!(cli.force);
        assert!(parse_args(["--help".to_string()]).unwrap().is_none());
        assert!(parse_args(["--nope".to_string()]).is_err());
    }
}
