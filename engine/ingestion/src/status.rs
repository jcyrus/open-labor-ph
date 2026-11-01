//! Reviewed legal status of Labor Code articles
//! (`data/labor_code_status.toml`).
//!
//! The source edition prints an article's old text and says in an editorial
//! footnote that it no longer applies, often on a Title or Chapter heading.
//! Keywords cannot read those notes safely: "superseded" may describe another
//! instrument, or a former article printed for reference. So each note is
//! reviewed by hand, here, and the file is tied to one PDF by its hash.
//!
//! [`StatusReview::apply`] refuses any footnote that looks like a status note
//! and has not been reviewed, so a parser change that surfaces new notes
//! fails the build of the dataset instead of labelling an article
//! `no_change_noted`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;

use crate::types::{LaborCodeArticle, LegalChange, LegalEffect, LegalStatus, LegalStatusKind};

/// Review file location relative to the repository root.
pub const DEFAULT_STATUS_PATH: &str = "data/labor_code_status.toml";

/// One note saying that articles no longer apply as printed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedChange {
    /// Article or inclusive range: "131" or "139-150".
    pub articles: String,

    /// Footnote stating the change; must be on each listed article.
    pub footnote: u32,

    /// How the note describes the change.
    pub effect: LegalEffect,

    /// Law or decision that made the change.
    pub by: String,

    /// The part affected. Omitted when the note covers the whole article.
    #[serde(default)]
    pub part: Option<String>,
}

/// A footnote that reads like a status note but does not change the article.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedNote {
    /// Footnote number.
    pub footnote: u32,

    /// Why the article's status is unaffected.
    pub reason: String,
}

/// The parsed review file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusReview {
    /// SHA-256 of the PDF whose footnotes were reviewed.
    pub source_sha256: String,

    #[serde(default, rename = "change")]
    pub changes: Vec<ReviewedChange>,

    #[serde(default, rename = "no_change")]
    pub no_changes: Vec<ReviewedNote>,
}

fn status_note_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)supersed|repeal|inoperative|obsolete|unconstitutional|abolish|no longer|now being undertaken|been transferred|modified by|expired|been replaced|replaced by|has affected|expanded by|qualified by|deemed|now refers|shall refer|absorb",
        )
        .expect("static regex")
    })
}

/// Parse "131" or "139-150" into an inclusive range.
fn parse_articles(raw: &str) -> Result<(u32, u32)> {
    let (first, last) = raw.split_once('-').unwrap_or((raw, raw));
    let first: u32 = first
        .trim()
        .parse()
        .with_context(|| format!("invalid articles {raw:?}"))?;
    let last: u32 = last
        .trim()
        .parse()
        .with_context(|| format!("invalid articles {raw:?}"))?;
    if first > last {
        bail!("invalid articles {raw:?}: range is reversed");
    }
    Ok((first, last))
}

