//! Evaluation framework for the Philippine labor law dataset.
//!
//! [`questions`] checks the benchmark against the corpus, [`retrieval`]
//! measures a BM25 baseline, [`llm`] and [`runner`] collect model answers,
//! and [`scoring`] grades them with a fixed rubric.

pub mod corpus;
pub mod llm;
pub mod questions;
pub mod retrieval;
pub mod runner;
pub mod scoring;
