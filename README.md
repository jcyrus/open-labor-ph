# Open-Labor-PH: The Philippine Labor Law Intelligence Dataset

**An open-source initiative to structure, validate, and democratize Philippine Labor Law for the era of Local AI.**

## 🚀 Mission

Philippine Labor Law is complex, scattered across decades of Department Orders, and currently trapped in PDF format. This repository aims to convert unstructured government statutes into **machine-readable, RAG-optimized datasets** (`.json`) with an accompanying **Evaluation Suite** to benchmark how well LLMs understand PH regulations.

_Note: This is the open-research arm of [hipstaff.asia](https://hipstaff.asia)._

## 📂 Project Structure

- `/data`: The clean, structured JSON datasets (The Product).
  - `/data/schemas`: JSON Schema (Draft-07) definitions for validation
  - `/data/raw`: Source PDFs (local only, gitignored; sources are listed in [DATA_SOURCES.md](DATA_SOURCES.md))
  - `/data/processed`: Validated JSON outputs (committed)
- `/engine`: The Rust workspace for ingestion and evaluation pipelines.
  - `/engine/ingestion`: PDF parsers (`parse-dole`, `parse-labor-code`) and the schema validator (`validate`)
  - `/engine/evals`: RAG evaluation framework (Phase 3, not yet implemented)

## 🚀 Getting Started

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Verify installation
rustc --version
cargo --version
```

### Build and Test

```bash
# Clone the repository
git clone https://github.com/jcyrus/open-labor-ph.git
cd open-labor-ph

# Build all workspace crates (Cargo.lock is committed; --locked keeps builds reproducible)
cargo build --release --locked

# Run the test suite, lints, and format check
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

### Parse a DOLE Department Order

```bash
cargo run --release --bin parse-dole -- \
  data/raw/DO-174-17.pdf data/processed/DO-174-17.json \
  --source-url https://bwc.dole.gov.ph/issuances/department-orders/ \
  --published-date 2017-03-20
```

- `--source-url` (**required**, `http`/`https` only): the official page the PDF came from. It is recorded as the record's provenance.
- `--published-date YYYY-MM-DD` (optional): the official publication date. Department Orders usually take effect a fixed number of days after publication, and that date is not printed in the Order itself. If you leave it out, `effective_date` is **omitted** and a warning is logged, rather than the parser guessing.
- `--effective-date YYYY-MM-DD` (optional): overrides the derived effective date.

The output keeps the effectivity sentence (`metadata.effectivity_clause`) and the signing date (`metadata.signed_date`) so that every date can be audited.

### Parse the Labor Code

```bash
cargo run --release --bin parse-labor-code -- \
  data/raw/labor-code-renumbered.pdf data/processed/labor_code.json
```

Writes a JSON array with one object per article, using the 2015 renumbering. The former number goes in `former_article_number`. Amendment notes ("As amended by …") are parsed into `metadata.amendments`.

### Validate Output

```bash
cargo run --release --bin validate -- data/processed/DO-174-17.json
```

The schema is inferred from the document's shape, or you can pass it explicitly with `--schema data/schemas/<name>.json`. Arrays are validated element by element. The exit code is non-zero if any element fails validation.

Every binary accepts `--help`.

### Project Status

- ✅ **Phase 1: Foundation & Schema Design**: Complete
  - JSON schemas for DOLE Orders, Labor Code articles, and benchmark questions
  - Rust type definitions with serde
  - Verified data source inventory ([DATA_SOURCES.md](DATA_SOURCES.md))
- 🟡 **Phase 2: Data Ingestion Pipeline**: Tooling complete, no documents ingested yet
  - ✅ `parse-dole`, `parse-labor-code`, `validate`
  - 🔲 Source download pipeline and OCR for scanned PDFs
  - 🔲 First processed documents in `data/processed/`
- 🔲 **Phase 3: Evaluation Framework**: Not started
- 🔲 **Phase 4: Documentation & Community**: Not started (Python bindings via PyO3 are planned here)
- 🔲 **Phase 5: Initial Release**: Not started

## 📊 Dataset Schema

See [`data/schemas/`](data/schemas/) for complete JSON Schema definitions:

- [`dole_order_schema.json`](data/schemas/dole_order_schema.json): DOLE Department Orders
- [`labor_code_schema.json`](data/schemas/labor_code_schema.json): Labor Code articles
- [`benchmark_question_schema.json`](data/schemas/benchmark_question_schema.json): Evaluation benchmarks

## 📚 Documentation

- [DATA_SOURCES.md](DATA_SOURCES.md): Catalog of priority legal sources
- [CONTRIBUTING.md](CONTRIBUTING.md): How to contribute
- [CHANGELOG.md](CHANGELOG.md): Version history

## ⚖️ License

See `DISCLAIMER.md` for data rights and `LICENSE` for code usage.
