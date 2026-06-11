//! PDF parsing and text extraction module.
//!
//! Provides utilities for extracting text and metadata from PDF documents,
//! specifically designed for Philippine Labor Law documents (DOLE Orders,
//! Labor Code, etc.).
//!
//! The module is organized in three layers:
//!
//! 1. **Extraction** — raw text out of the PDF (`extract_text`,
//!    `extract_pages`, `extract_metadata`, `parse_document`). Page-by-page
//!    extraction uses `lopdf` directly because `pdf-extract` cannot split
//!    pages; full-document extraction prefers `pdf-extract` because its
//!    font/encoding handling is more robust.
//! 2. **Cleanup** — deterministic normalization of the erratic formatting
//!    found in Philippine government PDFs (`clean_text`): mojibake repair,
//!    de-hyphenation across line breaks, control-character stripping.
//! 3. **Structure** — heuristic boundary detection for Philippine legal
//!    drafting style (`split_into_blocks`, `extract_subsections`,
//!    `parse_first_date`), shared by the `parse-dole` and `parse-labor-code`
//!    binaries.

use std::path::Path;
use std::sync::OnceLock;

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use pdf_extract::extract_text as pdf_extract_text;
use rayon::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};

use crate::errors::{IngestionError, IngestionResult};
use crate::types::Subsection;

/// Pages whose cleaned text is shorter than this are flagged as likely
/// scanned images that require OCR. Real text pages of legal documents are
/// essentially never this short (even a signature page carries names,
/// titles, and a date line).
const LOW_TEXT_THRESHOLD: usize = 50;

/// Common UTF-8-decoded-as-Latin-1 mojibake sequences found in government
/// PDFs, in replacement order. The bare "Â" entry must stay last: it is a
/// stray padding byte once the longer sequences have been repaired.
const MOJIBAKE_REPLACEMENTS: &[(&str, &str)] = &[
    ("â€™", "\u{2019}"),
    ("â€˜", "\u{2018}"),
    ("â€œ", "\u{201c}"),
    ("â€\u{9d}", "\u{201d}"),
    ("â€“", "\u{2013}"),
    ("â€”", "\u{2014}"),
    ("â€¦", "\u{2026}"),
    ("Ã±", "ñ"),
    ("Ã‘", "Ñ"),
    ("Ã©", "é"),
    ("Ã¡", "á"),
    // "Ã" + soft hyphen (U+00AD) is the mojibake form of "í"; the soft
    // hyphen must be escaped because it is an invisible character.
    ("Ã\u{ad}", "í"),
    ("Ã³", "ó"),
    ("Ãº", "ú"),
    ("Â§", "§"),
    ("Â", ""),
];

/// Metadata extracted from a PDF document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PdfMetadata {
    /// Document title (if available in PDF metadata).
    pub title: Option<String>,

    /// Document author (if available).
    pub author: Option<String>,

    /// Document subject/description.
    pub subject: Option<String>,

    /// PDF creator application.
    pub creator: Option<String>,

    /// Total number of pages.
    pub page_count: usize,

    /// File size in bytes.
    pub file_size: u64,
}

/// Represents a single page of extracted text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfPage {
    /// Page number (1-indexed).
    pub number: usize,

    /// Extracted text content.
    pub content: String,
}

/// Category of a non-fatal extraction problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningKind {
    /// Page produced no text at all — almost certainly a scanned image.
    EmptyPage,

    /// Page produced suspiciously little text (below [`LOW_TEXT_THRESHOLD`]).
    LowCharCount,

    /// `lopdf` failed on this page; the page is included with empty content.
    PageExtractionFailed,

    /// A whole-document fallback path was taken (see message for details).
    FallbackUsed,
}

/// A non-fatal problem encountered during extraction. Callers should surface
/// these to operators: they usually mean "this page needs OCR".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionWarning {
    /// Page number the warning refers to (0 = whole document).
    pub page: usize,

    /// Warning category.
    pub kind: WarningKind,

    /// Human-readable detail.
    pub message: String,
}

