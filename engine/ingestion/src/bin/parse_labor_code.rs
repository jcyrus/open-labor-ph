//! Labor Code of the Philippines (PD 442) PDF parser.
//!
//! Reads a Labor Code PDF and emits a JSON **array** of `LaborCodeArticle`
//! objects, assigning each article to its Book/Title/Chapter from the
//! running context headers ("BOOK THREE", "Title I", "Chapter II").
//!
//! Usage:
//! ```text
//! parse-labor-code <input.pdf> <output.json>
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use regex::Regex;
use tracing::{info, warn};

use labor_ingestion::parser::{self, extract_subsections};
use labor_ingestion::types::{LaborCodeArticle, LaborCodeBook, LaborCodeMetadata};

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

fn parse_args() -> Result<(PathBuf, PathBuf)> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 || args.iter().any(|a| a == "-h" || a == "--help") {
        bail!("Usage: parse-labor-code <input.pdf> <output.json>");
    }
    Ok((PathBuf::from(&args[0]), PathBuf::from(&args[1])))
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
    let (input, output) = parse_args()?;

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

    // Article construction (subsection carving, heading parsing) is
    // independent per article, so it parallelizes cleanly with rayon.
    let mut articles: Vec<LaborCodeArticle> =
        raw_articles.into_par_iter().map(build_article).collect();

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
    RE.get_or_init(|| {
        Regex::new(r"^\s*BOOK\s+(?P<num>ONE|TWO|THREE|FOUR|FIVE|SIX|VI|IV|V|III|II|I|[1-6])\b")
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

    // Articles 1–6 sit in the Preliminary Title, before "BOOK ONE". The
    // schema's book enum has no "Preliminary" variant, so they are assigned
    // to Book I with title_name "Preliminary Title" — the title_name keeps
    // the legal position recoverable.
    let mut book = LaborCodeBook::BookI;
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

/// Convert a delimited raw article into the typed domain struct.
fn build_article(raw: RawArticle) -> LaborCodeArticle {
    // Heading style is "Security of Tenure. – In cases of regular
    // employment…": short title, dash, then the body starts inline.
    let (heading, inline_body) = split_article_heading(&raw.heading_rest);

    let content = match (inline_body.is_empty(), raw.body.is_empty()) {
        (false, false) => format!("{inline_body}\n{}", raw.body),
        (false, true) => inline_body,
        (true, _) => raw.body.clone(),
    };

    let (intro, subsections) = extract_subsections(&content);
    // Keep the full body in `content` when there is no intro text, so the
    // article is never hollow; subsections always carry the item breakdown.
    let content = if intro.is_empty() { content } else { intro };

    // The renumbered edition's original article number is preserved as a
    // tag ("formerly_art_279") — the schema has no dedicated field for it,
    // and burying it in free text would make it unqueryable.
    let mut tags = Vec::new();
    if let Some(orig) = raw.original_number {
        tags.push(format!("formerly_art_{orig}"));
    }

    LaborCodeArticle {
        article_number: raw.number,
        book: raw.book,
        title_name: raw.title_name,
        chapter: raw.chapter,
        heading,
        content,
        metadata: LaborCodeMetadata {
            original_pd_442: true,
            amendments: Vec::new(),
            related_articles: Vec::new(),
            implementing_orders: Vec::new(),
            tags,
        },
        subsections,
    }
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
