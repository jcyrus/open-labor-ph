//! Labor Code of the Philippines (PD 442) PDF parser.
//!
//! Reads a Labor Code PDF and emits a JSON **array** of `LaborCodeArticle`
//! objects, assigning each article to its Book/Title/Chapter from the
//! running context headers ("BOOK THREE", "Title I", "Chapter II").
//!
//! Usage:
//! ```text
//! parse-labor-code <input.pdf> <output.json> [--manifest <sources.toml>]
//! ```
//!
//! Every article records the PDF's SHA-256 and the parser version in
//! `provenance`. If the PDF is pinned in the source manifest
//! (`data/sources.toml`), the entry must be a `labor_code` document.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use regex::Regex;
use tracing::{info, warn};

use labor_ingestion::manifest::{lookup_entry, sha256_file, DocumentKind};
use labor_ingestion::parser::{self, extract_subsections};
use labor_ingestion::parser::{date_to_utc, parse_first_date};
use labor_ingestion::types::{
    Amendment, LaborCodeArticle, LaborCodeBook, LaborCodeMetadata, Provenance,
};

/// An article delimited during the sequential scan, before the (parallel)
/// conversion into a `LaborCodeArticle`.
struct RawArticle {
    number: u32,
    /// Pre-renumbering article number from "Art. 294. [279]" style headings.
    original_number: Option<u32>,
    /// Remainder of the heading line (short title + possible inline body).
    heading_rest: String,
    /// Body lines until the next structural marker.
    body: String,
    book: LaborCodeBook,
    title_name: String,
    chapter: Option<String>,
}

const USAGE: &str = "Usage: parse-labor-code <input.pdf> <output.json> [--manifest <sources.toml>]";

struct Cli {
    input: PathBuf,
    output: PathBuf,
    /// Explicit manifest path; defaults to `data/sources.toml` found upward.
    manifest: Option<PathBuf>,
}

/// Parse CLI arguments. `Ok(None)` means help was requested.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Cli>> {
    let mut args = args.into_iter();
    let mut positional: Vec<String> = Vec::new();
    let mut manifest = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                manifest = Some(PathBuf::from(
                    args.next().context("--manifest requires a value")?,
                ));
            }
            "-h" | "--help" => return Ok(None),
            _ => positional.push(arg),
        }
    }

    if positional.len() != 2 {
        bail!(USAGE);
    }
    Ok(Some(Cli {
        input: PathBuf::from(&positional[0]),
        output: PathBuf::from(&positional[1]),
        manifest,
    }))
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
    let Some(Cli {
        input,
        output,
        manifest,
    }) = parse_args(std::env::args().skip(1))?
    else {
        println!("{USAGE}");
        return Ok(());
    };

    let sha256 =
        sha256_file(&input).with_context(|| format!("failed to hash {}", input.display()))?;
    match lookup_entry(manifest.as_deref(), &sha256)? {
        Some(entry) if entry.kind != DocumentKind::LaborCode => bail!(
            "manifest entry {} is a {:?}, not the Labor Code; use the matching parser",
            entry.id,
            entry.kind
        ),
        Some(entry) => info!(id = %entry.id, "matched source manifest entry by SHA-256"),
        None => info!(%sha256, "PDF is not pinned in the source manifest"),
    }
    let provenance = Provenance::new(sha256);

    let doc = parser::parse_document(&input)
        .with_context(|| format!("failed to parse PDF {}", input.display()))?;

    for w in &doc.warnings {
        warn!(page = w.page, "{}", w.message);
    }

    if doc.full_text.trim().is_empty() {
        bail!(
            "no text could be extracted from {} — the document appears to be a \
             scanned image and requires OCR before ingestion",
            input.display()
        );
    }

    let raw_articles = scan_articles(&doc.full_text);
    if raw_articles.is_empty() {
        bail!(
            "no article headings (\"Art. 294.\", \"ARTICLE 5.\") were found in {}; \
             is this actually a Labor Code document?",
            input.display()
        );
    }

    let raw_articles = dedupe_articles(raw_articles);

    // Article construction (subsection carving, heading parsing) is
    // independent per article, so it parallelizes cleanly with rayon.
    let mut articles: Vec<LaborCodeArticle> = raw_articles
        .into_par_iter()
        .map(|raw| build_article(raw, &provenance))
        .collect();

    // Deterministic output order regardless of scan/parallel ordering.
    articles.sort_by_key(|a| a.article_number);

    let json =
        serde_json::to_string_pretty(&articles).context("failed to serialize article list")?;
    std::fs::write(&output, json + "\n")
        .with_context(|| format!("failed to write {}", output.display()))?;

    info!(
        articles = articles.len(),
        output = %output.display(),
        "Labor Code parsed successfully"
    );
    Ok(())
}

