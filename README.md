# Open-Labor-PH: The Philippine Labor Law Intelligence Dataset

**An open-source initiative to structure, validate, and democratize Philippine Labor Law for the era of Local AI.**

## 🚀 Mission

Philippine Labor Law is complex, scattered across decades of Department Orders, and currently trapped in PDF format. This repository aims to convert unstructured government statutes into **machine-readable, RAG-optimized datasets** (`.json`) with an accompanying **Evaluation Suite** to benchmark how well LLMs understand PH regulations.

_Note: This is the open-research arm of [hipstaff.asia](https://hipstaff.asia)._

## 📂 Project Structure

- `/data`: The clean, structured JSON datasets (The Product).
  - `/data/sources.toml`: Source manifest listing every document with its official URL and pinned SHA-256
  - `/data/schemas`: JSON Schema (Draft-07) definitions for validation
  - `/data/raw`: Source PDFs (local only, gitignored; fetched and verified against `sources.toml`)
  - `/data/processed`: Validated JSON outputs (committed)
- `/engine`: The Rust workspace for ingestion and evaluation pipelines.
  - `/engine/ingestion`: Source fetcher (`fetch`), PDF parsers (`parse-dole`, `parse-labor-code`), and the schema validator (`validate`)
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

### Fetch the Source PDFs

Every document is listed in [`data/sources.toml`](data/sources.toml) with its official page and the SHA-256 of the exact PDF the dataset is built from.

```bash
# Download what can be downloaded, verify everything already in data/raw/
cargo run --release --bin fetch

# After checking a newly added file, record its hash in the manifest
cargo run --release --bin fetch -- --pin
```

- Entries with a `download_url` are downloaded automatically. Each download must be a real PDF and must match the pinned hash, or nothing is written.
- All dole.gov.ph hosts sit behind a Cloudflare JavaScript challenge that blocks automated downloads. For those entries, `fetch` prints a `manual` line with the official page and the exact path to save the PDF to (`data/raw/<id>.pdf`). It verifies the file on the next run.
- A hash mismatch is always an error and never overwrites a file. The exit code is non-zero if any entry failed.

### Parse a DOLE Department Order

```bash
cargo run --release --bin parse-dole -- \
  data/raw/DO-174-17.pdf data/processed/DO-174-17.json
```

The PDF's SHA-256 is looked up in `data/sources.toml`. A pinned entry supplies the `source_url` and `published_date`, and parsing fails if the extracted order number doesn't match the entry's `id`. Flags override the manifest:

- `--source-url <url>` (`http`/`https` only): the official page the PDF came from, recorded as provenance. **Required** when the PDF isn't pinned in the manifest.
- `--published-date YYYY-MM-DD`: the official publication date. Department Orders usually take effect a fixed number of days after publication, and that date is not printed in the Order itself. If no publication date is available, `effective_date` is **omitted** and a warning is logged, rather than the parser guessing.
- `--effective-date YYYY-MM-DD`: overrides the derived effective date.
- `--manifest <path>`: use a manifest other than `data/sources.toml`.

The output keeps the effectivity sentence (`metadata.effectivity_clause`) and the signing date (`metadata.signed_date`) so that every date can be audited. It also includes `provenance` (the source PDF's SHA-256 and the parser version), so any record can be traced to its exact source file.

### Parse the Labor Code

```bash
cargo run --release --bin parse-labor-code -- \
  data/raw/labor-code-renumbered.pdf data/processed/labor_code.json
```

Writes a JSON array with one object per article, using the 2015 renumbering. The former number goes in `former_article_number`. Amendment notes ("As amended by …") are parsed into `metadata.amendments`. Each article carries the same `provenance` as DOLE records.

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
- 🟡 **Phase 2: Data Ingestion Pipeline**: Tooling complete; Labor Code ingested
  - ✅ `parse-dole`, `parse-labor-code`, `validate`
  - ✅ Source manifest and `fetch` (hash-pinned; the Labor Code is pinned)
  - ✅ Labor Code processed: [`data/processed/labor_code.json`](data/processed/labor_code.json) (317 articles, with editorial footnotes and amendment history)
  - 🔲 Department Orders (PDFs must be downloaded by hand; see `fetch`)
  - 🔲 OCR for scanned PDFs
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
