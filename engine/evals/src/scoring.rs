//! Fixed-rubric grading of model answers.
//!
//! A phrase match shows that an answer states a fact, not that its reasoning
//! is sound: "not 30 days" still contains "30 days". Scores are repeatable
//! and cheap, and they are a screening measure rather than legal review.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use labor_ingestion::{BenchmarkQuestion, KeyPoint};
use regex::Regex;
use serde::Serialize;

use crate::corpus::{Corpus, Source};

/// Lowercase, straighten quotes and collapse whitespace.
pub fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| match c {
            '‘' | '’' => '\'',
            '“' | '”' => '"',
            '–' | '—' => '-',
            other => other,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `phrase` occurs in normalized `text` as whole words, so "30%"
/// does not match inside "130%".
pub fn contains_phrase(text: &str, phrase: &str) -> bool {
    let phrase = normalize(phrase);
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    text.match_indices(&phrase).any(|(start, matched)| {
        let before = text[..start].chars().next_back();
        let after = text[start + matched.len()..].chars().next();
        // A hyphen joins words: "five years" is not in "sixty-five years".
        let joined = word(before) || before == Some('-');
        !(word(phrase.chars().next()) && joined)
            && !(word(phrase.chars().next_back()) && word(after))
    })
}

fn key_point_hit(answer: &str, point: &KeyPoint) -> bool {
    point
        .any_of
        .iter()
        .any(|option| contains_phrase(answer, option))
}

fn article_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:articles?|arts?\.?)\s*(?:nos?\.?\s*)?(\d+(?:\s*(?:,|and|to|&|-)\s*\d+)*)",
        )
        .expect("static regex")
    })
}

fn order_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:(?i:department\s+order|d\.\s?o\.)|\bDO)[\s-]*(?i:nos?\.?\s*)?(\d+)")
            .expect("static regex")
    })
}

fn numbers_cited(text: &str, re: &Regex) -> BTreeSet<u32> {
    let digits = Regex::new(r"\d+").expect("static regex");
    re.captures_iter(text)
        .flat_map(|captures| {
            digits
                .find_iter(captures.get(1).map_or("", |m| m.as_str()))
                .filter_map(|m| m.as_str().parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}

/// Whether an answer cites a passage. Labor Code passages count by article,
/// under the current or the pre-2015 number. Orders count by order number,
/// because answers rarely quote this dataset's section ids.
fn cites(answer: &str, source: &Source) -> bool {
    match source {
        Source::LaborCode { article, former } => {
            let cited = numbers_cited(answer, article_re());
            cited.contains(article) || former.is_some_and(|f| cited.contains(&f))
        }
        Source::Order { number } => {
            let core = number.split('-').nth(1).and_then(|n| n.parse::<u32>().ok());
            core.is_some_and(|n| numbers_cited(answer, order_re()).contains(&n))
        }
    }
}

/// The grade of one answer.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AnswerScore {
    pub key_points_hit: usize,
    pub key_points_total: usize,
    pub citations_hit: usize,
    pub citations_total: usize,
    /// The answer states every key point and cites every source.
    pub pass: bool,
    pub missing_key_points: Vec<String>,
}

/// Grade one answer against one question.
pub fn score_answer(question: &BenchmarkQuestion, answer: &str, corpus: &Corpus) -> AnswerScore {
    let normalized = normalize(answer);
    let missing_key_points: Vec<String> = question
        .key_points
        .iter()
        .filter(|point| !key_point_hit(&normalized, point))
        .map(|point| point.label.clone())
        .collect();
    // Sections of one order, or a passage cited twice, count once.
    let sources: BTreeMap<String, &Source> = question
        .citations
        .iter()
        .filter_map(|c| corpus.get(c))
        .map(|p| match &p.source {
            Source::LaborCode { article, .. } => (format!("art:{article}"), &p.source),
            Source::Order { number } => (number.clone(), &p.source),
        })
        .collect();
    let citations_hit = sources.values().filter(|s| cites(answer, s)).count();
    let key_points_total = question.key_points.len();
    let key_points_hit = key_points_total - missing_key_points.len();
    AnswerScore {
        key_points_hit,
        key_points_total,
        citations_hit,
        citations_total: sources.len(),
        pass: missing_key_points.is_empty() && citations_hit == sources.len(),
        missing_key_points,
    }
}

/// Totals over a group of answers.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Tally {
    pub answers: usize,
    pub passed: usize,
    pub pass_rate: f64,
    pub key_point_coverage: f64,
    pub citation_coverage: f64,
}

/// Totals for a run, overall and by question group.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Summary {
    pub overall: Tally,
    pub by_group: BTreeMap<String, Tally>,
}

#[derive(Default)]
struct Totals {
    answers: usize,
    passed: usize,
    key_points: (usize, usize),
    citations: (usize, usize),
}

impl Totals {
    fn add(&mut self, score: &AnswerScore) {
        self.answers += 1;
        self.passed += usize::from(score.pass);
        self.key_points.0 += score.key_points_hit;
        self.key_points.1 += score.key_points_total;
        self.citations.0 += score.citations_hit;
        self.citations.1 += score.citations_total;
    }