/// Result of page-by-page extraction: the pages plus any warnings raised.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageExtraction {
    /// Per-page extracted text, ordered by page number.
    pub pages: Vec<PdfPage>,

    /// Non-fatal problems encountered (empty/low-text pages, etc.).
    pub warnings: Vec<ExtractionWarning>,
}

/// A parsed PDF document with extracted text and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfDocument {
    /// Source file path.
    pub source_path: String,

    /// Extracted metadata.
    pub metadata: PdfMetadata,

    /// Full extracted text content (cleaned).
    pub full_text: String,

    /// Per-page extracted text (cleaned).
    pub pages: Vec<PdfPage>,

    /// Non-fatal extraction warnings (pages needing OCR, fallbacks taken).
    #[serde(default)]
    pub warnings: Vec<ExtractionWarning>,
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Extract all text from a PDF file using `pdf-extract`.
///
/// # Errors
/// Returns an error if the file doesn't exist, is not a valid PDF,
/// or text extraction fails.
#[instrument(level = "debug", skip_all, fields(path = %path.as_ref().display()))]
pub fn extract_text<P: AsRef<Path>>(path: P) -> IngestionResult<String> {
    let path = path.as_ref();

    if !path.exists() {
        return Err(IngestionError::FileNotFound(path.to_path_buf()));
    }

    debug!("Extracting text from PDF");

    let text = pdf_extract_text(path).map_err(|e| {
        warn!(error = %e, "PDF text extraction failed");
        IngestionError::ExtractionFailed(e.to_string())
    })?;

    debug!(chars = text.len(), "Text extraction complete");
    Ok(text)
}

/// Extract text from a PDF file with a true page-by-page breakdown.
///
/// Uses `lopdf` directly (rather than `pdf-extract`) because only `lopdf`
/// exposes per-page content streams. Pages are extracted in parallel with
/// `rayon`; output order is still ascending page number.
///
/// Pages that fail to extract, or that produce no/low text (scanned image
/// sheets), are included with whatever content was recovered and reported in
/// the returned warning vector instead of aborting the whole document.
///
/// # Errors
/// Returns an error only if the file is missing or is not a loadable PDF.
#[instrument(level = "debug", skip_all, fields(path = %path.as_ref().display()))]
pub fn extract_pages<P: AsRef<Path>>(path: P) -> IngestionResult<PageExtraction> {
    let path = path.as_ref();

    if !path.exists() {
        return Err(IngestionError::FileNotFound(path.to_path_buf()));
    }

    let doc = lopdf::Document::load(path).map_err(|e| {
        warn!(error = %e, "Failed to load PDF");
        IngestionError::InvalidPdf(e.to_string())
    })?;

    let page_numbers: Vec<u32> = doc.get_pages().keys().copied().collect();

    // Parallel per-page extraction. `rayon`'s indexed collect preserves the
    // input order, so pages come back sorted by page number.
    let extracted: Vec<(usize, String, Option<ExtractionWarning>)> = page_numbers
        .par_iter()
        .map(|&number| {
            let page = number as usize;
            match doc.extract_text(&[number]) {
                Ok(raw) => {
                    let content = clean_text(&raw);
                    let warning = page_quality_warning(page, &content);
                    (page, content, warning)
                }
                Err(e) => {
                    let warning = ExtractionWarning {
                        page,
                        kind: WarningKind::PageExtractionFailed,
                        message: format!("lopdf failed to extract page {page}: {e}"),
                    };
                    (page, String::new(), Some(warning))
                }
            }
        })
        .collect();

    let mut pages = Vec::with_capacity(extracted.len());
    let mut warnings = Vec::new();
    for (number, content, warning) in extracted {
        if let Some(w) = warning {
            warn!(page = w.page, kind = ?w.kind, "{}", w.message);
            warnings.push(w);
        }
        pages.push(PdfPage { number, content });
    }

    Ok(PageExtraction { pages, warnings })
}

