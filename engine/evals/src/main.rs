//! Benchmark command line.
//!
//! ```text
//! evals check                         check questions against the corpus
//! evals retrieve --out <report.json>  measure the BM25 baseline
//! evals run --mode rag --out <a.jsonl>  collect model answers
//! evals score <a.jsonl>               grade model answers
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use labor_evals::corpus::Corpus;
use labor_evals::llm::{Client, Provider, DEFAULT_CLAUDE_MODEL};
use labor_evals::questions::{self, DEFAULT_QUESTIONS_PATH};
use labor_evals::retrieval;
use labor_evals::runner::{self, Mode, RunOptions};
use labor_evals::scoring::{score_answer, summarize, AnswerScore, Summary};
use serde::Serialize;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "evals", about = "Benchmark models on Philippine labor law")]
struct Cli {
    #[command(flatten)]
    data: DataArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct DataArgs {
    /// Benchmark questions
    #[arg(long, global = true, default_value = DEFAULT_QUESTIONS_PATH)]
    questions: PathBuf,

    /// Directory of processed records
    #[arg(long, global = true, default_value = "data/processed")]
    processed: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Check every question against the corpus
    Check,

    /// Measure the BM25 baseline
    Retrieve {
        /// Write the report here instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Ask a model every question. This calls a paid API unless pointed at a
    /// local server.
    Run(RunArgs),

    /// Grade an answers file
    Score {
        /// JSONL file written by `evals run`
        answers: PathBuf,

        /// Also write one row per question
        #[arg(long)]
        csv: Option<PathBuf>,
    },
}

#[derive(Args)]
struct RunArgs {
    #[arg(long, value_enum, default_value = "anthropic")]
    provider: Provider,

    /// Model id. Required for openai-compatible.
    #[arg(long)]
    model: Option<String>,

    #[arg(long, value_enum)]
    mode: Mode,

    /// Answers file. An existing file is resumed.
    #[arg(long)]
    out: PathBuf,

    /// Passages shown in rag mode
    #[arg(long, default_value_t = 5)]
    top_k: usize,

    /// Stop after this many new answers
    #[arg(long)]
    limit: Option<usize>,

    /// Claude effort level, or "default" to send none
    #[arg(long, default_value = "high")]
    effort: String,

    /// API endpoint. Required for openai-compatible.
    #[arg(long)]
    base_url: Option<String>,

    /// Environment variable holding the API key
    #[arg(long)]
    api_key_env: Option<String>,
}

fn write_or_print<T: Serialize>(value: &T, out: Option<&Path>) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    match out {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, json + "\n")
                .with_context(|| format!("failed to write {}", path.display()))
        }
        None => {
            println!("{json}");
            Ok(())
        }
    }
}

fn client(args: &RunArgs) -> Result<Client> {
    let model = match (&args.model, args.provider) {
        (Some(model), _) => model.clone(),
        (None, Provider::Anthropic) => DEFAULT_CLAUDE_MODEL.to_string(),
        (None, Provider::OpenaiCompatible) => bail!("--model is required for openai-compatible"),
    };
    let base_url = match (&args.base_url, Client::default_base_url(args.provider)) {
        (Some(url), _) => url.clone(),
        (None, Some(url)) => url.to_string(),
        (None, None) => bail!("--base-url is required for openai-compatible"),
    };
    let key_var = args.api_key_env.clone().or(match args.provider {
        Provider::Anthropic => Some("ANTHROPIC_API_KEY".to_string()),
        Provider::OpenaiCompatible => None,
    });
    let api_key = match key_var {
        Some(var) => Some(std::env::var(&var).with_context(|| format!("{var} is not set"))?),
        None => None,
    };
    let effort = match (args.provider, args.effort.as_str()) {
        (Provider::Anthropic, "default") | (Provider::OpenaiCompatible, _) => None,
        (Provider::Anthropic, level) => Some(level.to_string()),
    };
    Ok(Client {
        provider: args.provider,
        base_url,
        api_key,
        model,
        effort,
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()?,
        backoff: Duration::from_secs(2),
    })
}

