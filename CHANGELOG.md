# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **DOLE effective dates are no longer fabricated** (breaking schema change): `effective_date` is now optional and set only when the document determines it.
  - Implementation: `engine/ingestion/src/bin/parse_dole.rs`, `engine/ingestion/src/types.rs`, `data/schemas/dole_order_schema.json`
  - Resolution: explicit date in the effectivity clause → "N days after publication/signing" (requires `--published-date` for publication) → "upon publication" → "immediately" (signing date). Working-day offsets are never resolved. `--effective-date` overrides.
  - New `metadata.signed_date` and `metadata.effectivity_clause`; `metadata.published_date` is filled from `--published-date`. Removed the January-1-of-series-year fallback (and its `2000 + yy` century bug).
  - Signing date now prefers the last signing keyword, so preamble citations ("issued on March 1, 2010") are no longer mistaken for it.
- **Lettered Department Orders** (`DO-18-A-11`) are now recognized in the header, `Series of` notation, filename fallback, and `supersedes`; the schema pattern accepts `DO-NNN-X-YY`.
  - `supersedes` now also catches references placed before the verb ("D.O. No. 18-A, Series of 2011 is hereby superseded"), scoped to the keyword's paragraph.
- **Labor Code book coverage**: Added `Preliminary Title` and `Book VII: Transitory and Final Provisions` to the book enum.
  - Implementation: `engine/ingestion/src/types.rs`, `data/schemas/labor_code_schema.json`, `engine/ingestion/src/bin/parse_labor_code.rs`
  - Impact: Arts. 1–11 are no longer misfiled under Book I, and Arts. 303–317 (penalties, prescription of offenses and money claims) are no longer misfiled under Book VI. `BOOK VII` headings now match (`VII` was previously truncated by the `VI` alternative)
- **Data source catalog**: Corrected `DATA_SOURCES.md` after verification against DOLE issuance indexes.
  - Impact: Ten Department Orders were listed under the wrong topics (e.g. DO-174-17 is contracting, not sexual harassment; telecommuting is DO-202-19). Added DO-252-25 (supersedes DO-198-18), DO-183-17, DO-53-03, DO-230-21; Labor Code ranges now use renumbered articles with former numbers in brackets
- **Dataset tracking**: `data/processed/*.json` is no longer gitignored, so the published dataset can be committed. Source PDFs remain ignored.

### Added

- **PDF Parser Core Module**: High-performance PDF text extraction for labor law documents.
  - Implementation: `engine/ingestion/src/parser.rs`, `engine/ingestion/src/errors.rs`
  - Features: Full text extraction, metadata extraction, page-by-page parsing
  - Dependencies: `pdf-extract`, `lopdf`, `thiserror`
  - Impact: Foundation for DOLE Order and Labor Code ingestion pipeline
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
