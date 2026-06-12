#!/usr/bin/env python3
"""Re-run the ingestion pipeline for every document in data/raw/manifest.json.

For each manifest entry this script:
  1. verifies the raw file's SHA-256 against the manifest (provenance gate);
  2. picks the parser binary from doc_type (dole_order -> parse-dole,
     labor_code -> parse-labor-code);
  3. parses the OCR'd copy when `ocr_file` is set, otherwise the raw PDF,
     passing any `overrides` as CLI flags;
  4. validates the JSON output against its schema with the validate binary.

Exits non-zero if any step fails, so CI can gate on it. Documents with
status "pending" are skipped; "ocr_required" documents without an ocr_file
are reported and skipped.

Usage:
  python3 scripts/ingest.py [--release]
"""

import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
RAW = ROOT / "data" / "raw"
PROCESSED = ROOT / "data" / "processed"

PARSER_FOR_TYPE = {
    "dole_order": "parse-dole",
    "labor_code": "parse-labor-code",
}

OVERRIDE_FLAGS = {
    "order_number": "--order-number",
    "title": "--title",
    "effective_date": "--effective-date",
}


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    profile = "release" if "--release" in sys.argv else "debug"
    bin_dir = ROOT / "target" / profile
    if not (bin_dir / "parse-dole").exists():
        print(f"error: binaries not found in {bin_dir}; run cargo build first", file=sys.stderr)
        return 1

    manifest = json.loads((RAW / "manifest.json").read_text())
    failures = 0
    processed = 0

    for doc in manifest["documents"]:
        doc_id = doc["id"]
        raw_path = RAW / doc["file"]

        if not raw_path.exists():
            print(f"FAIL {doc_id}: missing raw file {raw_path}")
            failures += 1
            continue

        actual = sha256(raw_path)
        if actual != doc["sha256"]:
            print(f"FAIL {doc_id}: SHA-256 mismatch (manifest {doc['sha256'][:12]}…, file {actual[:12]}…)")
            failures += 1
            continue

        if doc["status"] == "pending":
            print(f"skip {doc_id}: status pending")
            continue

        ocr_file = doc.get("ocr_file")
        if doc["status"] == "ocr_required" and not ocr_file:
            print(f"skip {doc_id}: OCR required but no ocr_file present yet")
            continue

        input_path = RAW / ocr_file if ocr_file else raw_path
        if not input_path.exists():
            print(f"FAIL {doc_id}: input {input_path} not found")
            failures += 1
            continue

        parser = PARSER_FOR_TYPE.get(doc["doc_type"])
        if parser is None:
            print(f"FAIL {doc_id}: unknown doc_type {doc['doc_type']!r}")
            failures += 1
            continue

        output = ROOT / doc.get("output", f"data/processed/{doc_id}.json")
        cmd = [str(bin_dir / parser), str(input_path), str(output)]
        if parser == "parse-dole":
            cmd += ["--source-url", doc["official_source"]]
            for key, flag in OVERRIDE_FLAGS.items():
                value = doc.get("overrides", {}).get(key)
                if value:
                    cmd += [flag, value]

        if subprocess.run(cmd, cwd=ROOT).returncode != 0:
            print(f"FAIL {doc_id}: parser exited non-zero")
            failures += 1
            continue

        validate = subprocess.run([str(bin_dir / "validate"), str(output)], cwd=ROOT)
        if validate.returncode != 0:
            print(f"FAIL {doc_id}: schema validation failed")
            failures += 1
            continue

        print(f"ok   {doc_id} -> {output.relative_to(ROOT)}")
        processed += 1

    print(f"\n{processed} processed, {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
