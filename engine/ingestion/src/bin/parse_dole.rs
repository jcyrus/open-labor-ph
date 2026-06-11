//! DOLE Department Order PDF parser.
//!
//! Reads a Department Order PDF, applies the structural heuristics from
//! `labor_ingestion::parser`, and writes a schema-compliant `DoleOrder`
//! JSON file.
//!
//! Usage:
//! ```text
//! parse-dole <input.pdf> <output.json> [--source-url <url>]
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use regex::Regex;
use tracing::{info, warn};

use labor_ingestion::parser::{
    self, date_to_utc, extract_subsections, parse_first_date, split_into_blocks, MarkerKind,
    PdfDocument,
};
use labor_ingestion::types::{DoleOrder, OrderMetadata, Section};

/// Keyword → tag mapping for coarse topical tagging. Keys are matched
/// case-insensitively against the full document text.
const TAG_KEYWORDS: &[(&str, &str)] = &[
    ("telecommuting", "telecommuting"),
    ("contracting", "contracting"),
    ("subcontracting", "contracting"),
    ("occupational safety", "OSH"),
    ("safety and health", "OSH"),
    ("wage", "wages"),
    ("termination", "termination"),
    ("security of tenure", "termination"),
    ("probationary", "probationary"),
    ("apprentice", "apprenticeship"),
    ("maternity", "leave"),
    ("paternity", "leave"),
    ("collective bargaining", "labor_relations"),
    ("kasambahay", "kasambahay"),
    ("domestic worker", "kasambahay"),
    ("overseas filipino", "ofw"),
    ("migrant worker", "ofw"),
];

struct Cli {
    input: PathBuf,
    output: PathBuf,
    source_url: Option<String>,
}

fn parse_args() -> Result<Cli> {
    let mut args = std::env::args().skip(1);
    let mut positional: Vec<String> = Vec::new();
    let mut source_url = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source-url" => {
                source_url = Some(args.next().context("--source-url requires a value")?);
            }
            "-h" | "--help" => {
                bail!("Usage: parse-dole <input.pdf> <output.json> [--source-url <url>]");
            }
            _ => positional.push(arg),
        }
    }

    if positional.len() != 2 {
        bail!("Usage: parse-dole <input.pdf> <output.json> [--source-url <url>]");
    }

    Ok(Cli {
        input: PathBuf::from(&positional[0]),
        output: PathBuf::from(&positional[1]),
        source_url,
    })
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                // lopdf logs an info event per font lookup; keep it quiet.
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,lopdf=warn")),
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
    let cli = parse_args()?;

    let doc = parser::parse_document(&cli.input)
        .with_context(|| format!("failed to parse PDF {}", cli.input.display()))?;

    for w in &doc.warnings {
        warn!(page = w.page, "{}", w.message);
    }

    if doc.full_text.trim().is_empty() {
        bail!(
            "no text could be extracted from {} — the document appears to be a \
             scanned image and requires OCR before ingestion",
            cli.input.display()
        );
    }

    let order = build_dole_order(&doc, &cli)?;

    let json = serde_json::to_string_pretty(&order).context("failed to serialize DoleOrder")?;
    std::fs::write(&cli.output, json + "\n")
        .with_context(|| format!("failed to write {}", cli.output.display()))?;

    info!(
        order = %order.order_number,
        sections = order.sections.len(),
        output = %cli.output.display(),
        "DOLE Order parsed successfully"
    );
    Ok(())
}

