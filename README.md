# Open-Labor-PH: The Philippine Labor Law Intelligence Dataset

**An open-source initiative to structure, validate, and democratize Philippine Labor Law for the era of Local AI.**

## 🚀 Mission

Philippine Labor Law is complex, scattered across decades of Department Orders, and currently trapped in PDF format. This repository aims to convert unstructured government statutes into **machine-readable, RAG-optimized datasets** (`.json`) with an accompanying **Evaluation Suite** to benchmark how well LLMs understand PH regulations.

_Note: This is the open-research arm of [hipstaff.asia]._

## 📂 Project Structure

- `/data`: The clean, structured JSON datasets (The Product).
  - `/data/schemas`: JSON Schema definitions for validation
  - `/data/raw`: Source PDFs and documents
  - `/data/processed`: Structured JSON outputs
- `/engine`: The Rust workspace for ingestion and evaluation pipelines.
  - `/engine/ingestion`: High-performance PDF parsers and data processors
  - `/engine/evals`: RAG evaluation framework and benchmarking tools
- `/bindings`: Python bindings (via PyO3) for integration with ML frameworks

## 🚀 Getting Started

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Verify installation
rustc --version
cargo --version
```

### Build the Project

```bash
# Clone the repository
git clone https://github.com/jcyrus/open-labor-ph.git
cd open-labor-ph

# Build all workspace crates
cargo build --release

# Check for errors
cargo check --workspace
```

### Project Status

- ✅ **Phase 1: Foundation & Schema Design** - Complete
  - JSON schemas for DOLE Orders, Labor Code, and benchmarks
  - Rust type definitions with serde
  - Data source inventory
- ✅ **Phase 2: Data Ingestion Pipeline** - Complete
  - Page-by-page PDF extraction (lopdf + pdf-extract, rayon-parallel)
  - Structural parsers: `parse-dole`, `parse-labor-code`
  - Draft-07 schema validation: `validate`
- 🟡 **Phase 3: Corpus & Evaluation Framework** - In progress
  - ✅ Source acquisition with provenance manifest (`data/raw/manifest.json`)
  - ✅ OCR ingestion of the scanned official corpus (ocrmypdf/tesseract)
  - ✅ First dataset release: Labor Code (317 articles, Books I–VII) + 3 DOLE Orders, all schema-validated
  - 🔲 Benchmark question dataset
  - 🔲 Evaluation runner (`engine/evals`)

### OCR Prerequisite

The official DOLE corpus consists of scanned PDFs. Reproducing `data/processed/`
from the raw scans requires [ocrmypdf](https://ocrmypdf.readthedocs.io/) and
tesseract (`brew install ocrmypdf` on macOS), then:

```bash
cargo build --workspace
python3 scripts/ingest.py
```
- 🔲 **Phase 4: Documentation & Community** - Not started
- 🔲 **Phase 5: Initial Release** - Not started

## 📊 Dataset Schema

See [`data/schemas/`](data/schemas/) for complete JSON Schema definitions:

- [`dole_order_schema.json`](data/schemas/dole_order_schema.json) - DOLE Department Orders
- [`labor_code_schema.json`](data/schemas/labor_code_schema.json) - Labor Code articles
- [`benchmark_question_schema.json`](data/schemas/benchmark_question_schema.json) - Evaluation benchmarks

## 📚 Documentation

- [DATA_SOURCES.md](DATA_SOURCES.md) - Catalog of priority legal sources
- [CONTRIBUTING.md](CONTRIBUTING.md) - How to contribute
- [CHANGELOG.md](CHANGELOG.md) - Version history

## ⚖️ License

See `DISCLAIMER.md` for data rights and `LICENSE` for code usage.
