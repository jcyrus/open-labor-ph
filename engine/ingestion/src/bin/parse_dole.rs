//! DOLE Department Order PDF parser.
//!
//! Reads a Department Order PDF, applies the structural heuristics from
//! `labor_ingestion::parser`, and writes a schema-compliant `DoleOrder`
//! JSON file.
//!
//! Usage:
//! ```text
//! parse-dole <input.pdf> <output.json> [--source-url <url>]
//!            [--published-date YYYY-MM-DD] [--effective-date YYYY-MM-DD]
//! ```
//!
//! Department Orders usually take effect a fixed number of days after
//! publication, and the publication date is not printed in the Order itself.
//! Pass `--published-date` (from the Official Gazette or newspaper notice)
//! to let the parser compute `effective_date`; without it the field is
//! omitted rather than guessed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use chrono::{Days, NaiveDate};
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

const USAGE: &str = "Usage: parse-dole <input.pdf> <output.json> [--source-url <url>] \
                     [--published-date YYYY-MM-DD] [--effective-date YYYY-MM-DD]";

struct Cli {
    input: PathBuf,
    output: PathBuf,
    source_url: Option<String>,
    /// Official publication date, used to resolve "N days after publication".
    published_date: Option<NaiveDate>,
    /// Operator-supplied effective date; overrides anything derived from text.
    effective_date: Option<NaiveDate>,
}

fn parse_args() -> Result<Cli> {
    let mut args = std::env::args().skip(1);
    let mut positional: Vec<String> = Vec::new();
    let mut source_url = None;
    let mut published_date = None;
    let mut effective_date = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source-url" => {
                source_url = Some(args.next().context("--source-url requires a value")?);
            }
            "--published-date" => {
                published_date = Some(parse_date_arg("--published-date", args.next())?);
            }
            "--effective-date" => {
                effective_date = Some(parse_date_arg("--effective-date", args.next())?);
            }
            "-h" | "--help" => bail!(USAGE),
            _ => positional.push(arg),
        }
    }

    if positional.len() != 2 {
        bail!(USAGE);
    }

    Ok(Cli {
        input: PathBuf::from(&positional[0]),
        output: PathBuf::from(&positional[1]),
        source_url,
        published_date,
        effective_date,
    })
}

fn parse_date_arg(flag: &str, value: Option<String>) -> Result<NaiveDate> {
    let value = value.with_context(|| format!("{flag} requires a value (YYYY-MM-DD)"))?;
    NaiveDate::parse_from_str(&value, "%Y-%m-%d")
        .with_context(|| format!("{flag}: invalid date {value:?}; expected YYYY-MM-DD"))
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

    let mut sections = build_sections(text);

    let signed_date = extract_signed_date(text);
    let effectivity_clause = extract_effectivity_clause(&sections, text);
    let effective_date = cli.effective_date.or_else(|| {
        resolve_effective_date(
            effectivity_clause.as_deref(),
            cli.published_date,
            signed_date,
        )
    });
    if effective_date.is_none() {
        warn!(
            clause = effectivity_clause.as_deref().unwrap_or("<none found>"),
            "effective date could not be determined from the document; \
             pass --published-date or --effective-date to resolve it"
        );
    }

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
        published_date: cli.published_date.map(date_to_utc),
        signed_date: signed_date.map(date_to_utc),
        effectivity_clause,
    };

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
        effective_date: effective_date.map(date_to_utc),
        source_url,
        metadata,
        sections,
    })
}

// All three order-number patterns expose the same named groups — `num`,
// an optional amendment `letter` ("18-A"), and a 2- or 4-digit `year` — so
// `order_id_from_captures` can normalize any of them.

fn order_number_dash_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Department Order No. 174-17" / "D.O. No. 18-A-11"
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:department\s+order|d\.?\s*o\.?)\s*no\.?\s*(?P<num>\d{1,4})(?:\s*-\s*(?P<letter>[a-z]))?\s*-\s*(?P<year>\d{2})\b",
        )
        .expect("static regex")
    })
}

fn order_number_series_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Department Order No. 219, Series of 2021" / "No. 18-A, s. 2011"
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:department\s+order|d\.?\s*o\.?)\s*no\.?\s*(?P<num>\d{1,4})(?:\s*-\s*(?P<letter>[a-z]))?\s*,?\s*\(?(?:series\s+of|s\.)\s*(?P<year>\d{4})\)?",
        )
        .expect("static regex")
    })
}

