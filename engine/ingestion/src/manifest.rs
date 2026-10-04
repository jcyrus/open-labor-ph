//! Source manifest (`data/sources.toml`).
//!
//! The manifest lists every document the dataset is built from: where it
//! officially lives, where it can be downloaded, and the SHA-256 of the exact
//! PDF that was parsed. Pinning the hash makes ingestion reproducible —
//! `fetch` refuses a download that does not match the pin, and the parsers
//! use the hash to find a PDF's manifest entry (source URL, publication date,
//! expected identifier) without any extra flags.
//!
//! ```toml
//! [[document]]
//! id = "DO-174-17"
//! kind = "dole_order"
//! title = "Rules Implementing Articles 106 to 109 of the Labor Code"
//! source_url = "https://bwc.dole.gov.ph/issuances/department-orders/"
//! download_url = "https://…/DO-174-17.pdf"   # optional
//! published_date = "2017-03-20"              # optional, YYYY-MM-DD
//! sha256 = "…"                               # written by `fetch --pin`
//! ```

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::NaiveDate;
use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::errors::{IngestionError, IngestionResult};

/// Manifest location relative to the repository root.
pub const DEFAULT_MANIFEST_PATH: &str = "data/sources.toml";

/// What kind of document an entry is, which selects its parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    /// A DOLE Department Order, parsed by `parse-dole`.
    DoleOrder,
    /// The Labor Code, parsed by `parse-labor-code`.
    LaborCode,
}

/// One source document.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDocument {
    /// Dataset identifier; also the local file name (`<raw_dir>/<id>.pdf`).
    /// For Department Orders it must be the normalized order number.
    pub id: String,

    /// Document kind.
    pub kind: DocumentKind,

    /// Human-readable title.
    pub title: String,

    /// Official page for the document, recorded as `source_url` in the
    /// dataset.
    pub source_url: String,

    /// Direct PDF URL `fetch` can download. Absent when the host blocks
    /// automated downloads; the PDF is then saved by hand.
    #[serde(default)]
    pub download_url: Option<String>,

    /// Official publication date (`YYYY-MM-DD`), used to resolve "N days
    /// after publication" effectivity clauses.
    #[serde(default)]
    pub published_date: Option<String>,

    /// Lowercase hex SHA-256 of the PDF, once pinned.
    #[serde(default)]
    pub sha256: Option<String>,
}

impl SourceDocument {
    /// Where the PDF for this entry lives locally.
    pub fn pdf_path(&self, raw_dir: &Path) -> PathBuf {
        raw_dir.join(format!("{}.pdf", self.id))
    }

    /// The publication date, if set. Validated by [`Manifest::parse`].
    pub fn published_date(&self) -> Option<NaiveDate> {
        self.published_date
            .as_deref()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    }
}

/// The parsed manifest.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// All entries, in file order.
    #[serde(default, rename = "document")]
    pub documents: Vec<SourceDocument>,
}

fn id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]*$").expect("static regex"))
}

fn dole_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Same shape as `order_number` in dole_order_schema.json.
    RE.get_or_init(|| Regex::new(r"^DO-[0-9]+(-[A-Z])?-[0-9]{2}$").expect("static regex"))
}

fn sha256_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[0-9a-f]{64}$").expect("static regex"))
}

fn is_http_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

impl Manifest {
    /// Read and validate a manifest file.
    pub fn load(path: &Path) -> IngestionResult<Manifest> {
        let raw = std::fs::read_to_string(path)?;
        Manifest::parse(&raw)
            .map_err(|e| IngestionError::Manifest(format!("{}: {e}", path.display())))
    }

    /// Parse and validate manifest TOML. Every problem is reported at once
    /// so a contributor can fix the file in one pass.
    pub fn parse(raw: &str) -> Result<Manifest, String> {
        let manifest: Manifest = toml::from_str(raw).map_err(|e| e.to_string())?;

        let mut problems: Vec<String> = Vec::new();
        let mut ids: HashSet<&str> = HashSet::new();
        let mut hashes: HashSet<&str> = HashSet::new();
        for doc in &manifest.documents {
            let id = doc.id.as_str();
            if !ids.insert(id) {
                problems.push(format!("duplicate id {id:?}"));
            }
            if !id_re().is_match(id) {
                problems.push(format!(
                    "{id:?}: id may only contain letters, digits, '.', '_' and '-'"
                ));
            }
            if doc.kind == DocumentKind::DoleOrder && !dole_id_re().is_match(id) {
                problems.push(format!(
                    "{id:?}: dole_order ids must be normalized order numbers (DO-174-17, DO-18-A-11)"
                ));
            }
            if !is_http_url(&doc.source_url) {
                problems.push(format!("{id}: source_url must be an http(s) URL"));
            }
            if doc.download_url.as_deref().is_some_and(|u| !is_http_url(u)) {
                problems.push(format!("{id}: download_url must be an http(s) URL"));
            }
            if let Some(date) = doc.published_date.as_deref() {
                if NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
                    problems.push(format!("{id}: published_date {date:?} is not YYYY-MM-DD"));
                }
            }
            if let Some(hash) = doc.sha256.as_deref() {
                if !sha256_re().is_match(hash) {
                    problems.push(format!("{id}: sha256 must be 64 lowercase hex characters"));
                } else if !hashes.insert(hash) {
                    problems.push(format!("{id}: sha256 duplicates another entry's"));
                }
            }
        }