fn book_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "BOOK" is required in capitals: book headers are typeset in full caps,
    // and the case requirement rejects prose like "this book provides".
    // "VII" must precede "VI" in the alternation, otherwise the trailing
    // `\b` fails on "VII" and Book Seven is never recognized.
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*BOOK\s+(?P<num>ONE|TWO|THREE|FOUR|FIVE|SIX|SEVEN|VII|VI|IV|V|III|II|I|[1-7])\b",
        )
        .expect("static regex")
    })
}

fn title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*(?:TITLE|Title)\s+(?P<num>[IVXLCDM]{1,7}|\d{1,2})\s*(?:[.:\u{2013}\u{2014}-]\s*)?(?P<rest>.*)$")
            .expect("static regex")
    })
}

fn chapter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*(?:CHAPTER|Chapter)\s+(?P<num>[IVXLCDM]{1,7}|\d{1,2})\s*(?:[.:\u{2013}\u{2014}-]\s*)?(?P<rest>.*)$")
            .expect("static regex")
    })
}

fn article_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Matches "Art. 294." / "ARTICLE 5:" / "Article 13." with an optional
    // bracketed pre-renumbering reference: "Art. 294. [279] Security of
    // Tenure." (the DOLE renumbered edition prints both numbers). The
    // separator after the number is mandatory so cross-references like
    // "Article 4 of this Code shall…" wrapped to a line start don't split
    // an article.
    RE.get_or_init(|| {
        Regex::new(r"^\s*(?:ART(?:ICLE)?|Art(?:icle)?)\.?\s+(?P<num>\d{1,3})\s*[.:]\s*(?:\[(?P<orig>\d{1,3})\]\s*)?(?P<rest>.*)$")
            .expect("static regex")
    })
}

/// Map a book token ("THREE", "III", "3") to the typed enum.
fn book_from_token(token: &str) -> Option<LaborCodeBook> {
    match token.to_ascii_uppercase().as_str() {
        "ONE" | "I" | "1" => Some(LaborCodeBook::BookI),
        "TWO" | "II" | "2" => Some(LaborCodeBook::BookII),
        "THREE" | "III" | "3" => Some(LaborCodeBook::BookIII),
        "FOUR" | "IV" | "4" => Some(LaborCodeBook::BookIV),
        "FIVE" | "V" | "5" => Some(LaborCodeBook::BookV),
        "SIX" | "VI" | "6" => Some(LaborCodeBook::BookVI),
        "SEVEN" | "VII" | "7" => Some(LaborCodeBook::BookVII),
        _ => None,
    }
}

/// Headers often put the unit name on the following line(s) in capitals:
///
/// ```text
/// Title I
/// TERMINATION OF EMPLOYMENT
/// ```
///
/// Collect up to two such continuation lines starting at `idx`.
fn caps_continuation(lines: &[&str], idx: usize) -> (String, usize) {
    let mut parts: Vec<&str> = Vec::new();
    let mut consumed = 0;
    for line in lines.iter().skip(idx).take(2) {
        let trimmed = line.trim();
        if trimmed.is_empty() || !is_caps_heading(trimmed) {
            break;
        }
        parts.push(trimmed);
        consumed += 1;
    }
    (parts.join(" "), consumed)
}

/// A short line that is overwhelmingly uppercase — a typeset heading, not
/// body prose.
fn is_caps_heading(line: &str) -> bool {
    if line.len() > 80 {
        return false;
    }
    let alphabetic: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    if alphabetic.is_empty() {
        return false;
    }
    let upper = alphabetic.iter().filter(|c| c.is_uppercase()).count();
    upper * 10 >= alphabetic.len() * 8
}

