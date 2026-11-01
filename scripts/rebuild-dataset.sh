#!/usr/bin/env bash
# Rebuild data/processed from the PDFs in data/raw and validate every record.
# Run `cargo build --release` and `target/release/fetch` first.
set -euo pipefail

cd "$(dirname "$0")/.."
bin=target/release

"$bin/parse-labor-code" data/raw/labor-code-renumbered.pdf data/processed/labor_code.json
"$bin/validate" data/processed/labor_code.json

for pdf in data/raw/DO-*.pdf; do
    id=$(basename "$pdf" .pdf)
    "$bin/parse-dole" "$pdf" "data/processed/$id.json"
    "$bin/validate" "data/processed/$id.json"
done