impl StatusReview {
    /// Read a review file.
    pub fn load(path: &Path) -> Result<StatusReview> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
    }

    /// Set `legal_status` on every article. Every problem is reported at
    /// once so the review can be corrected in one pass.
    pub fn apply(&self, articles: &mut [LaborCodeArticle]) -> Result<()> {
        let mut problems: Vec<String> = Vec::new();
        let mut by_article: BTreeMap<u32, Vec<LegalChange>> = BTreeMap::new();
        let on_article = |article: &LaborCodeArticle, number: u32| {
            let meta = &article.metadata;
            meta.footnotes.iter().any(|f| f.number == number)
                || meta.heading_footnotes.iter().any(|f| f.number == number)
        };

        for change in &self.changes {
            let (first, last) = parse_articles(&change.articles)?;
            for number in first..=last {
                match articles.iter().find(|a| a.article_number == number) {
                    None => {
                        problems.push(format!("footnote {}: no Article {number}", change.footnote))
                    }
                    Some(article) if !on_article(article, change.footnote) => {
                        problems.push(format!(
                            "footnote {} is not on Article {number}",
                            change.footnote
                        ));
                    }
                    Some(_) => by_article.entry(number).or_default().push(LegalChange {
                        effect: change.effect,
                        part: change.part.clone(),
                        by: change.by.clone(),
                        footnote: change.footnote,
                    }),
                }
            }
        }

        let changed: BTreeSet<u32> = self.changes.iter().map(|c| c.footnote).collect();
        let unaffected: BTreeSet<u32> = self.no_changes.iter().map(|n| n.footnote).collect();
        for number in changed.intersection(&unaffected) {
            problems.push(format!(
                "footnote {number} is listed as both a change and no change"
            ));
        }

        let mut seen: BTreeSet<u32> = BTreeSet::new();
        for article in articles.iter() {
            let meta = &article.metadata;
            let texts = meta
                .footnotes
                .iter()
                .map(|f| (f.number, f.text.as_str()))
                .chain(
                    meta.heading_footnotes
                        .iter()
                        .map(|f| (f.number, f.text.as_str())),
                );
            for (number, text) in texts {
                seen.insert(number);
                if status_note_re().is_match(text)
                    && !changed.contains(&number)
                    && !unaffected.contains(&number)
                {
                    problems.push(format!(
                        "footnote {number} on Article {} reads like a status note and has not \
                         been reviewed",
                        article.article_number
                    ));
                }
            }
        }
        for number in unaffected.difference(&seen) {
            problems.push(format!("footnote {number} is not on any article"));
        }

        if !problems.is_empty() {
            problems.dedup();
            bail!("status review is out of date: {}", problems.join("; "));
        }

        for article in articles.iter_mut() {
            let changes = by_article
                .remove(&article.article_number)
                .unwrap_or_default();
            let status = if changes.is_empty() {
                LegalStatusKind::NoChangeNoted
            } else if changes.iter().any(|c| c.part.is_none()) {
                LegalStatusKind::Superseded
            } else {
                LegalStatusKind::PartlySuperseded
            };
            article.metadata.legal_status = Some(LegalStatus { status, changes });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Footnote, HeadingFootnote, LaborCodeBook, LaborCodeMetadata, Provenance};

    fn article(
        number: u32,
        footnotes: &[(u32, &str)],
        heading: &[(u32, &str)],
    ) -> LaborCodeArticle {
        LaborCodeArticle {
            article_number: number,
            former_article_number: None,
            book: LaborCodeBook::BookIII,
            title_name: "Title III".to_string(),
            chapter: None,
            heading: None,
            content: "Text.".to_string(),
            metadata: LaborCodeMetadata {
                footnotes: footnotes
                    .iter()
                    .map(|(number, text)| Footnote {
                        number: *number,
                        text: text.to_string(),
                    })
                    .collect(),
                heading_footnotes: heading
                    .iter()
                    .map(|(number, text)| HeadingFootnote {
                        heading: "Chapter III".to_string(),
                        number: *number,
                        text: text.to_string(),
                    })
                    .collect(),
                ..Default::default()
            },
            provenance: Provenance::new("0".repeat(64)),
            subsections: Vec::new(),
        }
    }

    fn review(body: &str) -> StatusReview {
        toml::from_str(&format!("source_sha256 = \"{}\"\n{body}", "0".repeat(64))).unwrap()
    }

    fn status_of(article: &LaborCodeArticle) -> LegalStatusKind {
        article.metadata.legal_status.as_ref().unwrap().status
    }

    #[test]
    fn test_whole_part_and_unchanged_articles() {
        let mut articles = vec![
            article(1, &[(1, "Superseded by R.A. No. 11210.")], &[]),
            article(2, &[(2, "The second paragraph was repealed.")], &[]),
            article(3, &[], &[]),
        ];
        review(
            r#"
            [[change]]
            articles = "1"
            footnote = 1
            effect = "superseded"
            by = "RA 11210"

            [[change]]
            articles = "2"
            footnote = 2
            effect = "repealed"
            by = "EO 111"
            part = "second paragraph"
            "#,
        )
        .apply(&mut articles)
        .unwrap();

        assert_eq!(status_of(&articles[0]), LegalStatusKind::Superseded);
        assert_eq!(status_of(&articles[1]), LegalStatusKind::PartlySuperseded);
        assert_eq!(status_of(&articles[2]), LegalStatusKind::NoChangeNoted);
    }

    #[test]
    fn test_heading_footnote_covers_a_range() {
        let note = [(9, "All articles of this Chapter are expressly repealed.")];
        let mut articles = vec![article(5, &[], &note), article(6, &[], &note)];
        review(
            r#"
            [[change]]
            articles = "5-6"
            footnote = 9
            effect = "repealed"
            by = "RA 10361"
            "#,
        )
        .apply(&mut articles)
        .unwrap();
        assert!(articles
            .iter()
            .all(|a| status_of(a) == LegalStatusKind::Superseded));
    }

    #[test]
    fn test_unreviewed_status_note_is_refused() {
        let mut articles = vec![article(1, &[(1, "Superseded by R.A. No. 11210.")], &[])];
        let err = review("").apply(&mut articles).unwrap_err().to_string();
        assert!(err.contains("footnote 1"), "{err}");
        assert!(articles[0].metadata.legal_status.is_none());
    }

    #[test]
    fn test_reviewed_note_leaves_status_unchanged() {
        let mut articles = vec![article(
            1,
            &[(1, "D.O. 18-A was superseded by D.O. 174.")],
            &[],
        )];
        review(
            r#"
            [[no_change]]
            footnote = 1
            reason = "Concerns another issuance."
            "#,
        )
        .apply(&mut articles)
        .unwrap();
        assert_eq!(status_of(&articles[0]), LegalStatusKind::NoChangeNoted);
    }

    #[test]
    fn test_review_problems_are_all_reported() {
        let mut articles = vec![article(1, &[(1, "Text.")], &[])];
        let err = review(
            r#"
            [[change]]
            articles = "1-2"
            footnote = 7
            effect = "repealed"
            by = "RA 1"

            [[no_change]]
            footnote = 7
            reason = "Listed twice."
            "#,
        )
        .apply(&mut articles)
        .unwrap_err()
        .to_string();
        for expected in [
            "footnote 7 is not on Article 1",
            "no Article 2",
            "both a change and no change",
            "footnote 7 is not on any article",
        ] {
            assert!(err.contains(expected), "missing {expected:?} in {err}");
        }
    }

    #[test]
    fn test_parse_articles() {
        assert_eq!(parse_articles("131").unwrap(), (131, 131));
        assert_eq!(parse_articles("139-150").unwrap(), (139, 150));
        assert!(parse_articles("150-139").is_err());
        assert!(parse_articles("x").is_err());
    }
}