/// Sequentially scan the document, tracking Book/Title/Chapter context and
/// delimiting article bodies.
///
/// This pass must be sequential: an article's book assignment depends on
/// every header seen before it. The per-article construction afterwards is
/// what gets parallelized.
fn scan_articles(text: &str) -> Vec<RawArticle> {
    let lines: Vec<&str> = text.lines().collect();

    // Articles 1–11 sit in the Preliminary Title, before "BOOK ONE".
    let mut book = LaborCodeBook::Preliminary;
    let mut seen_book_header = false;
    let mut title_name = String::from("Preliminary Title");
    let mut chapter: Option<String> = None;

    let mut articles: Vec<RawArticle> = Vec::new();
    let mut current: Option<RawArticle> = None;
    let mut body_lines: Vec<&str> = Vec::new();

    let flush = |current: &mut Option<RawArticle>,
                 body_lines: &mut Vec<&str>,
                 out: &mut Vec<RawArticle>| {
        if let Some(mut article) = current.take() {
            article.body = body_lines.join("\n").trim().to_string();
            out.push(article);
        }
        body_lines.clear();
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        if let Some(caps) = book_re().captures(line) {
            if let Some(b) = book_from_token(&caps["num"]) {
                flush(&mut current, &mut body_lines, &mut articles);
                book = b;
                seen_book_header = true;
                // Each book restarts its Title/Chapter numbering.
                title_name = String::from("General Provisions");
                chapter = None;
                // Skip the book's name line(s) ("CONDITIONS OF EMPLOYMENT").
                let (_, consumed) = caps_continuation(&lines, i + 1);
                i += 1 + consumed;
                continue;
            }
        }

        if let Some(caps) = title_re().captures(line) {
            let num = &caps["num"];
            let rest = caps["rest"].trim().to_string();
            // Only treat as a header when the name is inline or follows in
            // capitals; "Title to the property shall…" has neither.
            let (cont, consumed) = if rest.is_empty() {
                caps_continuation(&lines, i + 1)
            } else {
                (String::new(), 0)
            };
            let name = if rest.is_empty() { cont } else { rest };
            if !name.is_empty() || seen_book_header {
                flush(&mut current, &mut body_lines, &mut articles);
                title_name = if name.is_empty() {
                    format!("Title {num}")
                } else {
                    format!("Title {num}: {name}")
                };
                chapter = None;
                i += 1 + consumed;
                continue;
            }
        }

        if let Some(caps) = chapter_re().captures(line) {
            let num = &caps["num"];
            let rest = caps["rest"].trim().to_string();
            let (cont, consumed) = if rest.is_empty() {
                caps_continuation(&lines, i + 1)
            } else {
                (String::new(), 0)
            };
            let name = if rest.is_empty() { cont } else { rest };
            if !name.is_empty() || seen_book_header {
                flush(&mut current, &mut body_lines, &mut articles);
                chapter = Some(if name.is_empty() {
                    format!("Chapter {num}")
                } else {
                    format!("Chapter {num}: {name}")
                });
                i += 1 + consumed;
                continue;
            }
        }

        if let Some(caps) = article_re().captures(line) {
            if let Ok(number) = caps["num"].parse::<u32>() {
                flush(&mut current, &mut body_lines, &mut articles);
                current = Some(RawArticle {
                    number,
                    original_number: caps.name("orig").and_then(|m| m.as_str().parse().ok()),
                    heading_rest: caps["rest"].trim().to_string(),
                    body: String::new(),
                    book: book.clone(),
                    title_name: title_name.clone(),
                    chapter: chapter.clone(),
                });
                i += 1;
                continue;
            }
        }

        if current.is_some() {
            body_lines.push(line);
        }
        i += 1;
    }
    flush(&mut current, &mut body_lines, &mut articles);

    articles
}

/// Drop repeated article numbers, keeping the longest body of each.
///
/// A table of contents ("Art. 294. Security of Tenure ... 112") or a
/// heading repeated by the page layout produces a near-empty duplicate of
/// the real article. Scan order is preserved for the kept articles.
fn dedupe_articles(articles: Vec<RawArticle>) -> Vec<RawArticle> {
    let size = |a: &RawArticle| a.heading_rest.len() + a.body.len();

    let mut longest: HashMap<u32, usize> = HashMap::new();
    for (i, article) in articles.iter().enumerate() {
        longest
            .entry(article.number)
            .and_modify(|best| {
                if size(article) > size(&articles[*best]) {
                    *best = i;
                }
            })
            .or_insert(i);
    }

    let keep: HashSet<usize> = longest.into_values().collect();
    let dropped = articles.len() - keep.len();
    if dropped > 0 {
        warn!(
            dropped,
            "dropped duplicate articles (table of contents or repeated headings); kept the longest of each"
        );
    }

    articles
        .into_iter()
        .enumerate()
        .filter(|(i, _)| keep.contains(i))
        .map(|(_, a)| a)
        .collect()
}