        if problems.is_empty() {
            Ok(manifest)
        } else {
            Err(problems.join("; "))
        }
    }

    /// The entry with this identifier.
    pub fn find(&self, id: &str) -> Option<&SourceDocument> {
        self.documents.iter().find(|d| d.id == id)
    }

    /// The entry whose pinned hash equals `sha256`.
    pub fn find_by_sha256(&self, sha256: &str) -> Option<&SourceDocument> {
        self.documents
            .iter()
            .find(|d| d.sha256.as_deref() == Some(sha256))
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Lowercase hex SHA-256 of a file's contents.
pub fn sha256_file(path: &Path) -> IngestionResult<String> {
    if !path.exists() {
        return Err(IngestionError::FileNotFound(path.to_path_buf()));
    }
    Ok(sha256_hex(&std::fs::read(path)?))
}

/// Find `relative` (e.g. `data/sources.toml`) in the current directory or
/// the nearest ancestor, so the binaries work from the workspace root or any
/// crate subdirectory.
pub fn find_upward(relative: &Path) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors()
        .map(|dir| dir.join(relative))
        .find(|candidate| candidate.exists())
}

/// Find the manifest entry for a PDF by its hash.
///
/// With an explicit `manifest` path the file must exist. Otherwise
/// [`DEFAULT_MANIFEST_PATH`] is searched upward from the current directory
/// and simply skipped when absent, so the parsers still work on PDFs outside
/// any checkout.
pub fn lookup_entry(
    manifest: Option<&Path>,
    sha256: &str,
) -> IngestionResult<Option<SourceDocument>> {
    let path = match manifest {
        Some(path) => path.to_path_buf(),
        None => match find_upward(Path::new(DEFAULT_MANIFEST_PATH)) {
            Some(path) => path,
            None => return Ok(None),
        },
    };
    Ok(Manifest::load(&path)?.find_by_sha256(sha256).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
        [[document]]
        id = "DO-174-17"
        kind = "dole_order"
        title = "Contracting"
        source_url = "https://bwc.dole.gov.ph/issuances/department-orders/"
        published_date = "2017-03-20"
        sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

        [[document]]
        id = "labor-code-renumbered"
        kind = "labor_code"
        title = "Labor Code"
        source_url = "https://natlex.ilo.org/"
        download_url = "https://natlex.ilo.org/file.pdf"
    "#;

    #[test]
    fn test_parse_valid_manifest() {
        let manifest = Manifest::parse(VALID).unwrap();
        assert_eq!(manifest.documents.len(), 2);
        let order = manifest.find("DO-174-17").unwrap();
        assert_eq!(order.published_date(), NaiveDate::from_ymd_opt(2017, 3, 20));
        assert_eq!(
            order.pdf_path(Path::new("data/raw")),
            PathBuf::from("data/raw/DO-174-17.pdf")
        );
        assert_eq!(
            manifest
                .find_by_sha256(&"a".repeat(64))
                .map(|d| d.id.as_str()),
            Some("DO-174-17")
        );
    }

    #[test]
    fn test_parse_reports_every_problem() {
        let raw = r#"
            [[document]]
            id = "DO 174"
            kind = "dole_order"
            title = "Bad"
            source_url = "file:///tmp/x.pdf"
            published_date = "March 2017"
            sha256 = "ABC"
        "#;
        let err = Manifest::parse(raw).unwrap_err();
        for expected in [
            "may only contain",
            "normalized order numbers",
            "source_url must be",
            "not YYYY-MM-DD",
            "64 lowercase hex",
        ] {
            assert!(err.contains(expected), "missing {expected:?} in {err}");
        }
    }

    #[test]
    fn test_parse_rejects_duplicates_and_unknown_fields() {
        let dup = format!("{VALID}\n[[document]]\nid = \"DO-174-17\"\nkind = \"dole_order\"\ntitle = \"x\"\nsource_url = \"https://x.ph\"\n");
        assert!(Manifest::parse(&dup).unwrap_err().contains("duplicate id"));

        let unknown = "[[document]]\nid = \"x\"\nkind = \"labor_code\"\ntitle = \"x\"\nsource_url = \"https://x.ph\"\nurl = \"typo\"\n";
        assert!(Manifest::parse(unknown).is_err());
    }

    #[test]
    fn test_sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