/// Classify a page's cleaned text as empty / suspiciously short, if it is.
fn page_quality_warning(page: usize, content: &str) -> Option<ExtractionWarning> {
    let chars = content.trim().chars().count();
    if chars == 0 {
        Some(ExtractionWarning {
            page,
            kind: WarningKind::EmptyPage,
            message: format!("Page {page} produced no text; likely a scanned image (OCR required)"),
        })
    } else if chars < LOW_TEXT_THRESHOLD {
        Some(ExtractionWarning {
            page,
            kind: WarningKind::LowCharCount,
            message: format!(
                "Page {page} produced only {chars} characters; may be partially scanned"
            ),
        })
    } else {
        None
    }
}

/// Extract metadata from a PDF file.
#[instrument(level = "debug", skip_all, fields(path = %path.as_ref().display()))]
pub fn extract_metadata<P: AsRef<Path>>(path: P) -> IngestionResult<PdfMetadata> {
    let path = path.as_ref();

    if !path.exists() {
        return Err(IngestionError::FileNotFound(path.to_path_buf()));
    }

    let file_size = std::fs::metadata(path)?.len();

    let doc = lopdf::Document::load(path).map_err(|e| {
        warn!(error = %e, "Failed to load PDF for metadata");
        IngestionError::InvalidPdf(e.to_string())
    })?;

    let page_count = doc.get_pages().len();

    let mut metadata = PdfMetadata {
        page_count,
        file_size,
        ..Default::default()
    };

    if let Ok(info_dict) = doc.trailer.get(b"Info") {
        if let Ok(info_ref) = info_dict.as_reference() {
            if let Ok(info) = doc.get_dictionary(info_ref) {
                metadata.title = extract_string_from_dict(info, b"Title");
                metadata.author = extract_string_from_dict(info, b"Author");
                metadata.subject = extract_string_from_dict(info, b"Subject");
                metadata.creator = extract_string_from_dict(info, b"Creator");
            }
        }
    }

    debug!(?metadata, "Metadata extraction complete");
    Ok(metadata)
}

/// Parse a PDF document, extracting both text and metadata.
///
/// This is the main entry point for PDF parsing. The full text comes from
/// `pdf-extract` (better encoding handling); the per-page breakdown comes
/// from `lopdf`. Each extractor acts as the fallback for the other:
///
/// * If `pdf-extract` fails or returns nothing, the concatenated `lopdf`
///   pages become the full text.
/// * If every `lopdf` page is empty but `pdf-extract` recovered text, a
///   single synthetic page carrying the full text is emitted.
///
/// Both fallbacks are recorded in `warnings` rather than silently applied.
#[instrument(level = "info", skip_all, fields(path = %path.as_ref().display()))]
pub fn parse_document<P: AsRef<Path>>(path: P) -> IngestionResult<PdfDocument> {
    let path = path.as_ref();

    let metadata = extract_metadata(path)?;
    let PageExtraction {
        mut pages,
        mut warnings,
    } = extract_pages(path)?;

    let mut full_text = match extract_text(path) {
        Ok(raw) => clean_text(&raw),
        Err(e) => {
            warnings.push(ExtractionWarning {
                page: 0,
                kind: WarningKind::FallbackUsed,
                message: format!("pdf-extract failed ({e}); using concatenated lopdf pages"),
            });
            String::new()
        }
    };

    if full_text.trim().is_empty() {
        full_text = pages
            .iter()
            .map(|p| p.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
    }

    let pages_empty = pages.iter().all(|p| p.content.trim().is_empty());
    if pages_empty && !full_text.trim().is_empty() {
        warnings.push(ExtractionWarning {
            page: 0,
            kind: WarningKind::FallbackUsed,
            message: "lopdf produced no per-page text; emitting full text as a single page"
                .to_string(),
        });
        pages = vec![PdfPage {
            number: 1,
            content: full_text.clone(),
        }];
    }

    Ok(PdfDocument {
        source_path: path.display().to_string(),
        metadata,
        full_text,
        pages,
        warnings,
    })
}

/// Helper to extract a string value from a lopdf dictionary.
fn extract_string_from_dict(dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
    dict.get(key).ok().and_then(|obj| match obj {
        lopdf::Object::String(bytes, _) => {
            // Try UTF-8 first, then Latin-1 (every byte is a valid Latin-1
            // code point, so the fallback always succeeds).
            String::from_utf8(bytes.clone())
                .ok()
                .or_else(|| Some(bytes.iter().map(|&b| b as char).collect()))
        }
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

fn hyphen_break_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Lowercase letter, hyphen, line break, lowercase letter: a word that
    // was hyphenated across lines by the PDF layout engine. Restricting both
    // sides to lowercase avoids joining legitimate hyphenated compounds that
    // merely happen to sit at a line boundary (e.g. "DO-174-17").
    RE.get_or_init(|| Regex::new(r"([a-zà-öø-ÿ])-\n[ \t]*([a-zà-öø-ÿ])").expect("static regex"))
}

fn trailing_ws_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?m)[ \t]+$").expect("static regex"))
}

fn excess_newlines_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\n{3,}").expect("static regex"))
}

