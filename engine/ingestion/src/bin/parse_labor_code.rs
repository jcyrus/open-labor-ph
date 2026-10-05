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

use std::collections::{BTreeMap, HashMap, HashSet};
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
    Amendment, Footnote, LaborCodeArticle, LaborCodeBook, LaborCodeMetadata, Provenance,
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

    // Editorial footnotes (amendment history, implementing rules) are moved
    // out of the running text before scanning, and their inline markers are
    // turned into tokens that `build_article` resolves per article.
    // Footnotes sit at the foot of each page, so split per page; without
    // page boundaries fall back to paragraphs.
    let units: Vec<&str> = if doc.pages.len() > 1 {
        doc.pages.iter().map(|p| p.content.as_str()).collect()
    } else {
        doc.full_text.split("\n\n").collect()
    };
    let (body_text, footnotes) = split_footnotes(&units);
    let body_text = mark_footnote_refs(&body_text, &footnotes);
    info!(footnotes = footnotes.len(), "separated editorial footnotes");

    let raw_articles = scan_articles(&body_text);
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
        .map(|raw| build_article(raw, &provenance, &footnotes))
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
    // "BOOK THREE" and the DOLE edition's "Book Three – CONDITIONS OF
    // EMPLOYMENT". After the number only a separator or the end of the line
    // may follow, so a wrapped citation such as "Book IV of the Omnibus
    // Rules" never switches books. "VII" must precede "VI" in the
    // alternation, otherwise "VII" would be cut short.
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*(?:BOOK|Book)\s+(?P<num>ONE|One|TWO|Two|THREE|Three|FOUR|Four|FIVE|Five|SIX|Six|SEVEN|Seven|VII|VI|IV|V|III|II|I|[1-7])\s*(?:$|[-.:\u{2013}\u{2014}])",
        )
        .expect("static regex")
    })
}

fn title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // A separator or the end of the line must follow the number, so
        // "Title to the land…" and "Title II of the Code…" are prose.
        Regex::new(r"^\s*(?:TITLE|Title)\s+(?P<num>[IVXLCDM]{1,7}|\d{1,2})(?:\s*[.:\u{2013}\u{2014}-]\s*(?P<rest>.*)|\s*)$")
            .expect("static regex")
    })
}

fn chapter_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // As for titles: "Chapter II, of this Title…" is prose, not a header.
        Regex::new(r"^\s*(?:CHAPTER|Chapter)\s+(?P<num>[IVXLCDM]{1,7}|\d{1,2})(?:\s*[.:\u{2013}\u{2014}-]\s*(?P<rest>.*)|\s*)$")
            .expect("static regex")
    })
}

fn preliminary_title_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*PRELIMINARY\s+TITLE\s*$").expect("static regex"))
}

fn toc_entry_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // A table-of-contents line: a book/title/chapter heading followed by a
    // page number ("BOOK SEVEN – TRANSITORY AND FINAL PROVISIONS 120").
    RE.get_or_init(|| {
        Regex::new(
            r"^\s*(?:BOOK|Book|TITLE|Title|CHAPTER|Chapter|PRELIMINARY\s+TITLE)\b.*\S[ \t]+\d{1,3}\s*$",
        )
        .expect("static regex")
    })
}