/// A graded run.
#[derive(Serialize)]
struct ScoreReport {
    model: String,
    mode: Mode,
    questions_in_benchmark: usize,
    /// Answers that ended in a refusal; they score as wrong.
    refused: usize,
    /// Answers cut off by the token limit; they score as written.
    truncated: usize,
    input_tokens: u64,
    output_tokens: u64,
    #[serde(flatten)]
    summary: Summary,
}

fn score(
    questions: &[labor_ingestion::BenchmarkQuestion],
    corpus: &Corpus,
    answers: &Path,
    csv_path: Option<&Path>,
) -> Result<()> {
    let records = runner::load_answers(answers)?;
    let first = records.first().context("answers file is empty")?;
    let by_id: HashMap<&str, _> = questions.iter().map(|q| (q.id.as_str(), q)).collect();

    let mut scored: Vec<(Vec<String>, AnswerScore)> = Vec::with_capacity(records.len());
    let mut csv = csv_path.map(csv::Writer::from_path).transpose()?;
    if let Some(csv) = &mut csv {
        csv.write_record([
            "question_id",
            "pass",
            "key_points_hit",
            "key_points_total",
            "citations_hit",
            "citations_total",
            "stop_reason",
            "missing_key_points",
        ])?;
    }
    for record in &records {
        let question = by_id
            .get(record.question_id.as_str())
            .with_context(|| format!("unknown question {}", record.question_id))?;
        let grade = score_answer(question, &record.answer, corpus);
        if let Some(csv) = &mut csv {
            csv.write_record([
                record.question_id.as_str(),
                if grade.pass { "true" } else { "false" },
                &grade.key_points_hit.to_string(),
                &grade.key_points_total.to_string(),
                &grade.citations_hit.to_string(),
                &grade.citations_total.to_string(),
                &record.stop_reason,
                &grade.missing_key_points.join("; "),
            ])?;
        }
        scored.push((questions::groups(question), grade));
    }
    if let Some(csv) = &mut csv {
        csv.flush()?;
    }

    let count = |reason: &[&str]| {
        records
            .iter()
            .filter(|r| reason.contains(&r.stop_reason.as_str()))
            .count()
    };
    write_or_print(
        &ScoreReport {
            model: first.model.clone(),
            mode: first.mode,
            questions_in_benchmark: questions.len(),
            refused: count(&["refusal", "content_filter"]),
            truncated: count(&["max_tokens", "length"]),
            input_tokens: records.iter().map(|r| r.input_tokens).sum(),
            output_tokens: records.iter().map(|r| r.output_tokens).sum(),
            summary: summarize(scored.iter().map(|(groups, grade)| (groups.clone(), grade))),
        },
        None,
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("labor_evals=info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let questions = questions::load(&cli.data.questions)?;
    let corpus = Corpus::load(&cli.data.processed)?;

    match cli.command {
        Command::Check => {
            let problems = questions::check(&questions, &corpus);
            if !problems.is_empty() {
                bail!("benchmark has problems:\n{}", problems.join("\n"));
            }
            println!(
                "{} questions check out against {} passages",
                questions.len(),
                corpus.passages.len()
            );
            Ok(())
        }
        Command::Retrieve { out } => {
            write_or_print(&retrieval::evaluate(&corpus, &questions), out.as_deref())
        }
        Command::Run(args) => {
            let client = client(&args)?;
            let options = RunOptions {
                mode: args.mode,
                top_k: args.top_k,
                limit: args.limit,
            };
            let added = runner::run(&client, &questions, &corpus, &options, &args.out).await?;
            println!("{added} answers added to {}", args.out.display());
            Ok(())
        }
        Command::Score { answers, csv } => score(&questions, &corpus, &answers, csv.as_deref()),
    }
}
