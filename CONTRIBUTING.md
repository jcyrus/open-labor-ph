# Contributing to Open-Labor-PH

Thank you for your interest in contributing to democratizing Philippine Labor Law data! 🇵🇭

## 🎯 How You Can Help

### 1. Data Contribution

- **Source Documents**: Add new DOLE Department Orders or Labor Code amendments to `DATA_SOURCES.md` with their official URL. Source PDFs go in `data/raw/` locally but are gitignored; the validated JSON output in `data/processed/` is what gets committed
- **Validation**: Review parsed JSON outputs for accuracy against official sources
- **Benchmarking**: Propose real-world labor law questions in an issue with the `benchmark` label, including the question, expected answer, and citations as described in `data/schemas/benchmark_question_schema.json`

### 2. Code Contribution

- **Parsing Improvements**: Enhance PDF extraction quality in `engine/ingestion/`
- **Evaluation Metrics**: Add new evaluation criteria for LLM accuracy
- **Documentation**: Improve code comments, README sections, or add examples

## 📋 Contribution Workflow

1. **Fork & Clone** the repository
2. **Create a Branch**: `git checkout -b feat/your-feature-name`
3. **Make Changes**: Follow the code standards below
4. **Update CHANGELOG**: Add your changes under `[Unreleased]` in `CHANGELOG.md`
5. **Commit**: Use [Conventional Commits](https://www.conventionalcommits.org/)
   - `feat(ingestion): add support for DOLE Order parsing`
   - `fix(evals): correct benchmark question formatting`
   - `docs(readme): update installation instructions`
6. **Push & PR**: Submit a pull request with a clear description

## 🛠️ Code Standards

### Rust (Primary Language)

- **Type Safety**: Use strong typing with `serde` for all data structures
- **Error Handling**: Use `anyhow` or `thiserror` for proper error propagation
- **Documentation**: Add doc comments (`///`) for all public functions/structs
- **Formatting**: Use `cargo fmt` before committing
- **Linting**: Ensure `cargo clippy` passes with no warnings
- **Testing**: Write unit tests for parsers and validators

### Python (Planned Bindings & Examples)

Python bindings (PyO3) are planned for Phase 4 and do not exist yet. When they land:

- **Type Safety**: Use type hints for all function signatures
- **Documentation**: Add docstrings for public functions/classes
- **Formatting**: Use `black` for code formatting
- **Linting**: Ensure `pylint` or `ruff` passes

### Data Format

- Generate dataset JSON with the Rust binaries (`parse-dole`, `parse-labor-code`). They write pretty-printed JSON (2-space indent, trailing newline). Don't hand-write or reformat files in `data/processed/`. Fix the parser instead.
- Every file must pass `validate` (see Testing below).
- Provenance is required: DOLE Orders must be parsed with `--source-url` pointing to the official page the PDF came from.
- Dates are never guessed: pass `--published-date` when you know the official publication date, otherwise let `effective_date` be omitted. Never fill it in by hand.

## ⚖️ Legal Guidelines

- Only submit documents that are **public domain** (government works)
- Do not include copyrighted commentary or annotations
- Cite official sources (e.g., Official Gazette, DOLE website)

## 🧪 Testing

Before submitting, run from the repository root:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
```

If you changed anything in `data/processed/`, validate every file you touched:

```bash
for f in data/processed/*.json; do cargo run --release --bin validate -- "$f" || exit 1; done
```

## 💬 Questions?

Open an issue with the `question` label or reach out to the maintainers.

---

**By contributing, you agree that your contributions will be licensed under the MIT License (code) and CC-BY-4.0 (dataset structure).**