/// Assemble a `DoleOrder` from extracted text using deterministic heuristics.
fn build_dole_order(doc: &PdfDocument, cli: &Cli) -> Result<DoleOrder> {
    let text = &doc.full_text;

    let order_number = extract_order_number(text, &cli.input).context(
        "could not find a Department Order number (e.g. \"Department Order No. 174-17\") \
         in the document text or filename",
    )?;

    let title = extract_title(text)
        .or_else(|| doc.metadata.title.clone())
        .unwrap_or_else(|| format!("Department Order {order_number}"));

    let effective_date = extract_effective_date(text, &order_number)?;

    // Default to a file:// URL of the source PDF when no official URL is
    // given — honest provenance beats a fabricated dole.gov.ph link.
    let source_url = cli.source_url.clone().unwrap_or_else(|| {
        let canonical = cli
            .input
            .canonicalize()
            .unwrap_or_else(|_| cli.input.clone());
        format!("file://{}", canonical.display())
    });

    let metadata = OrderMetadata {
        issuing_authority: extract_issuing_authority(text)
            .unwrap_or_else(|| "Secretary of Labor and Employment".to_string()),
        supersedes: extract_supersedes(text, &order_number),
        related_laws: extract_related_laws(text),
        tags: extract_tags(text),
        published_date: None,
    };

    let mut sections = build_sections(text);
    if sections.is_empty() {
        // No structural markers at all (unusual layout): keep the document
        // ingestible by emitting the full text as one section.
        warn!("no structural markers found; emitting the full text as a single section");
        sections.push(Section {
            section_number: "Full Text".to_string(),
            title: None,
            content: text.clone(),
            subsections: Vec::new(),
        });
    }

    Ok(DoleOrder {
        order_number,
        title,
        effective_date: date_to_utc(effective_date),
        source_url,
        metadata,
        sections,
    })
}

fn order_number_dash_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Department Order No. 174-17" / "D.O. No. 174-17"
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:department\s+order|d\.?\s*o\.?)\s*no\.?\s*(\d{1,4})\s*-\s*(\d{2})\b")
            .expect("static regex")
    })
}

fn order_number_series_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Department Order No. 219, Series of 2021" / "… s. 2021"
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:department\s+order|d\.?\s*o\.?)\s*no\.?\s*(\d{1,4})\s*,?\s*\(?(?:series\s+of|s\.)\s*(\d{4})\)?",
        )
        .expect("static regex")
    })
}

fn filename_order_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bDO[-_ ]?(\d{1,4})[-_](\d{2,4})\b").expect("static regex"))
}

/// Extract the order number and normalize it to the schema's `DO-NNN-YY`
/// shape.
///
/// Both notation styles ("No. 174-17" and "No. 219, Series of 2021") are
/// searched and **the earliest match in the text wins**: a Department
/// Order's own number sits in the page-one header, before any "amending
/// Department Order No. …" reference to another order. Falls back to the
/// input filename when the body yields nothing.
fn extract_order_number(text: &str, input: &Path) -> Option<String> {
    let dash = order_number_dash_re().captures(text).map(|caps| {
        let start = caps.get(0).map_or(usize::MAX, |m| m.start());
        (start, format!("DO-{}-{}", &caps[1], &caps[2]))
    });
    let series = order_number_series_re().captures(text).map(|caps| {
        let start = caps.get(0).map_or(usize::MAX, |m| m.start());
        let year = &caps[2];
        (
            start,
            format!("DO-{}-{}", &caps[1], &year[year.len() - 2..]),
        )
    });

    let from_text = match (dash, series) {
        (Some((a, da)), Some((b, db))) => Some(if a <= b { da } else { db }),
        (Some((_, d)), None) | (None, Some((_, d))) => Some(d),
        (None, None) => None,
    };
    if from_text.is_some() {
        return from_text;
    }

    let filename = input.file_stem()?.to_string_lossy();
    let caps = filename_order_re().captures(&filename)?;
    let year = caps[2].to_string();
    let yy = if year.len() == 4 {
        year[2..].to_string()
    } else {
        year
    };
    Some(format!("DO-{}-{yy}", &caps[1]))
}

