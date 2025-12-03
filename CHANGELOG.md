# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Project Initialization**: Repository scaffolding for open-labor-ph.
  - Implementation: Directory structure (`data/raw`, `data/processed`, `engine/ingestion`, `engine/evals`)
  - Impact: Foundation for Philippine Labor Law dataset creation and RAG optimization.
- **Documentation**: Core documentation files (README.md, DISCLAIMER.md, LICENSE, CONTRIBUTING.md)
  - Impact: Clear project mission, legal compliance, and contribution guidelines.
- **Rust-First Architecture**: Committed to Rust as primary language for data processing.
  - Implementation: Updated all documentation to reflect Rust workspace structure
  - Impact: High-performance, memory-safe PDF parsing and data validation
  - Files: Updated README.md, CONTRIBUTING.md, requirements.txt, .gitignore
- **Development Setup**: Minimal Python dependencies for bindings only.
  - Implementation: `requirements.txt` with PyO3/maturin for Python bindings
  - Impact: Reproducible development environment with Rust toolchain focus.
- **Phase 1 Completion: Foundation & Schema Design**
  - Implementation: Complete JSON schemas for DOLE Orders, Labor Code, and benchmarks
  - Files: `data/schemas/*.json`, `engine/ingestion/src/types.rs`
  - Impact: Data model foundation ready for Phase 2 (ingestion pipeline)
- **Rust Workspace Setup**: Cargo workspace with ingestion and evals crates
  - Implementation: Root `Cargo.toml`, `engine/ingestion/Cargo.toml`, `engine/evals/Cargo.toml`
  - Impact: Type-safe data structures with serde, ready for PDF parsing
- **Data Source Inventory**: Documented priority DOLE Orders and Labor Code sections
  - Implementation: `DATA_SOURCES.md` with 10 priority DOLE Orders and key Labor Code books
  - Impact: Clear roadmap for Phase 2 data collection