/// Convert a delimited raw article into the typed domain struct.
fn build_article(raw: RawArticle, provenance: &Provenance) -> LaborCodeArticle {
    // Heading style is "Security of Tenure. – In cases of regular
    // employment…": short title, dash, then the body starts inline.
    let (heading, inline_body) = split_article_heading(&raw.heading_rest);

    let content = match (inline_body.is_empty(), raw.body.is_empty()) {
        (false, false) => format!("{inline_body}\n{}", raw.body),
        (false, true) => inline_body,
        (true, _) => raw.body.clone(),
    };

    // `content` is the full article body, enumerated items included;
    // `subsections` repeats the items one by one for fine-grained retrieval.
    let (_, subsections) = extract_subsections(&content);

    let amendments = extract_amendments(&content);
    let original_pd_442 = !amendment_note_re()
        .captures_iter(&content)
        .any(|caps| is_insertion_note(&caps["note"]));
    let related_articles = extract_related_articles(&content, raw.number);

    LaborCodeArticle {
        article_number: raw.number,
        former_article_number: raw.original_number,
        book: raw.book,
        title_name: raw.title_name,
        chapter: raw.chapter,
        heading,
        content,
        metadata: LaborCodeMetadata {
            original_pd_442,
            amendments,
            related_articles,
            // The Labor Code text never cites the DOLE Orders that implement
            // it; this is filled by cross-linking with the DOLE dataset.
            implementing_orders: Vec::new(),
            tags: Vec::new(),
        },
        provenance: provenance.clone(),
        subsections,
    }
}

fn amendment_note_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Editorial notes printed after amended articles:
    // "(As amended by Section 34, Republic Act No. 6715, March 21, 1989)",
    // "(As inserted by Republic Act No. 10151, June 21, 2011)".
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\(\s*(?P<note>(?:as\s+)?(?:amended|inserted|added|incorporated)\s+by\b[^)]*)\)",
        )
        .expect("static regex")
    })
}

fn law_citation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Republic Act No. 6715", "R.A. 6715", "P.D. 570-A", "B.P. Blg. 130",
    // "Executive Order No. 111".
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:(?P<ra>republic\s+act|r\.\s*a\.|ra)|(?P<pd>presidential\s+decree|p\.\s*d\.|pd)|(?P<bp>batas\s+pambansa|b\.\s*p\.|bp)|(?P<eo>executive\s+order|e\.\s*o\.|eo))\s*(?:no\.?|blg\.?|bilang)?\s*(?P<num>\d{1,5}(?:-[a-z])?)\b",
        )
        .expect("static regex")
    })
}

/// Whether an amendment note records an article added after 1974 rather
/// than a change to an original PD 442 article.
fn is_insertion_note(note: &str) -> bool {
    let lower = note.trim_start().to_ascii_lowercase();
    let verb = lower.strip_prefix("as ").unwrap_or(&lower).trim_start();
    ["inserted", "added", "incorporated"]
        .iter()
        .any(|v| verb.starts_with(v))
}

/// Parse "(As amended by …)" notes into amendments, one per cited law.
///
/// The note's date is attached only when the note cites a single law; with
/// several laws ("As amended by PD 570-A and PD 643, …") it is ambiguous
/// which law the date belongs to, so it is left unset rather than guessed.
fn extract_amendments(content: &str) -> Vec<Amendment> {
    let mut amendments: Vec<Amendment> = Vec::new();
    for caps in amendment_note_re().captures_iter(content) {
        let note = caps["note"]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let laws: Vec<String> = law_citation_re()
            .captures_iter(&note)
            .map(|law| {
                let prefix = if law.name("ra").is_some() {
                    "RA"
                } else if law.name("pd").is_some() {
                    "PD"
                } else if law.name("bp").is_some() {
                    "BP"
                } else {
                    "EO"
                };
                format!("{prefix} {}", law["num"].to_ascii_uppercase())
            })
            .collect();
        let date = if laws.len() == 1 {
            parse_first_date(&note).map(date_to_utc)
        } else {
            None
        };
        for law in laws {
            if amendments.iter().any(|a| a.law == law) {
                continue;
            }
            amendments.push(Amendment {
                law,
                date,
                description: Some(note.clone()),
            });
        }
    }
    amendments
}

fn article_reference_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "Article 297", "Art. 298", "Articles 106 to 109", "Articles 297 and 298".
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\bart(?:icle)?s?\.?\s+(?P<a>\d{1,3})(?:\s*(?P<sep>to|and|-|\u{2013})\s*(?P<b>\d{1,3}))?\b",
        )
        .expect("static regex")
    })
}