/// Extract the title: the run of mostly-uppercase lines immediately after
/// the "DEPARTMENT ORDER NO. …" line. Philippine DOs print the full title
/// in capitals directly under the order number on the cover page.
fn extract_title(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().take(60).collect();
    let header_idx = lines
        .iter()
        .position(|l| order_number_dash_re().is_match(l) || order_number_series_re().is_match(l))?;

    let mut title_lines: Vec<&str> = Vec::new();
    for line in lines.iter().skip(header_idx + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // Allow blank gaps before the title starts, then stop at the
            // first blank line after it.
            if title_lines.is_empty() {
                continue;
            }
            break;
        }
        // A "Series of 2021" sub-line belongs to the header, not the title.
        if trimmed.to_ascii_lowercase().starts_with("series of") {
            continue;
        }
        if !is_mostly_uppercase(trimmed) || title_lines.len() >= 8 {
            break;
        }
        title_lines.push(trimmed);
    }

    if title_lines.is_empty() {
        return None;
    }
    Some(title_lines.join(" "))
}

/// True if at least 70% of the alphabetic characters are uppercase.
fn is_mostly_uppercase(line: &str) -> bool {
    let alphabetic: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    if alphabetic.is_empty() {
        return false;
    }
    let upper = alphabetic.iter().filter(|c| c.is_uppercase()).count();
    upper * 10 >= alphabetic.len() * 7
}

fn signing_context_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b(?:signed|done|issued|approved)\b").expect("static regex"))
}

/// Determine the effective date. DOs state effectivity relative to
/// publication ("fifteen days after publication"), which cannot be computed
/// from the document alone, so the deterministic proxy is, in order of
/// preference:
///
/// 1. The date near a signing keyword ("Done/Signed this 16th day of …").
/// 2. The first date appearing in the final 2 000 characters (the signature
///    page).
/// 3. January 1 of the series year from the order number, with a warning.
fn extract_effective_date(text: &str, order_number: &str) -> Result<NaiveDate> {
    for m in signing_context_re().find_iter(text) {
        let window_end = (m.end() + 120).min(text.len());
        let window = char_boundary_slice(text, m.end(), window_end);
        if let Some(date) = parse_first_date(window) {
            return Ok(date);
        }
    }

    let tail_start = text.len().saturating_sub(2000);
    let tail = char_boundary_slice(text, tail_start, text.len());
    if let Some(date) = parse_first_date(tail) {
        return Ok(date);
    }

    // Series year is the trailing "-YY" of the normalized order number.
    let yy: i32 = order_number
        .rsplit('-')
        .next()
        .and_then(|s| s.parse().ok())
        .context("order number missing series year")?;
    let year = 2000 + yy;
    warn!(
        year,
        "no signing date found; falling back to January 1 of the series year"
    );
    NaiveDate::from_ymd_opt(year, 1, 1).context("invalid fallback year")
}

/// Slice `text` between byte offsets, snapping outward to char boundaries so
/// multi-byte characters (ñ, em dashes) never cause a panic.
fn char_boundary_slice(text: &str, mut start: usize, mut end: usize) -> &str {
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    &text[start..end]
}

fn secretary_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*(?:officer[- ]in[- ]charge|secretary)\b").expect("static regex")
    })
}

/// Extract the signing official: the name line immediately above a
/// "Secretary of Labor and Employment" line in the signature block.
fn extract_issuing_authority(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !secretary_line_re().is_match(line) {
            continue;
        }
        // Walk back to the nearest non-empty line; signature names are
        // printed in capitals ("SILVESTRE H. BELLO III").
        let name = lines[..i]
            .iter()
            .rev()
            .map(|l| l.trim())
            .find(|l| !l.is_empty())?;
        if is_mostly_uppercase(name) && name.split_whitespace().count() >= 2 {
            return Some(format!("{name}, {}", line.trim()));
        }
    }
    None
}

fn supersede_keyword_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:amend(?:ing|s|ed)?|supersed(?:ing|es|ed)|repeal(?:ing|s|ed)|revok(?:ing|es|ed)|modif(?:ying|ies|ied))\b",
        )
        .expect("static regex")
    })
}