/// Normalize raw extracted text deterministically.
///
/// Repairs, in order:
/// 1. Line endings (`\r\n` / `\r` → `\n`).
/// 2. Common Latin-1/UTF-8 mojibake sequences (`â€™` → `’`, `Ã±` → `ñ`, …).
/// 3. Control characters other than newline/tab (PDF extractors leak NULs
///    and form feeds from content streams).
/// 4. Words hyphenated across line breaks ("termina-\ntion" → "termination").
/// 5. Trailing whitespace and runs of 3+ blank lines.
pub fn clean_text(raw: &str) -> String {
    let mut text = raw.replace("\r\n", "\n").replace('\r', "\n");

    for (bad, good) in MOJIBAKE_REPLACEMENTS {
        if text.contains(bad) {
            text = text.replace(bad, good);
        }
    }

    text = text
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect();

    text = hyphen_break_re().replace_all(&text, "$1$2").into_owned();
    text = trailing_ws_re().replace_all(&text, "").into_owned();
    text = excess_newlines_re().replace_all(&text, "\n\n").into_owned();

    text.trim().to_string()
}

// ---------------------------------------------------------------------------
// Structure detection
// ---------------------------------------------------------------------------

/// The kind of structural unit a heading introduces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerKind {
    /// "RULE I", "Rule 2" — grouping unit in DOLE implementing rules.
    Rule,

    /// "ARTICLE 294", "Art. 5" — Labor Code articles, or grouping units in
    /// some Department Orders.
    Article,

    /// "SECTION 1", "Sec. 3" — the leaf unit of most Department Orders.
    Section,
}

/// A structural unit recovered from running text.
#[derive(Debug, Clone)]
pub struct StructuralBlock {
    /// What kind of heading opened this block.
    pub kind: MarkerKind,

    /// The unit number as printed ("1", "294", "III").
    pub number: String,

    /// Canonical label, e.g. "Section 1" or "Rule III".
    pub label: String,

    /// Inline title if the heading line carried one
    /// ("Section 1. Coverage. — …" → "Coverage").
    pub title: Option<String>,

    /// Body text up to the next structural marker (inline remainder of the
    /// heading line included).
    pub content: String,
}

fn marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Anchored at line start to reject mid-sentence references. The keyword
    // spellings cover Philippine legal drafting conventions: full word,
    // abbreviated with period, upper- and title-case.
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t]*(?P<kw>RULE|Rule|ARTICLE|Article|ART\.|Art\.|SECTION|Section|SEC\.|Sec\.)[ \t]+(?P<num>\d{1,4}[A-Za-z]?(?:-[A-Za-z0-9]+)?|[IVXLCDM]{1,7})(?P<sep>[ \t]*[.:\u{2013}\u{2014}-])?(?P<rest>[^\n]*)",
        )
        .expect("static regex")
    })
}