/// Ranges wider than this ("Articles 1 to 300") are treated as typos or
/// book-level references and only their endpoints are recorded.
const MAX_ARTICLE_RANGE: u32 = 20;

/// Collect Labor Code articles cross-referenced in `content`, excluding the
/// article itself. A reference followed by "of" a different instrument
/// ("Article 1700 of the Civil Code") is skipped; "of this Code" and "of the
/// Labor Code" are kept.
fn extract_related_articles(content: &str, self_number: u32) -> Vec<u32> {
    let mut related: Vec<u32> = Vec::new();
    for caps in article_reference_re().captures_iter(content) {
        let whole = caps.get(0).expect("match group 0 always present");
        let after: String = content[whole.end()..]
            .chars()
            .take(40)
            .collect::<String>()
            .to_ascii_lowercase();
        let after = after.trim_start_matches([',', ' ', '\n']);
        if after.starts_with("of ") && !after.contains("this code") && !after.contains("labor code")
        {
            continue;
        }

        let Ok(a) = caps["a"].parse::<u32>() else {
            continue;
        };
        let b = caps.name("b").and_then(|m| m.as_str().parse::<u32>().ok());
        let is_range = caps
            .name("sep")
            .is_some_and(|s| !s.as_str().eq_ignore_ascii_case("and"));
        match b {
            Some(b) if is_range && b > a && b - a <= MAX_ARTICLE_RANGE => related.extend(a..=b),
            Some(b) => related.extend([a, b]),
            None => related.push(a),
        }
    }
    related.retain(|n| *n != self_number);
    related.sort_unstable();
    related.dedup();
    related
}

