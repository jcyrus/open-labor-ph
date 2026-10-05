//! JSON Schema validator for ingestion outputs.
//!
//! Validates a parsed JSON file against its Draft-07 schema before it is
//! admitted into the dataset. The schema can be given explicitly or is
//! inferred from the document's shape and located under `data/schemas/`.
//!
//! Usage:
//! ```text
//! validate <data.json> [--schema <schema.json>]
//! ```
//!
//! Exit code is non-zero when any instance fails validation, so the binary
//! can gate CI pipelines.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use jsonschema::{Draft, JSONSchema};
use rayon::prelude::*;
use serde_json::Value;
use tracing::info;

use labor_ingestion::manifest::find_upward;

const USAGE: &str = "Usage: validate <data.json> [--schema <schema.json>]";

struct Cli {
    data: PathBuf,
    schema: Option<PathBuf>,
}

/// Parse CLI arguments. `Ok(None)` means help was requested.
fn parse_args() -> Result<Option<Cli>> {
    let mut args = std::env::args().skip(1);
    let mut positional: Vec<String> = Vec::new();
    let mut schema = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--schema" => {
                schema = Some(PathBuf::from(
                    args.next().context("--schema requires a value")?,
                ));
            }
            "-h" | "--help" => return Ok(None),
            _ => positional.push(arg),
        }
    }

    // Second positional argument is accepted as the schema path for
    // ergonomics: `validate out.json data/schemas/dole_order_schema.json`.
    match positional.len() {
        1 => Ok(Some(Cli {
            data: PathBuf::from(&positional[0]),
            schema,
        })),
        2 if schema.is_none() => Ok(Some(Cli {
            data: PathBuf::from(&positional[0]),
            schema: Some(PathBuf::from(&positional[1])),
        })),
        _ => bail!(USAGE),
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,lopdf=warn")),
        )
        .init();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let Some(cli) = parse_args()? else {
        println!("{USAGE}");
        return Ok(());
    };

    let data_raw = std::fs::read_to_string(&cli.data)
        .with_context(|| format!("failed to read {}", cli.data.display()))?;
    let data: Value = serde_json::from_str(&data_raw)
        .with_context(|| format!("{} is not valid JSON", cli.data.display()))?;

    let schema_path = match cli.schema {
        Some(path) => path,
        None => {
            let name = infer_schema_name(&data)?;
            let path = locate_schema(name)?;
            info!(schema = %path.display(), "inferred schema from document shape");
            path
        }
    };

    let schema_raw = std::fs::read_to_string(&schema_path)
        .with_context(|| format!("failed to read schema {}", schema_path.display()))?;
    let schema: Value = serde_json::from_str(&schema_raw)
        .with_context(|| format!("{} is not valid JSON", schema_path.display()))?;

    let compiled = JSONSchema::options()
        .with_draft(Draft::Draft7)
        .compile(&schema)
        .map_err(|e| anyhow!("failed to compile schema {}: {e}", schema_path.display()))?;

    // The schemas describe a single document. A top-level array (e.g. the
    // Labor Code output, one object per article) is validated element-wise,
    // in parallel — `JSONSchema` is `Sync`, so the compiled schema is shared
    // across rayon workers.
    let (errors, instance_count) = match &data {
        Value::Array(items) => {
            let errors: Vec<String> = items
                .par_iter()
                .enumerate()
                .flat_map_iter(|(index, item)| {
                    collect_errors(&compiled, item)
                        .into_iter()
                        .map(move |e| format!("[{index}] {e}"))
                })
                .collect();
            (errors, items.len())
        }
        _ => (collect_errors(&compiled, &data), 1),
    };

    if errors.is_empty() {
        println!(
            "OK: {} — {instance_count} instance(s) valid against {}",
            cli.data.display(),
            schema_path.display()
        );
        Ok(())
    } else {
        for error in &errors {
            eprintln!("INVALID: {error}");
        }
        bail!(
            "{} failed validation: {} error(s) across {instance_count} instance(s)",
            cli.data.display(),
            errors.len()
        );
    }
}

/// Run validation and render every error with its instance path.
fn collect_errors(schema: &JSONSchema, instance: &Value) -> Vec<String> {
    match schema.validate(instance) {
        Ok(()) => Vec::new(),
        Err(errors) => errors
            .map(|e| {
                let path = if e.instance_path.to_string().is_empty() {
                    "/".to_string()
                } else {
                    e.instance_path.to_string()
                };
                format!("at {path}: {e}")
            })
            .collect(),
    }
}

/// Infer which schema applies from the document's discriminating fields.
/// For arrays the first element is probed (outputs are homogeneous).
fn infer_schema_name(data: &Value) -> Result<&'static str> {
    let probe = match data {
        Value::Array(items) => items
            .first()
            .context("cannot infer schema for an empty array; pass --schema")?,
        other => other,
    };

    if probe.get("order_number").is_some() {
        Ok("dole_order_schema.json")
    } else if probe.get("article_number").is_some() {
        Ok("labor_code_schema.json")
    } else if probe.get("question").is_some() {
        Ok("benchmark_question_schema.json")
    } else {
        bail!(
            "could not infer a schema from the document shape \
             (no order_number / article_number / question field); pass --schema"
        )
    }
}

/// Locate `data/schemas/<name>` in the current directory or the nearest
/// ancestor, so the binary works from the workspace root or any crate
/// subdirectory.
fn locate_schema(name: &str) -> Result<PathBuf> {
    let relative = Path::new("data").join("schemas").join(name);
    find_upward(&relative).with_context(|| {
        format!(
            "schema {name} not found in any data/schemas/ directory above the current \
             directory; pass --schema"
        )
    })
}
