# Contributing to Open-Labor-PH

Thank you for your interest in contributing to democratizing Philippine Labor Law data! 🇵🇭

## 🎯 How You Can Help

### 1. Data Contribution

- **Source Documents**: Upload new DOLE Department Orders or Labor Code amendments to `data/raw/`
- **Validation**: Review parsed JSON outputs for accuracy against official sources
- **Benchmarking**: Submit real-world labor law questions to `engine/evals/`

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

### Python

- **Type Safety**: Use type hints for all function signatures
- **Documentation**: Add docstrings for public functions/classes
- **Formatting**: Use `black` for code formatting
- **Linting**: Ensure `pylint` or `ruff` passes

### Data Format

- JSON files must be **valid** and **prettified** (use `json.dumps(indent=2)`)
- Include metadata fields: `source_url`, `effective_date`, `last_updated`

## ⚖️ Legal Guidelines

- Only submit documents that are **public domain** (government works)
- Do not include copyrighted commentary or annotations
- Cite official sources (e.g., Official Gazette, DOLE website)

## 🧪 Testing

Before submitting:

- Ensure all scripts run without errors
- Validate JSON outputs against schema (if available)
- Test evaluation scripts produce expected results

## 💬 Questions?

Open an issue with the `question` label or reach out to the maintainers.

---

**By contributing, you agree that your contributions will be licensed under the MIT License (code) and CC-BY-4.0 (dataset structure).**