fn is_structural_line(line: &str) -> bool {
    book_re().is_match(line)
        || title_re().is_match(line)
        || chapter_re().is_match(line)
        || article_re().is_match(line)
        || toc_entry_re().is_match(line)
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
/// Collect up to two such continuation lines starting at `idx`. The same
/// helper picks up wrapped names ("… ADMINISTRATIVE MACHINERY FOR" /
/// "ITS IMPLEMENTATION"). A line that is itself a book/title/chapter/article
/// header is never consumed.
fn caps_continuation(lines: &[&str], idx: usize) -> (String, usize) {
    let mut parts: Vec<&str> = Vec::new();
    let mut consumed = 0;
    for line in lines.iter().skip(idx).take(2) {
        let trimmed = line.trim();
        if trimmed.is_empty() || !is_caps_heading(trimmed) || is_structural_line(line) {
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

        // Table-of-contents entries repeat every header with a page number;
        // honoring them would leave the scan "inside" the last book listed.
        if toc_entry_re().is_match(line) {
            i += 1;
            continue;
        }

        // The body's own "PRELIMINARY TITLE" header resets any state left by
        // table-of-contents lines whose page number wrapped onto the next line.
        if preliminary_title_re().is_match(line) {
            flush(&mut current, &mut body_lines, &mut articles);
            book = LaborCodeBook::Preliminary;
            title_name = String::from("Preliminary Title");
            chapter = None;
            i += 1;
            continue;
        }

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
            // Only treat as a header when the name is inline or follows in
            // capitals; "Title to the property shall…" has neither.
            let (name, consumed) = header_name(&caps, &lines, i);
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
            let (name, consumed) = header_name(&caps, &lines, i);
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

/// A title/chapter header's name: the inline remainder plus any wrapped
/// capitalized continuation lines, with footnote tokens removed. Returns
/// the name and how many following lines it consumed.
fn header_name(caps: &regex::Captures<'_>, lines: &[&str], i: usize) -> (String, usize) {
    let inline = caps.name("rest").map_or("", |m| m.as_str().trim());
    let (continuation, consumed) = caps_continuation(lines, i + 1);
    let name = [inline, continuation.as_str()]
        .iter()
        .filter(|part| !part.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    (take_footnote_refs(&name).0, consumed)
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
fn build_article(
    raw: RawArticle,
    provenance: &Provenance,
    footnotes: &BTreeMap<u32, String>,
) -> LaborCodeArticle {
    // Resolve footnote tokens first so markers never leak into the heading
    // ("Security of Tenure.252") or the body.
    let (heading_rest, mut refs) = take_footnote_refs(&raw.heading_rest);
    let (body, body_refs) = take_footnote_refs(&raw.body);
    refs.extend(body_refs);
    let notes: Vec<Footnote> = refs
        .iter()
        .filter_map(|n| {
            footnotes.get(n).map(|text| Footnote {
                number: *n,
                text: text.clone(),
            })
        })
        .collect();

    // Heading style is "Security of Tenure. – In cases of regular
    // employment…": short title, dash, then the body starts inline.
    let (heading, inline_body) = split_article_heading(&heading_rest);

    let content = match (inline_body.is_empty(), body.is_empty()) {
        (false, false) => format!("{inline_body}\n{body}"),
        (false, true) => inline_body,
        (true, _) => body,
    };
    // Line-start indentation is layout, not content (footnote splitting,
    // the only consumer of it, has already run).
    let content = content
        .lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");

    // `content` is the full article body, enumerated items included;
    // `subsections` repeats the items one by one for fine-grained retrieval.
    let (_, subsections) = extract_subsections(&content);

    // Amendment history comes from inline "(As amended by …)" notes and,
    // in the DOLE edition, from the article's footnotes.
    let mut amendments = extract_amendments(&content);
    for note in &notes {
        for amendment in amendments_from_note(&first_sentence(&note.text)) {
            if !amendments.iter().any(|a| a.law == amendment.law) {
                amendments.push(amendment);
            }
        }
    }
    let original_pd_442 = !amendment_note_re()
        .captures_iter(&content)
        .any(|caps| is_insertion_note(&caps["note"]))
        && !notes.iter().any(|n| is_insertion_note(&n.text));
    let related_articles = extract_related_articles(&content, raw.number);
    let implementing_orders = extract_order_refs(notes.iter().map(|n| n.text.as_str()));

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
            implementing_orders,
            footnotes: notes,
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

/// Parse inline "(As amended by …)" notes into amendments.
fn extract_amendments(content: &str) -> Vec<Amendment> {
    let mut amendments: Vec<Amendment> = Vec::new();
    for caps in amendment_note_re().captures_iter(content) {
        let note = caps["note"]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for amendment in amendments_from_note(&note) {
            if !amendments.iter().any(|a| a.law == amendment.law) {
                amendments.push(amendment);
            }
        }
    }
    amendments
}

/// Turn one amendment note ("As amended by Sec. 34 of R.A. No. 6715
/// (1989).") into one amendment per cited law. Notes that do not start
/// with an amending verb yield nothing.
///
/// * PD 442 — the Labor Code itself, often named in an amending law's
///   title ("Amending Certain Sections of PD No. 442") — is never recorded
///   as its own amendment.
/// * The note's date is attached only when it cites a single law and gives
///   a full date; with several laws it is ambiguous which one it belongs to,
///   and a bare year ("(1989)") is not a date.
fn amendments_from_note(note: &str) -> Vec<Amendment> {
    if !starts_with_amending_verb(note) {
        return Vec::new();
    }

    let mut laws: Vec<String> = Vec::new();
    for law in law_citation_re().captures_iter(note) {
        let prefix = if law.name("ra").is_some() {
            "RA"
        } else if law.name("pd").is_some() {
            "PD"
        } else if law.name("bp").is_some() {
            "BP"
        } else {
            "EO"
        };
        let name = format!("{prefix} {}", law["num"].to_ascii_uppercase());
        if name != "PD 442" && !laws.contains(&name) {
            laws.push(name);
        }
    }

    let date = if laws.len() == 1 {
        parse_first_date(note).map(date_to_utc)
    } else {
        None
    };
    laws.into_iter()
        .map(|law| Amendment {
            law,
            date,
            description: Some(note.to_string()),
        })
        .collect()
}

/// Whether a note opens with "(As) amended/inserted/added/incorporated by".
fn starts_with_amending_verb(note: &str) -> bool {
    let lower = note.trim_start().to_ascii_lowercase();
    let verb = lower.strip_prefix("as ").unwrap_or(&lower).trim_start();
    ["amended", "inserted", "added", "incorporated"]
        .iter()
        .any(|v| verb.starts_with(v))
}

/// Single-letter initials ("R.A.", "E.O.") and these abbreviations end in a
/// period without ending the sentence.
const NOTE_ABBREVIATIONS: &[&str] = &[
    "No", "Nos", "Sec", "Secs", "Art", "Arts", "Par", "Pars", "Blg", "Rep", "Inc", "Co", "vs",
];

/// The first sentence of a footnote. Later sentences cite related laws
/// ("See also R.A. No. 10911") rather than amending ones.
fn first_sentence(text: &str) -> String {
    for (i, c) in text.char_indices() {
        if c != '.' {
            continue;
        }
        let followed_by_space = text[i + 1..].chars().next().is_none_or(char::is_whitespace);
        if !followed_by_space {
            continue;
        }
        let word = text[..i]
            .rsplit(|ch: char| !ch.is_alphanumeric())
            .next()
            .unwrap_or("");
        let is_initial = word.chars().count() == 1 && word.chars().all(char::is_alphabetic);
        let is_abbreviation = NOTE_ABBREVIATIONS
            .iter()
            .any(|a| a.eq_ignore_ascii_case(word));
        if !is_initial && !is_abbreviation {
            return text[..=i].to_string();
        }
    }
    text.to_string()
}

fn order_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // "D.O. No. 147-15", "D.O. No. 18-A-11", "D.O. No. 05 (1992)",
    // "Department Order No. 40, Series of 2003".
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:D\.\s*O\.|DO|Department\s+Order)\s*No\.?\s*(?P<num>\d{1,4})(?:-(?P<letter>[a-z]))?(?:\s*-\s*(?P<yy>\d{2})\b|,?\s*(?:series\s+of|s\.)\s*(?P<series>\d{4})|\s*\((?P<paren>\d{4})\))",
        )
        .expect("static regex")
    })
}

/// Department Orders cited in `texts`, normalized like `DoleOrder`
/// numbers (`DO-147-15`, `DO-18-A-11`; "No. 05 (1992)" → `DO-5-92`), in
/// first-appearance order.
fn extract_order_refs<'a>(texts: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut orders: Vec<String> = Vec::new();
    for text in texts {
        for caps in order_ref_re().captures_iter(text) {
            let Ok(num) = caps["num"].parse::<u32>() else {
                continue;
            };
            let yy = match (caps.name("yy"), caps.name("series").or(caps.name("paren"))) {
                (Some(yy), _) => yy.as_str().to_string(),
                (None, Some(year)) => year.as_str()[2..].to_string(),
                (None, None) => continue,
            };
            let id = match caps.name("letter") {
                Some(letter) => format!("DO-{num}-{}-{yy}", letter.as_str().to_ascii_uppercase()),
                None => format!("DO-{num}-{yy}"),
            };
            if !orders.contains(&id) {
                orders.push(id);
            }
        }
    }
    orders
}

// ---------------------------------------------------------------------------
// Footnotes
// ---------------------------------------------------------------------------

/// Footnote markers are replaced by `FOOTNOTE_OPEN n FOOTNOTE_CLOSE` tokens
/// (Unicode private-use characters, which never occur in the source text)
/// so they survive scanning and can be resolved per article.
const FOOTNOTE_OPEN: char = '\u{E000}';
const FOOTNOTE_CLOSE: char = '\u{E001}';

/// How far footnote numbering may jump (a footnote lost to a scanned page)
/// and still be followed.
const FOOTNOTE_GAP: u32 = 3;

fn footnote_start_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // A line opening with a bare number and text that starts like a
    // sentence or citation: "252 As amended by…", "  5 See also…",
    // "22 “Refer to…". Wrapped body text ("15 days after…") starts lowercase.
    RE.get_or_init(|| {
        Regex::new(r#"^\s*(?P<n>\d{1,3})[ \t]+(?P<text>[A-Z"\u{201c}(\[].*)$"#)
            .expect("static regex")
    })
}

/// Footnotes must be set this much wider than body text (median line
/// length) before line width is trusted to tell them apart.
const FOOTNOTE_WIDTH_RATIO: f64 = 1.3;

/// Lines shorter than this (paragraph tails such as "day; or") say nothing
/// about the font size and are left out of width statistics.
const MIN_MEASURED_LINE: usize = 20;

/// Separate editorial footnotes from the running text.
///
/// The DOLE edition prints footnotes at the foot of each page, each opening
/// with its number, so `units` should be the document's pages. Within a
/// page, the footnote area starts at the first line that opens a footnote
/// and runs to the end of the page: later lines either open the next
/// footnote or continue the current one (quoted statute text, "See also…"
/// paragraphs). A line opens a footnote only when its number is the next one
/// expected — footnote numbers run sequentially through the whole document,
/// allowing a gap of [`FOOTNOTE_GAP`] — and its text starts like a sentence,
/// so a body line that happens to begin with a number is left alone.
///
/// A long footnote can spill onto the next page, where its remainder sits
/// *above* that page's first numbered footnote with no number of its own.
/// Footnotes are set in a smaller font, so their lines are much wider
/// (≈155 vs ≈90 characters in the DOLE edition). A first pass learns both
/// widths; when they are clearly apart, multi-line paragraphs at that width
/// directly above a page's footnote area, after a page that ended inside a
/// footnote, are moved back into that footnote.
///
/// Without page boundaries, pass paragraphs as `units`; each paragraph is
/// then treated like a page.
///
/// Returns the running text without footnotes (units joined by newlines)
/// and the footnotes by number.
fn split_footnotes(units: &[&str]) -> (String, BTreeMap<u32, String>) {
    let (_, _, widths) = split_footnotes_with(units, None);
    let (body, footnotes, _) = split_footnotes_with(units, widths.threshold());
    (body, footnotes)
}

/// Line lengths seen in footnote text and in body text.
#[derive(Default)]
struct LineWidths {
    note: Vec<usize>,
    body: Vec<usize>,
}

impl LineWidths {
    fn record(target: &mut Vec<usize>, line: &str) {
        let len = line.trim().chars().count();
        if len >= MIN_MEASURED_LINE {
            target.push(len);
        }
    }

    /// Midpoint between the median footnote and body widths, when footnotes
    /// are clearly wider.
    fn threshold(&self) -> Option<usize> {
        let note = median(&self.note)?;
        let body = median(&self.body)?;
        ((note as f64) >= (body as f64) * FOOTNOTE_WIDTH_RATIO).then_some((note + body) / 2)
    }
}

fn median(values: &[usize]) -> Option<usize> {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied()
}

/// Whether a paragraph is set at footnote width. Single-line paragraphs
/// never qualify: the extractor sometimes emits a whole body sentence
/// unwrapped on one line.
fn is_wide_paragraph(lines: &[&str], threshold: usize) -> bool {
    let measured: Vec<usize> = lines
        .iter()
        .map(|l| l.trim().chars().count())
        .filter(|len| *len >= MIN_MEASURED_LINE)
        .collect();
    lines.len() >= 2 && median(&measured).is_some_and(|m| m >= threshold)
}

/// The footnote number a line opens, if it opens the next expected one.
fn footnote_start(line: &str, next: u32) -> Option<(u32, String)> {
    let caps = footnote_start_re().captures(line)?;
    let n = caps["n"].parse::<u32>().ok()?;
    (next..=next + FOOTNOTE_GAP)
        .contains(&n)
        .then(|| (n, caps["text"].to_string()))
}

fn split_footnotes_with(
    units: &[&str],
    spill_width: Option<usize>,
) -> (String, BTreeMap<u32, String>, LineWidths) {
    let mut body: Vec<String> = Vec::new();
    let mut footnotes: BTreeMap<u32, String> = BTreeMap::new();
    let mut widths = LineWidths::default();
    let mut next = 1u32;
    // The footnote the previous unit ended inside, if any.
    let mut open_note: Option<u32> = None;

    let append = |footnotes: &mut BTreeMap<u32, String>, n: u32, line: &str| {
        let text = normalize_whitespace(line);
        if !text.is_empty() {
            let note = footnotes.entry(n).or_default();
            if !note.is_empty() {
                note.push(' ');
            }
            note.push_str(&text);
        }
    };

    for unit in units {
        let lines: Vec<&str> = unit.lines().collect();
        let area_start = lines
            .iter()
            .position(|line| footnote_start(line, next).is_some())
            .unwrap_or(lines.len());

        // Move a spilled footnote remainder (wide paragraphs right above the
        // footnote area) back into the footnote it belongs to.
        let mut body_end = area_start;
        if let (Some(threshold), Some(n)) = (spill_width, open_note) {
            loop {
                let before = &lines[..body_end];
                let Some(last_text) = before.iter().rposition(|l| !l.trim().is_empty()) else {
                    break;
                };
                let para_start = before[..last_text]
                    .iter()
                    .rposition(|l| l.trim().is_empty())
                    .map_or(0, |i| i + 1);
                if !is_wide_paragraph(&before[para_start..=last_text], threshold) {
                    break;
                }
                body_end = para_start;
            }
            for line in &lines[body_end..area_start] {
                append(&mut footnotes, n, line);
            }
        }

        let mut current: Option<u32> = None;
        for line in &lines[area_start..] {
            if let Some((n, text)) = footnote_start(line, next) {
                LineWidths::record(&mut widths.note, &text);
                footnotes.insert(n, normalize_whitespace(&text));
                next = n + 1;
                current = Some(n);
            } else if let Some(n) = current {
                LineWidths::record(&mut widths.note, line);
                append(&mut footnotes, n, line);
            }
        }

        for line in &lines[..body_end] {
            LineWidths::record(&mut widths.body, line);
        }
        let unit_body = lines[..body_end].join("\n");
        if !unit_body.trim().is_empty() {
            body.push(unit_body);
        }

        // A unit with its own footnote area ends inside its last footnote; a
        // unit without one leaves any spill state unchanged only if it was
        // entirely spill, otherwise the spill is over.
        open_note = match current {
            Some(n) => Some(n),
            None if body_end == 0 && area_start > 0 => open_note,
            None => None,
        };
    }

    (body.join("\n"), footnotes, widths)
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn footnote_marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // A number glued to the preceding word or punctuation ("promulgation.1",
    // "Policy.2 –", "TENANTS4", "Tenure.252"), or alone on a line where a
    // marker wrapped below its heading.
    RE.get_or_init(|| {
        Regex::new(
            r"(?:(?P<pre>[A-Za-z.,;:)\]\u{201d}\u{2019}])(?P<n>\d{1,3})\b|(?m:^)(?P<alone>\d{1,3})(?m:$))",
        )
        .expect("static regex")
    })
}

/// Replace inline footnote markers with tokens. Markers, like footnotes,
/// appear in ascending order, so a candidate is accepted only when it names
/// an existing footnote and continues the sequence (within
/// [`FOOTNOTE_GAP`]); a glued number that breaks the sequence ("Sec.2"
/// late in the text) is left untouched.
fn mark_footnote_refs(text: &str, footnotes: &BTreeMap<u32, String>) -> String {
    let mut expected = 1u32;
    footnote_marker_re()
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let whole = caps[0].to_string();
            let number = caps.name("n").or(caps.name("alone"));
            let Some(Ok(n)) = number.map(|m| m.as_str().parse::<u32>()) else {
                return whole;
            };
            if footnotes.contains_key(&n) && (expected..=expected + FOOTNOTE_GAP).contains(&n) {
                expected = n + 1;
                let pre = caps.name("pre").map_or("", |m| m.as_str());
                format!("{pre}{FOOTNOTE_OPEN}{n}{FOOTNOTE_CLOSE}")
            } else {
                whole
            }
        })
        .into_owned()
}