fn kind_from_keyword(kw: &str) -> MarkerKind {
    match kw.to_ascii_uppercase().as_str() {
        "RULE" => MarkerKind::Rule,
        "ARTICLE" | "ART." => MarkerKind::Article,
        _ => MarkerKind::Section,
    }
}

/// Split running text into structural blocks at Section/Article/Rule
/// boundaries.
///
/// Heuristics (tuned for Philippine legal drafting):
/// * A marker only counts at the start of a line — "as provided in Section 3
///   of RA 11058" mid-paragraph never splits a block.
/// * After the number there must be punctuation (`.`, `:`, dash) or end of
///   line. This rejects line-wrapped cross-references like
///   "Section 5 of the Labor Code provides…" while accepting both
///   "Section 1. Coverage. — text" and a bare "RULE I" heading whose title
///   sits on the next line.
/// * Text before the first marker (preamble / WHEREAS clauses) is not
///   returned here; callers that need it should slice `text` up to the first
///   block themselves.
pub fn split_into_blocks(text: &str) -> Vec<StructuralBlock> {
    let mut markers: Vec<(usize, usize, MarkerKind, String, String)> = Vec::new();

    for caps in marker_re().captures_iter(text) {
        let whole = caps.get(0).expect("match group 0 always present");
        let rest = caps.name("rest").map(|m| m.as_str()).unwrap_or("");

        // Reject cross-references: require a separator after the number, or
        // nothing at all on the rest of the line (bare heading).
        if caps.name("sep").is_none() && !rest.trim().is_empty() {
            continue;
        }

        let kw = &caps["kw"];
        let num = caps["num"].to_string();
        markers.push((
            whole.start(),
            whole.end(),
            kind_from_keyword(kw),
            num,
            rest.trim().to_string(),
        ));
    }

    let mut blocks = Vec::with_capacity(markers.len());
    for (i, (_start, header_end, kind, number, rest)) in markers.iter().enumerate() {
        let body_end = markers.get(i + 1).map_or(text.len(), |next| next.0);
        let trailing = text[*header_end..body_end].trim();

        let (title, inline) = split_heading_line(rest);
        let content = match (inline.is_empty(), trailing.is_empty()) {
            (false, false) => format!("{inline}\n{trailing}"),
            (false, true) => inline,
            (true, _) => trailing.to_string(),
        };

        let keyword = match kind {
            MarkerKind::Rule => "Rule",
            MarkerKind::Article => "Article",
            MarkerKind::Section => "Section",
        };

        blocks.push(StructuralBlock {
            kind: *kind,
            number: number.clone(),
            label: format!("{keyword} {number}"),
            title,
            content,
        });
    }

    blocks
}

/// Split the remainder of a heading line into (title, inline content).
///
/// Philippine drafting style is "Section 1. Coverage. — This Order shall
/// apply…": a short title terminated by a dash before the body starts.
/// Em dash, en dash, and the ASCII " - " that survives some PDF text
/// extractors are all accepted. Without a dash, a short line ending in a
/// period is treated as a title; anything else is body text.
fn split_heading_line(rest: &str) -> (Option<String>, String) {
    let rest = rest.trim().trim_start_matches(['.', ':', '-']).trim();
    if rest.is_empty() {
        return (None, String::new());
    }

    // Dash separators in decreasing specificity; " - " is spaced so that
    // hyphenated compounds ("DO-174-17") never split a heading.
    for dash in ["\u{2014}", "\u{2013}", " - "] {
        if let Some(idx) = rest.find(dash) {
            let title = rest[..idx].trim().trim_end_matches('.').trim();
            let body = rest[idx + dash.len()..].trim();
            let title = (!title.is_empty()).then(|| title.to_string());
            return (title, body.to_string());
        }
    }

    if rest.len() < 100 && rest.ends_with('.') && !rest.trim_end_matches('.').contains(". ") {
        return (Some(rest.trim_end_matches('.').to_string()), String::new());
    }

    (None, rest.to_string())
}

