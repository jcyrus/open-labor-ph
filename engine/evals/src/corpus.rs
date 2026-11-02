//! The processed dataset as a flat list of citable passages.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{bail, Context, Result};
use labor_ingestion::{DoleOrder, LaborCodeArticle, LegalStatusKind};
use serde::de::DeserializeOwned;

/// Where a passage comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    LaborCode { article: u32, former: Option<u32> },
    Order { number: String },
}

/// One article or section.
#[derive(Debug, Clone)]
pub struct Passage {
    /// Citation: "Labor Code Art. 297" or "DO-147-15 Rule I-A, Section 5".
    pub id: String,
    pub source: Source,
    /// Article or section heading, prefixed for orders with the order title.
    pub title: String,
    pub body: String,
    /// The article's own editorial footnotes.
    pub footnotes: String,
    /// Footnotes printed on the Title or Chapter heading above the article.
    pub heading_notes: String,
    /// Warning that the text no longer applies as printed.
    pub status_note: Option<String>,
}

impl Passage {
    /// Text a citation can be checked against.
    pub fn full_text(&self) -> String {
        [
            self.body.as_str(),
            self.footnotes.as_str(),
            self.heading_notes.as_str(),
            self.status_note.as_deref().unwrap_or(""),
        ]
        .join("\n")
    }

    /// Text the retriever indexes. Heading notes are left out because each
    /// is repeated on every article under its heading.
    pub fn index_text(&self) -> String {
        [
            self.title.as_str(),
            self.body.as_str(),
            self.footnotes.as_str(),
            self.status_note.as_deref().unwrap_or(""),
        ]
        .join("\n")
    }
}

/// Every passage in `data/processed`.
#[derive(Debug, Clone)]
pub struct Corpus {
    pub passages: Vec<Passage>,
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn article_status(article: &LaborCodeArticle) -> Option<String> {
    let status = article.metadata.legal_status.as_ref()?;
    if status.status == LegalStatusKind::NoChangeNoted {
        return None;
    }
    let changes: Vec<String> = status
        .changes
        .iter()
        .map(|change| {
            let effect = serde_json::to_value(change.effect)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
                .unwrap_or_default();
            match &change.part {
                Some(part) => format!("{part}: {effect} by {}", change.by),
                None => format!("{effect} by {}", change.by),
            }
        })
        .collect();
    Some(format!(
        "This text no longer applies as printed ({}).",
        changes.join("; ")
    ))
}

fn article_passage(article: &LaborCodeArticle) -> Passage {
    let meta = &article.metadata;
    let join = |texts: Vec<&str>| texts.join("\n");
    Passage {
        id: format!("Labor Code Art. {}", article.article_number),
        source: Source::LaborCode {
            article: article.article_number,
            former: article.former_article_number,
        },
        title: article.heading.clone().unwrap_or_default(),
        body: article.content.clone(),
        footnotes: join(meta.footnotes.iter().map(|f| f.text.as_str()).collect()),
        heading_notes: join(
            meta.heading_footnotes
                .iter()
                .map(|f| f.text.as_str())
                .collect(),
        ),
        status_note: article_status(article),
    }
}

fn order_passages(order: &DoleOrder) -> Vec<Passage> {
    order
        .sections
        .iter()
        .filter(|section| !section.content.trim().is_empty())
        .map(|section| Passage {
            id: format!("{} {}", order.order_number, section.section_number),
            source: Source::Order {
                number: order.order_number.clone(),
            },
            title: match &section.title {
                Some(title) => format!("{}: {title}", order.title),
                None => order.title.clone(),
            },
            body: section.content.clone(),
            footnotes: String::new(),
            heading_notes: String::new(),
            status_note: order
                .metadata
                .superseded_by
                .as_ref()
                .map(|by| format!("This order was superseded by {by}.")),
        })
        .collect()
}

impl Corpus {
    /// Load `labor_code.json` and every `DO-*.json` from a directory.
    pub fn load(dir: &Path) -> Result<Corpus> {
        let articles: Vec<LaborCodeArticle> = read_json(&dir.join("labor_code.json"))?;
        let mut passages: Vec<Passage> = articles.iter().map(article_passage).collect();

        let mut orders: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("failed to read {}", dir.display()))?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("DO-") && name.ends_with(".json"))
            })
            .collect();
        orders.sort();
        for path in orders {
            passages.extend(order_passages(&read_json::<DoleOrder>(&path)?));
        }

        Self::from_passages(passages)
    }

    /// Build a corpus, refusing duplicate citations.
    pub fn from_passages(passages: Vec<Passage>) -> Result<Corpus> {
        let mut seen = HashSet::new();
        for passage in &passages {
            if !seen.insert(passage.id.as_str()) {
                bail!("duplicate passage {:?}", passage.id);
            }
        }
        Ok(Corpus { passages })
    }

    pub fn get(&self, id: &str) -> Option<&Passage> {
        self.passages.iter().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dataset() -> Corpus {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/processed");
        Corpus::load(&root).unwrap()
    }

    #[test]
    fn test_dataset_loads_with_status_warnings() {
        let corpus = dataset();
        assert!(corpus.passages.len() > 317);
        let maternity = corpus.get("Labor Code Art. 131").unwrap();
        assert!(maternity.status_note.as_ref().unwrap().contains("RA 11210"));
        assert!(maternity.full_text().contains("One Hundred Five"));
        assert!(corpus
            .get("Labor Code Art. 83")
            .unwrap()
            .status_note
            .is_none());
        let osh = corpus.get("DO-198-18 Section 6").unwrap();
        assert!(osh.status_note.as_ref().unwrap().contains("DO-252-25"));
    }

    #[test]
    fn test_duplicate_citation_is_refused() {
        let passage = dataset().passages[0].clone();
        assert!(Corpus::from_passages(vec![passage.clone(), passage]).is_err());
    }
}
