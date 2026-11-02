# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Benchmark of 57 questions** in `data/benchmark/questions.json`, written from the corpus text. No lawyer has reviewed them, and no model has been scored yet.
- **`evals` binary**: `check` ties every question to the corpus, `retrieve` measures a BM25 baseline, `run` collects closed-book or RAG answers from Claude or any `/chat/completions` server and resumes after an interruption, and `score` grades answers with a fixed rubric.
- **BM25 retrieval baseline** in `data/benchmark/results/retrieval-bm25.json`. CI rebuilds it and fails if it changes.
- **`evidence` and `key_points`** on benchmark questions, required by the schema and `BenchmarkQuestion`.
- **7 Department Orders**: DO-147-15, DO-174-17, DO-183-17, DO-202-19, DO-53-03, DO-230-21 and, as history, DO-198-18. All are downloaded from pinned Wayback Machine snapshots. `effective_date` stays unset on all of them: each runs from a publication date that is not yet verified.
- **OCR text as a committed input**: the new `ocr` binary writes scans to `data/ocr/<id>.txt` and never replaces a file without `--force`. `parse-dole` reads that text and still hashes the PDF, so rebuilds are exact and need no OCR engine. Records carry `metadata.ocr_text`. Order numbers, signing dates and signatories were corrected against the scans.
- **`metadata.superseded_by`** from the new manifest field of the same name.
- **`scripts/rebuild-dataset.sh`**: CI now rebuilds and diffs every processed record.
- **`metadata.legal_status`** on Labor Code articles: `superseded` (33), `partly_superseded` (20) or `no_change_noted` (264), with the notes behind it in `changes`. It comes from a hand review of the edition's footnotes (`data/labor_code_status.toml`, `engine/ingestion/src/status.rs`) tied to the PDF's SHA-256. `parse-labor-code` fails on a status-like footnote that has not been reviewed; a PDF outside the review gets no `legal_status`. `no_change_noted` is not a finding that an article is in force.
- **`metadata.heading_footnotes`**: notes on Title and Chapter headings now reach every article under the heading. They were dropped before, which hid the express repeal of Arts. 139–150 (R.A. No. 10361) and the Title-wide note on Arts. 43–56 (R.A. No. 7796).
- **CI** (`.github/workflows/ci.yml`): format, clippy and tests, plus a rebuild of `labor_code.json` from the pinned PDF that fails on any difference from the committed file. NATLEX returns 403 to CI runners, so the Labor Code `download_url` now points at a Wayback Machine snapshot with the same pinned SHA-256.

### Fixed

- **`parse-dole` on real orders**: signing dates come from the closing "Manila, Philippines" line, which also ends the body; abbreviated months parse ("16 MAR 2017"); "PART II" groups keep restarted Sections apart; uppercase "SECTION 2" closes an amended Rule; "RULE I-A" keeps its letter; "A. COVERAGE" headings split orders with no Sections; titles may span blank lines; "as amended" in a citation no longer adds to `supersedes`.
- **Labor Code chapters and titles**: a heading the extractor leaves on the end of a sentence now starts its chapter, so Arts. 137–138 are in "Chapter II: EMPLOYMENT OF MINORS" and the heading is gone from Art. 136's `content`. "Title VII-A" keeps its number instead of becoming "Title VII: A – GRIEVANCE…".
- **Labor Code parser handles the real DOLE renumbered edition** (found on the first real-PDF run):
  - Book headers: the body prints "Book One - PRE-EMPLOYMENT" in title case; table-of-contents lines ending in a page number are skipped, and the body's "PRELIMINARY TITLE" resets any state left by the TOC. Previously all 317 articles landed in Book VII; now every book matches the verified ranges.
  - Footnotes: the edition's 258 editorial footnotes are split out of the text per page (from a page's first footnote to its end; wide-font remainders that spill onto the next page are moved back), inline markers are resolved per article, and the footnotes are kept in new `metadata.footnotes`. `amendments` (first sentence of "As amended/added by …" footnotes, never PD 442 itself) and `implementing_orders` (cited DOs, normalized) are derived from them.
  - Headings no longer carry footnote markers ("Security of Tenure.252"); title/chapter names keep wrapped continuations; prose like "Book IV of the Omnibus Rules" or "Chapter II, of this Title" is no longer read as a header.
- **Text extraction**: `parse_document` takes per-page text from `pdf-extract` (`extract_text_by_pages`; new `extract_text_pages`), falling back to `lopdf` only on failure. `lopdf` cannot decode Identity-H fonts, so its page text (used for running-header detection and page warnings) was placeholder garbage. `clean_text` collapses justified-text space runs inside lines.