fn subsection_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Matches enumeration markers at line starts: "(a)", "(1)", "(iv)",
    // "a.", "1.", "a)" — the styles used in DOLE Orders and the Labor Code.
    // Labels are capped at 2 chars / 4 roman numerals so that years
    // ("2021.") and article numbers never match.
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^[ \t]*(?:\((?P<paren>[a-z]{1,2}|\d{1,2}|[ivxl]{1,4})\)|(?P<dot>[a-z]|\d{1,2})[.)])[ \t]+",
        )
        .expect("static regex")
    })
}

/// Carve labeled subsections out of a section body.
///
/// Returns `(intro, subsections)` where `intro` is the text before the first
/// enumeration marker. A minimum of **two** markers is required before the
/// text is treated as an enumeration — a single "(a)" is far more likely to
/// be an inline reference than a one-item list, and splitting on it would
/// corrupt the section body.
pub fn extract_subsections(content: &str) -> (String, Vec<Subsection>) {
    let markers: Vec<(usize, usize, String)> = subsection_re()
        .captures_iter(content)
        .map(|caps| {
            let whole = caps.get(0).expect("match group 0 always present");
            let label = caps
                .name("paren")
                .or_else(|| caps.name("dot"))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            (whole.start(), whole.end(), label)
        })
        .collect();

    if markers.len() < 2 {
        return (content.trim().to_string(), Vec::new());
    }

    let intro = content[..markers[0].0].trim().to_string();
    let mut subsections = Vec::with_capacity(markers.len());
    for (i, (_, body_start, label)) in markers.iter().enumerate() {
        let body_end = markers.get(i + 1).map_or(content.len(), |next| next.0);
        let body = content[*body_start..body_end].trim();
        if body.is_empty() {
            continue;
        }
        subsections.push(Subsection {
            label: Some(label.clone()),
            content: body.to_string(),
        });
    }

    (intro, subsections)
}

// ---------------------------------------------------------------------------
// Date parsing
// ---------------------------------------------------------------------------

fn date_mdy_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "March 1, 2021" / "March 1 2021"
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?P<month>January|February|March|April|May|June|July|August|September|October|November|December)\s+(?P<day>\d{1,2}),?\s+(?P<year>\d{4})",
        )
        .expect("static regex")
    })
}

fn date_dmy_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "1st day of March, 2021" / "01 March 2021" — common in signing blocks.
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?P<day>\d{1,2})(?:st|nd|rd|th)?\s+(?:day\s+of\s+)?(?P<month>January|February|March|April|May|June|July|August|September|October|November|December),?\s+(?P<year>\d{4})",
        )
        .expect("static regex")
    })
}

fn month_number(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let lower = name.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|m| *m == lower)
        .map(|i| i as u32 + 1)
}

