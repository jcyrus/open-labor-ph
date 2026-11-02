//! Benchmark questions and the checks that tie them to the corpus.

use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use labor_ingestion::{BenchmarkQuestion, Difficulty, QuestionCategory};

use crate::corpus::Corpus;
use crate::scoring::{contains_phrase, normalize, score_answer};

/// Benchmark location relative to the repository root.
pub const DEFAULT_QUESTIONS_PATH: &str = "data/benchmark/questions.json";

/// Tag required on a question that rests on text no longer in force.
pub const STATUS_TAG: &str = "legal_status";

/// Read the benchmark.
pub fn load(path: &Path) -> Result<Vec<BenchmarkQuestion>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

/// Groups a question is reported under.
pub fn groups(question: &BenchmarkQuestion) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(category) = &question.category {
        names.push(format!("category:{}", category_name(category)));
    }
    if let Some(difficulty) = &question.difficulty {
        names.push(format!("difficulty:{}", difficulty_name(difficulty)));
    }
    if question.tags.iter().any(|t| t == STATUS_TAG) {
        names.push(format!("tag:{STATUS_TAG}"));
    }
    names
}

fn category_name(category: &QuestionCategory) -> String {
    serde_json::to_value(category)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn difficulty_name(difficulty: &Difficulty) -> String {
    serde_json::to_value(difficulty)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Everything wrong with one question.
fn check_question(question: &BenchmarkQuestion, corpus: &Corpus) -> Vec<String> {
    let mut problems = Vec::new();
    let mut report = |message: String| problems.push(format!("{}: {message}", question.id));

    if question.citations.is_empty() || question.evidence.is_empty() {
        report("needs citations and evidence".to_string());
    }
    if question.key_points.is_empty() {
        report("needs key points".to_string());
    }

    let tagged = question.tags.iter().any(|t| t == STATUS_TAG);
    for citation in &question.citations {
        match corpus.get(citation) {
            None => report(format!("no passage {citation:?}")),
            Some(passage) if passage.status_note.is_some() && !tagged => report(format!(
                "{citation} no longer applies as printed; tag the question {STATUS_TAG} or \
                 choose another source"
            )),
            Some(_) => {}
        }
        if !question.evidence.iter().any(|e| &e.citation == citation) {
            report(format!("no evidence for {citation:?}"));
        }
    }

    let mut grounds = String::new();
    for evidence in &question.evidence {
        if !question.citations.contains(&evidence.citation) {
            report(format!(
                "evidence cites {:?}, not listed",
                evidence.citation
            ));
            continue;
        }
        let Some(passage) = corpus.get(&evidence.citation) else {
            continue;
        };
        if !normalize(&passage.full_text()).contains(&normalize(&evidence.quote)) {
            report(format!(
                "quote not found in {}: {:?}",
                evidence.citation, evidence.quote
            ));
        }
        grounds.push_str(&normalize(&evidence.quote));
        grounds.push(' ');
    }

    let asked = normalize(&question.question);
    for point in &question.key_points {
        if !point.any_of.iter().any(|o| contains_phrase(&grounds, o)) {
            report(format!(
                "key point {:?} is not in the evidence",
                point.label
            ));
        }
        if point.any_of.iter().any(|o| contains_phrase(&asked, o)) {
            report(format!(
                "key point {:?} is met by repeating the question",
                point.label
            ));
        }
    }

    let gold = score_answer(question, &question.expected_answer, corpus);
    if !gold.pass {
        report(format!("expected answer fails its own rubric: {gold:?}"));
    }
    problems
}

/// Every problem in the benchmark, so one pass fixes them all.
pub fn check(questions: &[BenchmarkQuestion], corpus: &Corpus) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    for question in questions {
        if !seen.insert(question.id.as_str()) {
            problems.push(format!("{}: duplicate id", question.id));
        }
        problems.extend(check_question(question, corpus));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{Passage, Source};
    use serde_json::{json, Value};

    fn corpus(status_note: Option<&str>) -> Corpus {
        Corpus::from_passages(vec![Passage {
            id: "Labor Code Art. 95".to_string(),
            source: Source::LaborCode {
                article: 95,
                former: None,
            },
            title: String::new(),
            body: "a yearly service incentive leave of\nfive days with pay".to_string(),
            footnotes: String::new(),
            heading_notes: String::new(),
            status_note: status_note.map(str::to_string),
        }])
        .unwrap()
    }

    fn question(edit: impl FnOnce(&mut Value)) -> BenchmarkQuestion {
        let mut value = json!({
            "id": "q",
            "question": "How long is service incentive leave?",
            "expected_answer": "Five days with pay under Article 95.",
            "citations": ["Labor Code Art. 95"],
            "evidence": [{
                "citation": "Labor Code Art. 95",
                "quote": "incentive leave of five days with pay",
            }],
            "key_points": [{"label": "five days", "any_of": ["five days", "5 days"]}],
        });
        edit(&mut value);
        serde_json::from_value(value).unwrap()
    }

    fn problems(question: BenchmarkQuestion, corpus: &Corpus) -> String {
        check(&[question], corpus).join("\n")
    }

    #[test]
    fn test_valid_question_passes() {
        assert_eq!(problems(question(|_| {}), &corpus(None)), "");
    }

    #[test]
    fn test_unknown_citation_is_refused() {
        let q = question(|v| v["citations"] = json!(["Labor Code Art. 9999"]));
        assert!(problems(q, &corpus(None)).contains("no passage"));
    }

    #[test]
    fn test_quote_outside_the_passage_is_refused() {
        let q = question(|v| v["evidence"][0]["quote"] = json!("ten days with pay"));
        assert!(problems(q, &corpus(None)).contains("quote not found"));
    }

    #[test]
    fn test_key_point_needs_grounds_in_evidence() {
        let q = question(|v| v["key_points"][0]["any_of"] = json!(["six days"]));
        assert!(problems(q, &corpus(None)).contains("is not in the evidence"));
    }

    #[test]
    fn test_key_point_must_not_repeat_the_question() {
        let q = question(|v| v["question"] = json!("Is it five days?"));
        assert!(problems(q, &corpus(None)).contains("repeating the question"));
    }

    #[test]
    fn test_expected_answer_must_pass_its_rubric() {
        let q = question(|v| v["expected_answer"] = json!("Five days with pay."));
        assert!(problems(q, &corpus(None)).contains("fails its own rubric"));
    }

    #[test]
    fn test_superseded_source_needs_the_status_tag() {
        let corpus = corpus(Some("Superseded."));
        assert!(problems(question(|_| {}), &corpus).contains(STATUS_TAG));
        let tagged = question(|v| v["tags"] = json!([STATUS_TAG]));
        assert_eq!(problems(tagged, &corpus), "");
    }

    #[test]
    fn test_duplicate_ids_are_refused() {
        let found = check(&[question(|_| {}), question(|_| {})], &corpus(None));
        assert!(found.iter().any(|p| p.contains("duplicate id")));
    }

    #[test]
    fn test_groups() {
        let q = question(|v| {
            v["category"] = json!("labor_relations");
            v["difficulty"] = json!("hard");
            v["tags"] = json!([STATUS_TAG]);
        });
        assert_eq!(
            groups(&q),
            [
                "category:labor_relations",
                "difficulty:hard",
                "tag:legal_status"
            ]
        );
    }
}