fn footnote_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!("{FOOTNOTE_OPEN}(\\d+){FOOTNOTE_CLOSE}")).expect("static regex")
    })
}

/// Remove footnote tokens from `text`, returning the cleaned text and the
/// referenced footnote numbers in order.
fn take_footnote_refs(text: &str) -> (String, Vec<u32>) {
    let refs = footnote_token_re()
        .captures_iter(text)
        .filter_map(|caps| caps[1].parse().ok())
        .collect();
    (footnote_token_re().replace_all(text, "").into_owned(), refs)
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
        build_article(raw, &Provenance::new("0".repeat(64)), &BTreeMap::new())
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

    fn footnote_map(entries: &[(u32, &str)]) -> BTreeMap<u32, String> {
        entries.iter().map(|(n, t)| (*n, t.to_string())).collect()
    }

    #[test]
    fn test_title_case_book_headers_and_toc_skipped() {
        let text = "Table of Contents\n\
                    BOOK ONE - PRE-EMPLOYMENT 4\n\
                    BOOK SEVEN – TRANSITORY AND FINAL PROVISIONS 120\n\n\
                    PRELIMINARY TITLE\n\n\
                    ART. 1. Name of Decree. – This decree shall be known as the Labor Code.\n\n\
                    Book One - PRE-EMPLOYMENT\n\n\
                    ART. 12. Statement of Objectives. – It is the policy of the State.\n";
        let articles = scan_articles(text);
        assert_eq!(book_of(&articles, 1), LaborCodeBook::Preliminary);
        assert_eq!(book_of(&articles, 12), LaborCodeBook::BookI);
    }

    #[test]
    fn test_preliminary_title_resets_wrapped_toc_title() {
        // A TOC entry whose page number wrapped to the next line looks like
        // a real title header; the body's PRELIMINARY TITLE must reset it.
        let text = "TITLE VIII – STRIKES AND LOCKOUTS AND FOREIGN\n\
                    INVOLVEMENT IN TRADE UNION ACTIVITIES 112\n\n\
                    PRELIMINARY TITLE\n\n\
                    ART. 1. Name of Decree. – This decree shall be known as the Labor Code.\n";
        let articles = scan_articles(text);
        assert_eq!(articles[0].title_name, "Preliminary Title");
        assert_eq!(articles[0].book, LaborCodeBook::Preliminary);
    }

    #[test]
    fn test_book_and_chapter_mentions_in_prose_are_not_headers() {
        let text = "BOOK THREE\nCONDITIONS OF EMPLOYMENT\n\
                    Chapter I – HOURS OF WORK\n\
                    ART. 82. Coverage. – The provisions of this Title shall apply\n\
                    Book IV of the Omnibus Rules applies here.\n\
                    Chapter II, of this Title, governs rest days.\n\
                    ART. 83. Normal Hours of Work. – Eight hours.\n";
        let articles = scan_articles(text);
        assert_eq!(book_of(&articles, 83), LaborCodeBook::BookIII);
        let normal_hours = articles.iter().find(|a| a.number == 83).unwrap();
        assert_eq!(
            normal_hours.chapter.as_deref(),
            Some("Chapter I: HOURS OF WORK")
        );
    }

    #[test]
    fn test_footnote_area_runs_to_end_of_page() {
        let pages = [
            "ART. 294. Security of Tenure.1 – In cases of regular employment.\n\n\
             1 As amended by Sec. 34 of R.A. No. 6715 (1989).\n\n\
             See also R.A. No. 10911 (2016).",
            "5 hectares shall be retained by the landowner.\n\n\
             2 Refer to D.O. No. 147-15 (2015).",
        ];
        let (body, notes) = split_footnotes(&pages);
        assert_eq!(
            notes.get(&1).map(String::as_str),
            Some("As amended by Sec. 34 of R.A. No. 6715 (1989). See also R.A. No. 10911 (2016).")
        );
        assert_eq!(
            notes.get(&2).map(String::as_str),
            Some("Refer to D.O. No. 147-15 (2015).")
        );
        // A lowercase line opening with a number is body text, not footnote 2.
        assert!(body.contains("5 hectares shall be retained"));
        assert!(!body.contains("As amended"));
    }

    #[test]
    fn test_out_of_sequence_number_is_not_a_footnote() {
        let pages = ["Body text.\n\n12 Months after effectivity the rules apply."];
        let (body, notes) = split_footnotes(&pages);
        assert!(notes.is_empty());
        assert!(body.contains("12 Months after"));
    }

    #[test]
    fn test_spilled_footnote_is_moved_back_by_width() {
        let narrow = "Body text wraps at a narrow width here.";
        let wide = "Footnote text is set in a smaller font so each of its lines runs far wider than any body line on the page.";
        let page1 = format!("{narrow}\n{narrow}\n{narrow}\n\n1 {wide}\n{wide}\n{wide}");
        let page2 = format!(
            "{narrow}\n{narrow}\n{narrow}\n\n(a) {wide}\n(b) {wide}\n\n2 See {wide}\n{wide}"
        );
        let (body, notes) = split_footnotes(&[page1.as_str(), page2.as_str()]);
        assert!(
            !body.contains("(a) Footnote"),
            "spill leaked into body: {body}"
        );
        assert!(notes[&1].contains("(a) Footnote") && notes[&1].contains("(b) Footnote"));
        assert!(notes[&2].starts_with("See Footnote"));
        assert_eq!(body.matches(narrow).count(), 6);
    }

    #[test]
    fn test_markers_follow_footnote_sequence() {
        let notes = footnote_map(&[(1, "a"), (2, "b"), (9, "c")]);
        let marked = mark_footnote_refs(
            "Security of Tenure.1 – text\nTENANTS2\nSec.9 out of order",
            &notes,
        );
        let (clean, refs) = take_footnote_refs(&marked);
        assert_eq!(refs, [1, 2]);
        assert_eq!(
            clean,
            "Security of Tenure. – text\nTENANTS\nSec.9 out of order"
        );
    }

    #[test]
    fn test_standalone_marker_line() {
        let notes = footnote_map(&[(1, "a")]);
        let marked = mark_footnote_refs("OF EMPLOYEES\n1\n\nNext", &notes);
        assert_eq!(
            take_footnote_refs(&marked),
            ("OF EMPLOYEES\n\n\nNext".to_string(), vec![1])
        );
    }

    #[test]
    fn test_footnotes_drive_article_metadata() {
        let notes = footnote_map(&[
            (
                1,
                "As amended by P.D. No. 570-A, “Amending Certain Sections of PD No. 442,” \
                 (1974). See also R.A. No. 10911 (2016).",
            ),
            (
                2,
                "Refer to D.O. No. 147-15 (2015), Amending the Rules of Book VI.",
            ),
        ]);
        let text = "Art. 297. [282] Termination by Employer.1 – An employer may \
                    terminate an employment.2\n";
        let marked = mark_footnote_refs(text, &notes);
        let article = build_article(
            scan_articles(&marked).remove(0),
            &Provenance::new("0".repeat(64)),
            &notes,
        );
        assert_eq!(article.heading.as_deref(), Some("Termination by Employer"));
        assert!(!article.content.contains('2'));
        let numbers: Vec<u32> = article
            .metadata
            .footnotes
            .iter()
            .map(|f| f.number)
            .collect();
        assert_eq!(numbers, [1, 2]);
        // PD 442 (the Code itself) and the "See also" sentence are excluded.
        let laws: Vec<&str> = article
            .metadata
            .amendments
            .iter()
            .map(|a| a.law.as_str())
            .collect();
        assert_eq!(laws, ["PD 570-A"]);
        assert_eq!(article.metadata.implementing_orders, ["DO-147-15"]);
    }

    #[test]
    fn test_inserted_by_footnote_marks_article_as_added() {
        let notes = footnote_map(&[(1, "As added by Sec. 9 of P.D. No. 1368 (1978).")]);
        let marked = mark_footnote_refs("Art. 214. Liability.1 – Text.\n", &notes);
        let article = build_article(
            scan_articles(&marked).remove(0),
            &Provenance::new("0".repeat(64)),
            &notes,
        );
        assert!(!article.metadata.original_pd_442);
        assert_eq!(article.metadata.amendments[0].law, "PD 1368");
    }

    #[test]
    fn test_first_sentence_skips_citation_abbreviations() {
        assert_eq!(
            first_sentence("As amended by Sec. 34 of R.A. No. 6715 (1989). See also E.O. No. 1."),
            "As amended by Sec. 34 of R.A. No. 6715 (1989)."
        );
    }

    #[test]
    fn test_order_refs_are_normalized() {
        let refs = extract_order_refs(
            [
                "See D.O. No. 05 (1992) and D.O. No. 18-A-11.",
                "Department Order No. 40, Series of 2003; D.O. No. 147-15 (2015).",
            ]
            .into_iter(),
        );
        assert_eq!(refs, ["DO-5-92", "DO-18-A-11", "DO-40-03", "DO-147-15"]);
    }
}