/// Find order numbers referenced near amend/supersede/repeal keywords.
/// A 200-character window after each keyword keeps the association tight —
/// a DO number three paragraphs away is a citation, not a superseded order.
fn extract_supersedes(text: &str, self_number: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for m in supersede_keyword_re().find_iter(text) {
        let window_end = (m.end() + 200).min(text.len());
        let window = char_boundary_slice(text, m.end(), window_end);
        for caps in order_number_dash_re().captures_iter(window) {
            let number = format!("DO-{}-{}", &caps[1], &caps[2]);
            if number != self_number && !found.contains(&number) {
                found.push(number);
            }
        }
    }
    found
}

fn republic_act_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:republic\s+act|r\.?\s*a\.?)\s*(?:no\.?\s*)?(\d{3,5})\b")
            .expect("static regex")
    })
}

fn presidential_decree_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:presidential\s+decree|p\.?\s*d\.?)\s*(?:no\.?\s*)?(\d{1,4})\b")
            .expect("static regex")
    })
}

/// Collect cited laws (Republic Acts, Presidential Decrees, the Labor Code)
/// in first-appearance order, de-duplicated.
fn extract_related_laws(text: &str) -> Vec<String> {
    let mut laws: Vec<String> = Vec::new();
    let push_unique = |laws: &mut Vec<String>, law: String| {
        if !laws.contains(&law) {
            laws.push(law);
        }
    };

    for caps in republic_act_re().captures_iter(text) {
        push_unique(&mut laws, format!("RA {}", &caps[1]));
    }
    for caps in presidential_decree_re().captures_iter(text) {
        push_unique(&mut laws, format!("PD {}", &caps[1]));
    }
    if text.to_ascii_lowercase().contains("labor code") {
        push_unique(&mut laws, "PD 442 (Labor Code)".to_string());
    }
    laws
}

/// Coarse topical tags from keyword presence in the document body.
fn extract_tags(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut tags: Vec<String> = Vec::new();
    for (keyword, tag) in TAG_KEYWORDS {
        if lower.contains(keyword) && !tags.iter().any(|t| t == tag) {
            tags.push((*tag).to_string());
        }
    }
    tags
}

/// Convert structural blocks into `Section`s.
///
/// Department Orders come in two layouts:
/// * Flat: "Section 1 … Section 30".
/// * Grouped: "RULE I / Section 1 …" (implementing rules) or
///   "ARTICLE I / Section 1 …".
///
/// When grouping units (Rules/Articles) coexist with Sections, each grouping
/// header is emitted as its own `Section` (carrying any intro text) and the
/// leaf sections under it are qualified as "Rule I, Section 2" so the flat
/// `sections` array preserves the hierarchy losslessly.
fn build_sections(text: &str) -> Vec<Section> {
    let blocks = split_into_blocks(text);
    let has_leaf_sections = blocks.iter().any(|b| b.kind == MarkerKind::Section);

    let mut sections = Vec::with_capacity(blocks.len());
    let mut current_group: Option<String> = None;

    for block in blocks {
        let is_group = has_leaf_sections && block.kind != MarkerKind::Section;
        let section_number = if is_group {
            current_group = Some(block.label.clone());
            block.label.clone()
        } else {
            match &current_group {
                Some(group) => format!("{group}, {}", block.label),
                None => block.label.clone(),
            }
        };

        // `content` keeps the text preceding the first enumeration marker;
        // the enumerated items live in `subsections`. When the body has no
        // intro (it opens directly with "(a) …"), the full body stays in
        // `content` to avoid an empty section, and subsections still carry
        // the per-item breakdown for fine-grained retrieval.
        let (intro, subsections) = extract_subsections(&block.content);
        let content = if intro.is_empty() {
            block.content.clone()
        } else {
            intro
        };

        sections.push(Section {
            section_number,
            title: block.title,
            content,
            subsections,
        });
    }

    sections
}