    fn tally(&self) -> Tally {
        let ratio = |(hit, total): (usize, usize)| {
            let value = hit as f64 / total.max(1) as f64;
            (value * 10_000.0).round() / 10_000.0
        };
        Tally {
            answers: self.answers,
            passed: self.passed,
            pass_rate: ratio((self.passed, self.answers)),
            key_point_coverage: ratio(self.key_points),
            citation_coverage: ratio(self.citations),
        }
    }
}

/// Add up scored answers. A question may fall in several groups.
pub fn summarize<'a>(scores: impl Iterator<Item = (Vec<String>, &'a AnswerScore)>) -> Summary {
    let mut overall = Totals::default();
    let mut groups: BTreeMap<String, Totals> = BTreeMap::new();
    for (names, score) in scores {
        overall.add(score);
        for name in names {
            groups.entry(name).or_default().add(score);
        }
    }
    Summary {
        overall: overall.tally(),
        by_group: groups
            .into_iter()
            .map(|(name, totals)| (name, totals.tally()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Passage;

    fn corpus() -> Corpus {
        let passage = |id: &str, source: Source| Passage {
            id: id.to_string(),
            source,
            title: String::new(),
            body: String::new(),
            footnotes: String::new(),
            heading_notes: String::new(),
            status_note: None,
        };
        Corpus::from_passages(vec![
            passage(
                "Labor Code Art. 297",
                Source::LaborCode {
                    article: 297,
                    former: Some(282),
                },
            ),
            passage(
                "DO-147-15 Section 1",
                Source::Order {
                    number: "DO-147-15".to_string(),
                },
            ),
            passage(
                "DO-147-15 Section 2",
                Source::Order {
                    number: "DO-147-15".to_string(),
                },
            ),
        ])
        .unwrap()
    }

    fn question() -> BenchmarkQuestion {
        serde_json::from_value(serde_json::json!({
            "id": "q",
            "question": "?",
            "expected_answer": "",
            "citations": ["Labor Code Art. 297", "DO-147-15 Section 1", "DO-147-15 Section 2"],
            "evidence": [],
            "key_points": [{"label": "five days", "any_of": ["5 days", "five days"]}],
        }))
        .unwrap()
    }

    #[test]
    fn test_full_answer_passes() {
        let score = score_answer(
            &question(),
            "Five  days under Article 297 and Department Order No. 147-15.",
            &corpus(),
        );
        assert!(score.pass, "{score:?}");
        assert_eq!(score.citations_total, 2, "sections of one order count once");
    }

    #[test]
    fn test_empty_answer_scores_nothing() {
        let score = score_answer(&question(), "", &corpus());
        assert!(!score.pass);
        assert_eq!((score.key_points_hit, score.citations_hit), (0, 0));
        assert_eq!(score.missing_key_points, ["five days"]);
    }

    #[test]
    fn test_key_point_alone_does_not_pass() {
        let score = score_answer(&question(), "5 days.", &corpus());
        assert_eq!(score.key_points_hit, 1);
        assert!(!score.pass);
    }

    #[test]
    fn test_former_article_number_counts() {
        let source = Source::LaborCode {
            article: 297,
            former: Some(282),
        };
        assert!(cites("See Art. 282 of the Code.", &source));
        assert!(cites("Articles 100, 282 and 300", &source));
        assert!(!cites("See Art. 2820.", &source));
        assert!(!cites("See paragraph 297.", &source));
    }

    #[test]
    fn test_order_citation_forms() {
        let source = Source::Order {
            number: "DO-174-17".to_string(),
        };
        for text in [
            "DO 174-17",
            "DO-174-17",
            "D.O. No. 174, series of 2017",
            "Department Order No. 174",
        ] {
            assert!(cites(text, &source), "{text}");
        }
        assert!(!cites("what they do 174 times", &source));
        assert!(!cites("DO 18-A", &source));
    }

    #[test]
    fn test_phrases_match_whole_words() {
        assert!(contains_phrase("at least 30% more", "30%"));
        assert!(!contains_phrase("at least 130% more", "30%"));
        assert!(!contains_phrase("sixty-five years", "five years"));
        assert!(contains_phrase("(five years)", "five years"));
    }

    #[test]
    fn test_normalize() {
        assert_eq!(
            normalize("The  “Control”\nTest’s"),
            "the \"control\" test's"
        );
    }

    #[test]
    fn test_summary_groups() {
        let pass = AnswerScore {
            key_points_hit: 2,
            key_points_total: 2,
            citations_hit: 1,
            citations_total: 1,
            pass: true,
            missing_key_points: Vec::new(),
        };
        let fail = AnswerScore {
            key_points_hit: 1,
            key_points_total: 2,
            citations_hit: 0,
            citations_total: 1,
            pass: false,
            missing_key_points: vec!["x".to_string()],
        };
        let rows = [
            (vec!["wages".to_string()], &pass),
            (vec!["wages".to_string(), "hard".to_string()], &fail),
        ];
        let summary = summarize(rows.into_iter());
        assert_eq!(summary.overall.passed, 1);
        assert_eq!(summary.overall.key_point_coverage, 0.75);
        assert_eq!(summary.by_group["wages"].answers, 2);
        assert_eq!(summary.by_group["hard"].pass_rate, 0.0);
    }
}
