//! BM25 baseline retriever and its recall metrics.
//!
//! BM25 needs no model or API key, so the same questions rank the same
//! passages on every machine and CI can pin the result.

use std::collections::{BTreeMap, HashMap, HashSet};

use labor_ingestion::BenchmarkQuestion;
use serde::Serialize;

use crate::corpus::{Corpus, Passage};

const K1: f64 = 1.2;
const B: f64 = 0.75;
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from", "has",
    "have", "how", "if", "in", "is", "it", "its", "may", "must", "of", "on", "or", "shall", "that",
    "the", "their", "there", "this", "to", "under", "what", "when", "which", "who", "will", "with",
];

/// Cut-offs reported in the benchmark.
pub const CUTOFFS: [usize; 4] = [1, 3, 5, 10];

fn stem(token: &str) -> &str {
    match token.strip_suffix('s') {
        Some(stem) if stem.len() > 2 && !stem.ends_with('s') => stem,
        _ => token,
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty() && !STOPWORDS.contains(token))
        .map(|token| stem(token).to_string())
        .collect()
}

/// A BM25 index over a corpus.
pub struct Bm25<'a> {
    corpus: &'a Corpus,
    term_counts: Vec<HashMap<String, u32>>,
    lengths: Vec<f64>,
    average_length: f64,
    document_frequency: HashMap<String, u32>,
}

impl<'a> Bm25<'a> {
    pub fn new(corpus: &'a Corpus) -> Bm25<'a> {
        let mut term_counts = Vec::with_capacity(corpus.passages.len());
        let mut lengths = Vec::with_capacity(corpus.passages.len());
        let mut document_frequency: HashMap<String, u32> = HashMap::new();
        for passage in &corpus.passages {
            let tokens = tokenize(&passage.index_text());
            lengths.push(tokens.len() as f64);
            let mut counts: HashMap<String, u32> = HashMap::new();
            for token in tokens {
                *counts.entry(token).or_default() += 1;
            }
            for term in counts.keys() {
                *document_frequency.entry(term.clone()).or_default() += 1;
            }
            term_counts.push(counts);
        }
        let average_length = lengths.iter().sum::<f64>() / lengths.len().max(1) as f64;
        Bm25 {
            corpus,
            term_counts,
            lengths,
            average_length,
            document_frequency,
        }
    }

    /// The `k` best passages, best first. Ties go to the lower citation so
    /// the order never depends on hashing.
    pub fn search(&self, query: &str, k: usize) -> Vec<&'a Passage> {
        let total = self.corpus.passages.len() as f64;
        let terms: HashSet<String> = tokenize(query).into_iter().collect();
        let mut scored: Vec<(f64, &'a Passage)> = Vec::with_capacity(self.term_counts.len());
        for (index, passage) in self.corpus.passages.iter().enumerate() {
            let length_ratio = self.lengths[index] / self.average_length;
            let mut score = 0.0;
            for term in &terms {
                let Some(&count) = self.term_counts[index].get(term) else {
                    continue;
                };
                let df = f64::from(self.document_frequency[term]);
                let idf = ((total - df + 0.5) / (df + 0.5) + 1.0).ln();
                let tf = f64::from(count);
                score += idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * length_ratio));
            }
            if score > 0.0 {
                scored.push((score, passage));
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
        scored.truncate(k);
        scored.into_iter().map(|(_, passage)| passage).collect()
    }
}

/// How one question fared.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QuestionRetrieval {
    pub id: String,
    pub citations: Vec<String>,
    /// 1-based rank of the first cited passage in the top 10.
    pub first_hit_rank: Option<usize>,
    pub recall_at_5: f64,
}

/// Metrics for a whole benchmark.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RetrievalReport {
    pub retriever: String,
    pub passages: usize,
    pub questions: usize,
    /// Mean share of a question's citations found in the top k.
    pub recall_at: BTreeMap<String, f64>,
    /// Mean reciprocal rank of the first cited passage, within the top 10.
    pub mrr: f64,
    pub per_question: Vec<QuestionRetrieval>,
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// Measure a retriever-independent ranking of every question.
pub fn evaluate(corpus: &Corpus, questions: &[BenchmarkQuestion]) -> RetrievalReport {
    let index = Bm25::new(corpus);
    let deepest = CUTOFFS[CUTOFFS.len() - 1];
    let mut sums = [0.0; CUTOFFS.len()];
    let mut reciprocal = 0.0;
    let mut per_question = Vec::with_capacity(questions.len());

    for question in questions {
        let ranked: Vec<&str> = index
            .search(&question.question, deepest)
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        let recall = |k: usize| {
            let top = &ranked[..k.min(ranked.len())];
            let found = question
                .citations
                .iter()
                .filter(|c| top.contains(&c.as_str()))
                .count();
            found as f64 / question.citations.len() as f64
        };
        for (sum, k) in sums.iter_mut().zip(CUTOFFS) {
            *sum += recall(k);
        }
        let first_hit_rank = ranked
            .iter()
            .position(|id| question.citations.iter().any(|c| c == id))
            .map(|position| position + 1);
        reciprocal += first_hit_rank.map_or(0.0, |rank| 1.0 / rank as f64);
        per_question.push(QuestionRetrieval {
            id: question.id.clone(),
            citations: question.citations.clone(),
            first_hit_rank,
            recall_at_5: round(recall(5)),
        });
    }

    let count = questions.len().max(1) as f64;
    RetrievalReport {
        retriever: "bm25".to_string(),
        passages: corpus.passages.len(),
        questions: questions.len(),
        recall_at: CUTOFFS
            .iter()
            .zip(sums)
            .map(|(k, sum)| (format!("{k:02}"), round(sum / count)))
            .collect(),
        mrr: round(reciprocal / count),
        per_question,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Source;

    fn passage(id: &str, body: &str) -> Passage {
        Passage {
            id: id.to_string(),
            source: Source::Order {
                number: "DO-1-01".to_string(),
            },
            title: String::new(),
            body: body.to_string(),
            footnotes: String::new(),
            heading_notes: String::new(),
            status_note: None,
        }
    }

    fn corpus() -> Corpus {
        Corpus::from_passages(vec![
            passage("b", "overtime pay for hours worked"),
            passage("a", "overtime pay for hours worked"),
            passage("c", "retirement age and benefits"),
        ])
        .unwrap()
    }

    fn question(citations: &[&str]) -> BenchmarkQuestion {
        serde_json::from_value(serde_json::json!({
            "id": "q",
            "question": "What is overtime pay?",
            "expected_answer": "",
            "citations": citations,
            "evidence": [],
            "key_points": [],
        }))
        .unwrap()
    }

    #[test]
    fn test_ties_are_broken_by_citation() {
        let corpus = corpus();
        let ids: Vec<&str> = Bm25::new(&corpus)
            .search("overtime", 3)
            .iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn test_tokens_drop_stopwords_and_plurals() {
        assert_eq!(tokenize("What are the Benefits?"), ["benefit"]);
        assert_eq!(tokenize("class"), ["class"]);
    }

    #[test]
    fn test_metrics() {
        let report = evaluate(&corpus(), &[question(&["b", "c"])]);
        assert_eq!(report.recall_at["01"], 0.0);
        assert_eq!(report.recall_at["03"], 0.5);
        assert_eq!(report.mrr, 0.5);
        assert_eq!(report.per_question[0].first_hit_rank, Some(2));
    }

    #[test]
    fn test_missed_question_has_no_rank() {
        let report = evaluate(&corpus(), &[question(&["c"])]);
        assert_eq!(report.per_question[0].first_hit_rank, None);
        assert_eq!(report.mrr, 0.0);
    }
}