fn date_from_captures(caps: &regex::Captures<'_>) -> Option<NaiveDate> {
    let month = month_number(caps.name("month")?.as_str())?;
    let day: u32 = caps.name("day")?.as_str().parse().ok()?;
    let year: i32 = caps.name("year")?.as_str().parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Find the first parseable date in `text`, in either "March 1, 2021" or
/// "1st day of March, 2021" style. Returns `None` if no date is present.
pub fn parse_first_date(text: &str) -> Option<NaiveDate> {
    let mdy = date_mdy_re()
        .captures(text)
        .and_then(|c| date_from_captures(&c).map(|d| (c.get(0).map_or(0, |m| m.start()), d)));
    let dmy = date_dmy_re()
        .captures(text)
        .and_then(|c| date_from_captures(&c).map(|d| (c.get(0).map_or(0, |m| m.start()), d)));

    // Whichever style matched earliest in the text wins.
    match (mdy, dmy) {
        (Some((a, da)), Some((b, db))) => Some(if a <= b { da } else { db }),
        (Some((_, d)), None) | (None, Some((_, d))) => Some(d),
        (None, None) => None,
    }
}

/// Convert a calendar date to the midnight-UTC `DateTime` our domain types
/// use.
pub fn date_to_utc(date: NaiveDate) -> DateTime<Utc> {
    Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_extract_text_file_not_found() {
        let path = PathBuf::from("/nonexistent/file.pdf");
        let result = extract_text(&path);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            IngestionError::FileNotFound(_)
        ));
    }

    #[test]
    fn test_extract_metadata_file_not_found() {
        let path = PathBuf::from("/nonexistent/file.pdf");
        let result = extract_metadata(&path);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            IngestionError::FileNotFound(_)
        ));
    }

    #[test]
    fn test_clean_text_dehyphenates_line_breaks() {
        assert_eq!(
            clean_text("termina-\ntion of employment"),
            "termination of employment"
        );
    }

    #[test]
    fn test_clean_text_keeps_compound_identifiers() {
        // Uppercase identifiers split at line ends must keep their hyphen.
        assert_eq!(clean_text("DO-174-\n17"), "DO-174-\n17");
    }

    #[test]
    fn test_clean_text_repairs_mojibake() {
        assert_eq!(
            clean_text("the employerâ€™s duty"),
            "the employer\u{2019}s duty"
        );
        assert_eq!(clean_text("DueÃ±as"), "Dueñas");
    }

    #[test]
    fn test_split_into_blocks_dole_style() {
        let text = "WHEREAS, preamble text here;\n\n\
                    Section 1. Coverage. \u{2014} This Order applies to all employers.\n\
                    More coverage text.\n\n\
                    Section 2. Definition of Terms. \u{2014} As used in this Order:\n";
        let blocks = split_into_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].label, "Section 1");
        assert_eq!(blocks[0].title.as_deref(), Some("Coverage"));
        assert!(blocks[0].content.contains("More coverage text."));
        assert_eq!(blocks[1].title.as_deref(), Some("Definition of Terms"));
    }

    #[test]
    fn test_split_into_blocks_rejects_cross_references() {
        let text = "pursuant to\nSection 3 of RA 11058 which provides safety standards.\n";
        assert!(split_into_blocks(text).is_empty());
    }

    #[test]
    fn test_split_into_blocks_bare_rule_heading() {
        let text = "RULE I\nGENERAL PROVISIONS\n\nSection 1. Title. \u{2014} These Rules.\n";
        let blocks = split_into_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, MarkerKind::Rule);
        assert_eq!(blocks[0].label, "Rule I");
        assert!(blocks[0].content.contains("GENERAL PROVISIONS"));
    }

    #[test]
    fn test_extract_subsections() {
        let content = "As used in this Order:\n\
                       (a) Employer refers to any person who employs workers.\n\
                       (b) Employee refers to any person in the employ of an employer.\n";
        let (intro, subs) = extract_subsections(content);
        assert_eq!(intro, "As used in this Order:");
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].label.as_deref(), Some("a"));
        assert!(subs[1].content.starts_with("Employee refers"));
    }

    #[test]
    fn test_extract_subsections_single_marker_not_split() {
        let content = "The penalty under paragraph\n(a) above shall apply.";
        let (intro, subs) = extract_subsections(content);
        assert!(subs.is_empty());
        assert_eq!(intro, content.trim());
    }

    #[test]
    fn test_parse_first_date_styles() {
        assert_eq!(
            parse_first_date("Signed this 16th day of March, 2021 in Manila."),
            NaiveDate::from_ymd_opt(2021, 3, 16)
        );
        assert_eq!(
            parse_first_date("Done in Manila, Philippines, March 16, 2021."),
            NaiveDate::from_ymd_opt(2021, 3, 16)
        );
        assert_eq!(parse_first_date("no dates here"), None);
    }
}