/// Split the heading remainder into (short title, inline body).
fn split_article_heading(rest: &str) -> (Option<String>, String) {
    let rest = rest.trim();
    if rest.is_empty() {
        return (None, String::new());
    }

    // Dash separator: everything before it is the heading. Em dash, en
    // dash, and the spaced ASCII " - " variant are all accepted.
    for dash in ["\u{2014}", "\u{2013}", " - "] {
        if let Some(idx) = rest.find(dash) {
            let heading = rest[..idx].trim().trim_end_matches('.').trim();
            let body = rest[idx + dash.len()..].trim();
            let heading = (!heading.is_empty()).then(|| heading.to_string());
            return (heading, body.to_string());
        }
    }

    // No dash: a short period-terminated line is a bare heading
    // ("Art. 5. Rules and regulations."), anything longer is body text.
    if rest.len() < 100 && rest.ends_with('.') && !rest.trim_end_matches('.').contains(". ") {
        return (Some(rest.trim_end_matches('.').to_string()), String::new());
    }

    (None, rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "PRELIMINARY TITLE\n\
        CHAPTER I\n\
        GENERAL PROVISIONS\n\
        Art. 1. Name of Decree. - This Decree shall be known as the Labor Code of the Philippines.\n\
        BOOK ONE\n\
        PRE-EMPLOYMENT\n\
        Art. 12. Statement of Objectives. - It is the policy of the State.\n\
        BOOK SIX\n\
        POST-EMPLOYMENT\n\
        Title I\n\
        TERMINATION OF EMPLOYMENT\n\
        Art. 294. [279] Security of Tenure. - In cases of regular employment.\n\
        BOOK SEVEN\n\
        TRANSITORY AND FINAL PROVISIONS\n\
        Title II\n\
        PRESCRIPTION OF OFFENSES AND CLAIMS\n\
        Art. 306. [291] Money Claims. - All money claims shall be filed within three (3) years.\n";

    fn build(raw: RawArticle) -> LaborCodeArticle {
        build_article(raw, &Provenance::new("0".repeat(64)))
    }

    fn book_of(articles: &[RawArticle], number: u32) -> LaborCodeBook {
        articles
            .iter()
            .find(|a| a.number == number)
            .map(|a| a.book.clone())
            .unwrap_or_else(|| panic!("article {number} not scanned"))
    }

    #[test]
    fn test_preliminary_title_articles_are_not_book_one() {
        let articles = scan_articles(SAMPLE);
        assert_eq!(book_of(&articles, 1), LaborCodeBook::Preliminary);
        assert_eq!(book_of(&articles, 12), LaborCodeBook::BookI);
    }

    #[test]
    fn test_book_seven_is_recognized() {
        let articles = scan_articles(SAMPLE);
        assert_eq!(book_of(&articles, 294), LaborCodeBook::BookVI);
        let money_claims = articles.iter().find(|a| a.number == 306).unwrap();
        assert_eq!(money_claims.book, LaborCodeBook::BookVII);
        assert_eq!(
            money_claims.title_name,
            "Title II: PRESCRIPTION OF OFFENSES AND CLAIMS"
        );
        assert_eq!(money_claims.original_number, Some(291));
    }

    #[test]
    fn test_book_roman_seven_not_truncated_to_six() {
        let caps = book_re().captures("BOOK VII").expect("BOOK VII matches");
        assert_eq!(book_from_token(&caps["num"]), Some(LaborCodeBook::BookVII));
    }

    #[test]
    fn test_toc_duplicate_articles_keep_body() {
        let text = "BOOK SIX\nPOST-EMPLOYMENT\n\
                    Art. 294. Security of Tenure.\n\
                    Art. 295. Regular and Casual Employment.\n\
                    Art. 294. [279] Security of Tenure. - In cases of regular employment, \
                    the employer shall not terminate the services of an employee.\n\
                    Art. 295. [280] Regular and Casual Employment. - The provisions of \
                    written agreement to the contrary notwithstanding.\n";
        let articles = dedupe_articles(scan_articles(text));
        assert_eq!(articles.len(), 2);
        let tenure = articles.iter().find(|a| a.number == 294).unwrap();
        assert_eq!(tenure.original_number, Some(279));
    }

    #[test]
    fn test_article_content_keeps_enumerated_items() {
        let text = "Art. 297. [282] Termination by Employer. - An employer may terminate \
                    an employment for any of the following causes:\n\
                    (a) Serious misconduct;\n\
                    (b) Gross and habitual neglect of duties;\n";
        let article = build(scan_articles(text).remove(0));
        assert!(article.content.contains("(b) Gross and habitual neglect"));
        assert_eq!(article.subsections.len(), 2);
    }

    #[test]
    fn test_former_number_is_a_field_not_a_tag() {
        let text = "Art. 294. [279] Security of Tenure. - In cases of regular employment.\n";
        let article = build(scan_articles(text).remove(0));
        assert_eq!(article.former_article_number, Some(279));
        assert!(article.metadata.tags.is_empty());
    }

    #[test]
    fn test_amendment_note_with_single_law_keeps_date() {
        let content = "The employer shall not terminate.\n\
                       (As amended by Section 34, Republic Act No. 6715, March 21, 1989)";
        let amendments = extract_amendments(content);
        assert_eq!(amendments.len(), 1);
        assert_eq!(amendments[0].law, "RA 6715");
        assert_eq!(
            amendments[0].date.map(|d| d.date_naive()),
            chrono::NaiveDate::from_ymd_opt(1989, 3, 21)
        );
        assert_eq!(
            amendments[0].description.as_deref(),
            Some("As amended by Section 34, Republic Act No. 6715, March 21, 1989")
        );
    }

    #[test]
    fn test_amendment_note_with_several_laws_leaves_date_unset() {
        let content = "(As amended by Presidential Decree No. 570-A and \
                       Batas Pambansa Blg. 130, November 1, 1974)";
        let amendments = extract_amendments(content);
        let laws: Vec<&str> = amendments.iter().map(|a| a.law.as_str()).collect();
        assert_eq!(laws, ["PD 570-A", "BP 130"]);
        assert!(amendments.iter().all(|a| a.date.is_none()));
    }

    #[test]
    fn test_inserted_article_is_not_original_pd_442() {
        let text = "Art. 128. Visitorial power. - The Secretary may inspect.\n\
                    (As inserted by Republic Act No. 7730, June 2, 1994)\n\
                    Art. 129. Recovery of wages. - Upon complaint.\n";
        let articles: Vec<LaborCodeArticle> = scan_articles(text).into_iter().map(build).collect();
        assert!(!articles[0].metadata.original_pd_442);
        assert_eq!(articles[0].metadata.amendments[0].law, "RA 7730");
        assert!(articles[1].metadata.original_pd_442);
    }

    #[test]
    fn test_related_articles_ranges_and_foreign_codes() {
        let content = "Subject to Articles 106 to 109 and Article 297 of this Code, \
                       and to Article 19 of the Civil Code, see also Article 294.";
        assert_eq!(
            extract_related_articles(content, 294),
            [106, 107, 108, 109, 297]
        );
    }

    #[test]
    fn test_help_is_not_an_error() {
        assert!(parse_args(["--help".to_string()]).unwrap().is_none());
    }
}