fn filename_order_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "DO-174-17.pdf" / "DO_18-A_2011.pdf"
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\bDO[-_ ]?(?P<num>\d{1,4})(?:[-_](?P<letter>[a-z]))?[-_](?P<year>\d{2,4})\b",
        )
        .expect("static regex")
    })
}

/// Normalize captures to the schema's `DO-NNN-YY` or, for lettered
/// amendments, `DO-NNN-X-YY` identifier.
fn order_id_from_captures(caps: &regex::Captures<'_>) -> String {
    let year = &caps["year"];
    let yy = &year[year.len() - 2..];
    match caps.name("letter") {
        Some(letter) => format!(
            "DO-{}-{}-{yy}",
            &caps["num"],
            letter.as_str().to_ascii_uppercase()
        ),
        None => format!("DO-{}-{yy}", &caps["num"]),
    }
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
    let from_text = [order_number_dash_re(), order_number_series_re()]
        .into_iter()
        .filter_map(|re| re.captures(text))
        .min_by_key(|caps| caps.get(0).map_or(usize::MAX, |m| m.start()))
        .map(|caps| order_id_from_captures(&caps));
    if from_text.is_some() {
        return from_text;
    }

    let filename = input.file_stem()?.to_string_lossy();
    let caps = filename_order_re().captures(&filename)?;
    Some(order_id_from_captures(&caps))
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

/// Find the signing date: the date following a signing keyword ("Done in
/// the City of Manila, this 16th day of March, 2017"). The **last** keyword
/// is tried first because the signature block closes the document, while
/// "issued"/"approved" in the preamble usually cite older issuances. Falls
/// back to the first date on the signature page (final 2 000 characters).
fn extract_signed_date(text: &str) -> Option<NaiveDate> {
    let keywords: Vec<_> = signing_context_re().find_iter(text).collect();
    for m in keywords.into_iter().rev() {
        let window_end = (m.end() + 120).min(text.len());
        let window = char_boundary_slice(text, m.end(), window_end);
        if let Some(date) = parse_first_date(window) {
            return Some(date);
        }
    }

    let tail_start = text.len().saturating_sub(2000);
    parse_first_date(char_boundary_slice(text, tail_start, text.len()))
}

fn effectivity_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:shall\s+take\s+effect|shall\s+(?:become|be)\s+effective|takes\s+effect)\b",
        )
        .expect("static regex")
    })
}