- **Labor Code metadata is derived from the text** instead of hard-coded: "(As amended by …)" notes become `amendments` (law normalized to `RA`/`PD`/`BP`/`EO`; date kept only when the note cites a single law), articles "inserted/added/incorporated by" a later law get `original_pd_442: false`, and cross-references ("Articles 106 to 109 of this Code") fill `related_articles` (references to other codes are skipped). `Amendment.date` is now optional. `implementing_orders` stays empty by design: it needs cross-linking with the DOLE dataset.
- **`former_article_number`** field replaces the `formerly_art_N` tag for the pre-2015 article number.
- **DOLE tags are precise and consistently named**: a tag needs its keyword in the title or ≥ 3 word-bounded mentions; tags are lowercase snake_case aligned with benchmark categories (`osh`, not `OSH`). Added `harassment` and `drug_free_workplace`.
- **Issuing authority** is read from the signature block (scanning from the end) instead of the first "Secretary …" line, which could be a preamble citation.
- **Nested subsections**: "(a) … (1) … (i) …" lists keep their hierarchy via a recursive `subsections` field; "(i)" after "(h)" is read as a letter, and as a roman numeral when "(ii)" follows.
- **`--help` exits 0** and prints usage to stdout in all three binaries.
- **DOLE title extraction**: titles are found when the order number and "Series of" sit on separate lines (previously fell back to "Department Order DO-NNN-YY").
- **Signature block** is cut off before section splitting, so the last section no longer ends with "Done in the City of Manila… / SIGNATORY / Secretary". Signatory and signing date are still read from the full text.
- **`--source-url` is now required and must be http(s)** (breaking CLI change): the `file://` fallback published the operator's local path into the dataset.
- **Page furniture removed** (`engine/ingestion/src/parser.rs`): page-number lines ("Page 3 of 12", "- 3 -", bare numbers at page breaks), table-of-contents lines with dot leaders, and running headers/footers (lines on ≥ half of ≥ 3 pages; first occurrence kept). Duplicate sections/articles from a table of contents are dropped, keeping the longest.
- **`content` holds the full body** of sections and articles, enumerated items included; `subsections` still breaks the items out. Previously `content` held only the text before the first "(a)".
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

### Changed

- **`parse-dole --source-url` is optional for pinned PDFs**: the manifest entry supplies it (and `published_date`); it is still required for unpinned PDFs. `validate` reuses the shared upward path search.

- **Documentation brought in line with the code**: README status (Phase 2 tooling complete, no documents ingested), usage for all three binaries, removed the nonexistent `/bindings` directory (PyO3 bindings are planned for Phase 4), fixed the hipstaff.asia link. CONTRIBUTING data rules and pre-submit checks now name real commands and fields instead of `json.dumps` and an undefined `last_updated`.
- **`Cargo.lock` is committed** for reproducible builds of the binaries (`cargo build --locked`).
- **Schema `$id`s resolve**: they now point at the raw files on `main` instead of non-existent GitHub paths.

### Added

- **First processed dataset**: `data/processed/labor_code.json`, the renumbered Labor Code (317 articles), parsed from the pinned NATLEX PDF.
- **Source manifest and `fetch` binary**: `data/sources.toml` lists every source document (official `source_url`, optional `download_url`, `published_date`, pinned `sha256`).
  - Implementation: `engine/ingestion/src/manifest.rs`, `engine/ingestion/src/bin/fetch.rs`, `data/sources.toml`
  - `fetch` downloads entries with a `download_url` (rejecting non-PDF bodies such as bot-challenge pages), verifies every local PDF against its pin, writes atomically, and reports challenge-protected entries as manual downloads with the exact target path. `--pin` records new hashes, preserving the manifest's comments. Any mismatch is an error and the exit code is non-zero.
  - Parsers look up the input's SHA-256 in the manifest: a match supplies `source_url`/`published_date` and must agree on document kind and order number.
  - Seeded with the Tier 1/2 Department Orders (manual: dole.gov.ph blocks automated downloads) and the renumbered Labor Code from ILO NATLEX (downloaded and pinned).
- **Provenance on every record** (`provenance.source_sha256`, `provenance.parser`), required by both schemas. No timestamps, so output stays deterministic.

- **Phase 2: Data Ingestion Pipeline** (`ee17ed5`): Three CLI binaries on top of the PDF parser core.
  - Implementation: `engine/ingestion/src/bin/parse_dole.rs`, `parse_labor_code.rs`, `validate.rs`; structure detection (`split_into_blocks`, `extract_subsections`, `parse_first_date`) and text cleanup (`clean_text`) in `engine/ingestion/src/parser.rs`; schema-compatible date serialization in `engine/ingestion/src/types.rs`
  - `parse-dole`: Department Order PDF → `DoleOrder` JSON (order number, title, sections, metadata)
  - `parse-labor-code`: Labor Code PDF → array of `LaborCodeArticle` with Book/Title/Chapter context
  - `validate`: Draft-07 schema validation with schema inference and per-element array validation; non-zero exit on failure
  - Impact: End-to-end PDF → validated JSON tooling for the dataset
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