/// Locate the sentence stating when the Order takes effect. A section
/// titled "Effectivity" is searched first; otherwise the **last**
/// effectivity phrase in the document is used, since earlier ones usually
/// say when a particular obligation applies, not the Order itself. Only the
/// enclosing sentence is kept, so a signature block that the section
/// splitter left in the same section never leaks its date into the clause.
fn extract_effectivity_clause(sections: &[Section], text: &str) -> Option<String> {
    let section_text = sections
        .iter()
        .rev()
        .find(|s| {
            s.title
                .as_deref()
                .is_some_and(|t| t.to_ascii_lowercase().contains("effectiv"))
        })
        .map(|s| {
            std::iter::once(s.content.as_str())
                .chain(s.subsections.iter().map(|sub| sub.content.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        });
    let haystack = section_text.as_deref().unwrap_or(text);

    let m = effectivity_re().find_iter(haystack).last()?;
    let sentence = enclosing_sentence(haystack, m.start(), m.end());
    Some(sentence.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Abbreviations that end in a period without ending the sentence.
const CITATION_ABBREVIATIONS: &[&str] = &["No", "Nos", "Sec", "Art", "Rep", "s"];

/// Expand `[start, end)` to its enclosing sentence. Sentences end at a blank
/// line or at a period followed by whitespace that does not close a
/// citation abbreviation ("Department Order No. 174").
fn enclosing_sentence(text: &str, start: usize, end: usize) -> &str {
    let ends_sentence = |dot: usize| {
        let followed_by_space = text[dot + 1..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace);
        let word_before = text[..dot]
            .rsplit(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("");
        followed_by_space
            && !CITATION_ABBREVIATIONS
                .iter()
                .any(|a| a.eq_ignore_ascii_case(word_before))
    };

    let mut sentence_start = 0;
    for (i, c) in text[..start].char_indices().rev() {
        if (c == '.' && ends_sentence(i)) || (c == '\n' && text[..i].ends_with('\n')) {
            sentence_start = i + 1;
            break;
        }
    }

    let mut sentence_end = text.len();
    for (offset, c) in text[end..].char_indices() {
        let i = end + offset;
        if c == '.' && ends_sentence(i) {
            sentence_end = i + 1;
            break;
        }
        if c == '\n' && text[i + 1..].starts_with('\n') {
            sentence_end = i;
            break;
        }
    }

    text[sentence_start..sentence_end]
        .trim()
        .trim_start_matches(['\u{2014}', '\u{2013}', '-'])
        .trim()
}

fn days_after_digits_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "fifteen (15) days after" / "15 calendar days following"
    RE.get_or_init(|| {
        Regex::new(r"(?i)\(?(\d{1,3})\)?\s*(?:calendar\s+)?days?\s+(?:after|following|from)\b")
            .expect("static regex")
    })
}

fn days_after_words_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Spelled-out offsets without a numeral: "fifteen days after".
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(ten|fifteen|thirty|sixty)\s+(?:calendar\s+)?days?\s+(?:after|following|from)\b")
            .expect("static regex")
    })
}

/// The day offset stated in an effectivity clause, if any. "Working days"
/// are deliberately not matched: they depend on the holiday calendar and
/// cannot be resolved deterministically.
fn effectivity_offset_days(clause: &str) -> Option<u64> {
    if let Some(caps) = days_after_digits_re().captures(clause) {
        return caps[1].parse().ok();
    }
    let caps = days_after_words_re().captures(clause)?;
    match caps[1].to_ascii_lowercase().as_str() {
        "ten" => Some(10),
        "fifteen" => Some(15),
        "thirty" => Some(30),
        "sixty" => Some(60),
        _ => None,
    }
}

/// Derive the effective date from the effectivity clause, or return `None`
/// when the document alone does not determine it. Never guesses: a clause
/// like "fifteen (15) days after publication" resolves only when the
/// publication date is known.
///
/// Resolution order:
/// 1. An explicit date in the clause ("shall take effect on January 1, 2022").
/// 2. "N days after publication / signing" → that base date + N days.
/// 3. "upon publication" → the publication date.
/// 4. "immediately" (with no mention of publication) → the signing date.
///
/// Working-day offsets are never resolved (see [`effectivity_offset_days`]).
fn resolve_effective_date(
    clause: Option<&str>,
    published: Option<NaiveDate>,
    signed: Option<NaiveDate>,
) -> Option<NaiveDate> {
    let clause = clause?;
    if let Some(date) = parse_first_date(clause) {
        return Some(date);
    }

    let lower = clause.to_ascii_lowercase();
    // A working-day offset depends on the holiday calendar; it must not
    // fall through to the "upon publication" rule below.
    if lower.contains("working day") {
        return None;
    }
    let mentions_publication = lower.contains("publication");
    let base = if mentions_publication {
        published
    } else if lower.contains("sign") || lower.contains("approv") {
        signed
    } else {
        None
    };

    if let Some(days) = effectivity_offset_days(clause) {
        return base?.checked_add_days(Days::new(days));
    }
    if mentions_publication {
        return published;
    }
    if lower.contains("immediately") {
        return signed;
    }
    None
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
///
/// The window spans 200 characters on **both** sides of the keyword,
/// clipped to the keyword's paragraph: repealing clauses put the reference
/// either before the verb ("D.O. No. 18-A, Series of 2011 is hereby
/// superseded") or after it ("superseding D.O. No. 18-A-11"), while a DO
/// number in a different paragraph is a citation, not a superseded order.
fn extract_supersedes(text: &str, self_number: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for m in supersede_keyword_re().find_iter(text) {
        let paragraph_start = text[..m.start()].rfind("\n\n").map_or(0, |i| i + 2);
        let paragraph_end = text[m.end()..]
            .find("\n\n")
            .map_or(text.len(), |i| m.end() + i);
        let start = m.start().saturating_sub(200).max(paragraph_start);
        let end = (m.end() + 200).min(paragraph_end);
        let window = char_boundary_slice(text, start, end);

        for re in [order_number_dash_re(), order_number_series_re()] {
            for caps in re.captures_iter(window) {
                let number = order_id_from_captures(&caps);
                if number != self_number && !found.contains(&number) {
                    found.push(number);
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, d)
    }

    fn section(title: &str, content: &str) -> Section {
        Section {
            section_number: "Section 10".to_string(),
            title: Some(title.to_string()),
            content: content.to_string(),
            subsections: Vec::new(),
        }
    }

    const NO_FILE: &str = "order.pdf";

    #[test]
    fn test_order_number_lettered_dash_style() {
        let text = "DEPARTMENT ORDER NO. 18-A-11\nRULES IMPLEMENTING ARTICLES 106 TO 109";
        assert_eq!(
            extract_order_number(text, Path::new(NO_FILE)).as_deref(),
            Some("DO-18-A-11")
        );
    }

    #[test]
    fn test_order_number_lettered_series_style() {
        let text = "Department Order No. 18-A\nSeries of 2011\n";
        assert_eq!(
            extract_order_number(text, Path::new(NO_FILE)).as_deref(),
            Some("DO-18-A-11")
        );
    }

    #[test]
    fn test_order_number_plain_series_across_lines() {
        let text = "DEPARTMENT ORDER NO. 174\nSeries of 2017\n";
        assert_eq!(
            extract_order_number(text, Path::new(NO_FILE)).as_deref(),
            Some("DO-174-17")
        );
    }

    #[test]
    fn test_order_number_lettered_filename_fallback() {
        assert_eq!(
            extract_order_number("no header", Path::new("DO_18-A_2011.pdf")).as_deref(),
            Some("DO-18-A-11")
        );
    }

    #[test]
    fn test_supersedes_reference_before_keyword() {
        let text = "Section 3. Repealing Clause. - Department Order No. 18-A, Series of 2011 \
                    is hereby superseded.";
        assert_eq!(extract_supersedes(text, "DO-174-17"), vec!["DO-18-A-11"]);
    }

    #[test]
    fn test_supersedes_ignores_other_paragraphs() {
        let text = "Department Order No. 40-03 provides the framework.\n\n\
                    This Order amends the reporting forms.";
        assert!(extract_supersedes(text, "DO-174-17").is_empty());
    }

    #[test]
    fn test_signed_date_prefers_signature_block_over_preamble() {
        let text = "WHEREAS, the rules issued on March 1, 2010 require revision;\n\n\
                    Done in the City of Manila, this 16th day of March, 2017.";
        assert_eq!(extract_signed_date(text), date(2017, 3, 16));
    }

    #[test]
    fn test_effectivity_clause_excludes_signature_block() {
        let sections = [section(
            "Effectivity",
            "This Order shall take effect fifteen (15) days after its publication in a \
             newspaper of general circulation.\n\n\
             Done in the City of Manila, this 16th day of March, 2017.",
        )];
        let clause = extract_effectivity_clause(&sections, "").unwrap();
        assert_eq!(
            clause,
            "This Order shall take effect fifteen (15) days after its publication in a \
             newspaper of general circulation."
        );
        // Without a publication date the effective date stays unknown
        // rather than borrowing the signing date.
        assert_eq!(
            resolve_effective_date(Some(&clause), None, date(2017, 3, 16)),
            None
        );
    }

    #[test]
    fn test_effectivity_clause_survives_citation_abbreviations() {
        let text = "Section 9. Effectivity. \u{2014} This Order shall take effect upon \
                    publication, consistent with Department Order No. 174, Series of 2017.";
        let clause = extract_effectivity_clause(&[], text).unwrap();
        assert!(clause.starts_with("This Order shall take effect"));
        assert!(clause.ends_with("Series of 2017."));
    }

    #[test]
    fn test_effective_date_days_after_publication() {
        let clause = "This Order shall take effect fifteen (15) days after its publication.";
        assert_eq!(
            resolve_effective_date(Some(clause), date(2019, 4, 25), None),
            date(2019, 5, 10)
        );
    }

    #[test]
    fn test_effective_date_spelled_out_days() {
        let clause = "This Order shall take effect thirty days following its publication.";
        assert_eq!(
            resolve_effective_date(Some(clause), date(2021, 1, 1), None),
            date(2021, 1, 31)
        );
    }

    #[test]
    fn test_effective_date_explicit() {
        let clause = "This Order shall take effect on January 1, 2022.";
        assert_eq!(
            resolve_effective_date(Some(clause), None, None),
            date(2022, 1, 1)
        );
    }

    #[test]
    fn test_effective_date_immediately_uses_signing_date() {
        let clause = "This Order shall take effect immediately.";
        assert_eq!(
            resolve_effective_date(Some(clause), None, date(2017, 3, 16)),
            date(2017, 3, 16)
        );
    }

    #[test]
    fn test_effective_date_working_days_not_guessed() {
        let clause = "This Order shall take effect ten (10) working days after publication.";
        assert_eq!(
            resolve_effective_date(Some(clause), date(2020, 6, 1), None),
            None
        );
    }

    #[test]
    fn test_effective_date_absent_clause_is_none() {
        assert_eq!(resolve_effective_date(None, date(2020, 6, 1), None), None);
    }
}
