//! HTTP clients for the models under test.
//!
//! Rust has no official Anthropic SDK, so both clients are plain HTTP. Claude
//! requests send no `temperature`, which current models reject, and no
//! refusal `fallbacks`, which would answer with a model other than the one
//! being scored. A refusal is recorded as a refusal.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::warn;

const ANTHROPIC_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 16_000;
const MAX_ATTEMPTS: u32 = 5;

/// Model used for Claude runs when none is named.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5-5";

/// API dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    /// Claude Messages API
    Anthropic,
    /// Any server with a `/chat/completions` endpoint, local or hosted
    OpenaiCompatible,
}

/// One model reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub stop_reason: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A client bound to one model.
#[derive(Debug, Clone)]
pub struct Client {
    pub provider: Provider,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    /// Claude `output_config.effort`; `None` leaves the model's default.
    pub effort: Option<String>,
    pub http: reqwest::Client,
    /// Base delay between retries.
    pub backoff: Duration,
}

impl Client {
    /// Default endpoint for a provider.
    pub fn default_base_url(provider: Provider) -> Option<&'static str> {
        match provider {
            Provider::Anthropic => Some(ANTHROPIC_URL),
            Provider::OpenaiCompatible => None,
        }
    }

    fn request(&self, system: &str, user: &str) -> reqwest::RequestBuilder {
        let base = self.base_url.trim_end_matches('/');
        match self.provider {
            Provider::Anthropic => {
                let mut body = json!({
                    "model": self.model,
                    "max_tokens": MAX_TOKENS,
                    "system": system,
                    "messages": [{"role": "user", "content": user}],
                });
                if let Some(effort) = &self.effort {
                    body["output_config"] = json!({"effort": effort});
                }
                self.http
                    .post(format!("{base}/v1/messages"))
                    .header("x-api-key", self.api_key.as_deref().unwrap_or_default())
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .json(&body)
            }
            Provider::OpenaiCompatible => {
                let body = json!({
                    "model": self.model,
                    "max_tokens": MAX_TOKENS,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user},
                    ],
                });
                let request = self.http.post(format!("{base}/chat/completions"));
                match &self.api_key {
                    Some(key) => request.bearer_auth(key).json(&body),
                    None => request.json(&body),
                }
            }
        }
    }

    /// Ask the model once, retrying rate limits, server errors and dropped
    /// connections. Any other failure stops the run.
    pub async fn complete(&self, system: &str, user: &str) -> Result<Completion> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let delay = match self.request(system, user).send().await {
                Ok(response) => {
                    let status = response.status();
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse().ok())
                        .map(Duration::from_secs);
                    let text = response.text().await.context("failed to read response")?;
                    if status.is_success() {
                        let value: Value = serde_json::from_str(&text)
                            .with_context(|| format!("invalid JSON from model: {text}"))?;
                        return parse_completion(self.provider, &value);
                    }
                    let retryable = status.as_u16() == 429 || status.is_server_error();
                    if !retryable || attempt >= MAX_ATTEMPTS {
                        bail!("model request failed with {status}: {text}");
                    }
                    warn!(%status, attempt, "retrying model request");
                    retry_after.unwrap_or(self.backoff * 2u32.pow(attempt - 1))
                }
                Err(error) if attempt < MAX_ATTEMPTS => {
                    warn!(%error, attempt, "retrying model request");
                    self.backoff * 2u32.pow(attempt - 1)
                }
                Err(error) => return Err(error).context("model request failed"),
            };
            tokio::time::sleep(delay).await;
        }
    }
}

fn parse_completion(provider: Provider, value: &Value) -> Result<Completion> {
    let count = |v: &Value| v.as_u64().unwrap_or(0);
    match provider {
        Provider::Anthropic => Ok(Completion {
            // Thinking blocks and any others carry no answer text.
            text: value["content"]
                .as_array()
                .ok_or_else(|| anyhow!("no content in {value}"))?
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect(),
            stop_reason: value["stop_reason"].as_str().unwrap_or("").to_string(),
            input_tokens: count(&value["usage"]["input_tokens"]),
            output_tokens: count(&value["usage"]["output_tokens"]),
        }),
        Provider::OpenaiCompatible => {
            let choice = &value["choices"][0];
            if choice.is_null() {
                bail!("no choices in {value}");
            }
            Ok(Completion {
                text: choice["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                stop_reason: choice["finish_reason"].as_str().unwrap_or("").to_string(),
                input_tokens: count(&value["usage"]["prompt_tokens"]),
                output_tokens: count(&value["usage"]["completion_tokens"]),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serve the given (status, body) replies in order, keeping each request.
    async fn serve(replies: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, body) in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let read = socket.read(&mut buffer).await.unwrap();
                    request.extend_from_slice(&buffer[..read]);
                    let text = String::from_utf8_lossy(&request);
                    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let length: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.parse().unwrap())
                        })
                        .unwrap_or(0);
                    if rest.len() >= length {
                        break;
                    }
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request).to_string());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        (url, seen)
    }

    fn client(provider: Provider, url: String) -> Client {
        Client {
            provider,
            base_url: url,
            api_key: Some("key".to_string()),
            model: "model-x".to_string(),
            effort: Some("high".to_string()),
            http: reqwest::Client::new(),
            backoff: Duration::ZERO,
        }
    }

    fn claude_reply() -> String {
        json!({
            "content": [
                {"type": "thinking", "thinking": ""},
                {"type": "text", "text": "Six "},
                {"type": "text", "text": "months."},
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 4},
        })
        .to_string()
    }

    #[tokio::test]
    async fn test_claude_request_and_reply() {
        let (url, seen) = serve(vec![(200, claude_reply())]).await;
        let reply = client(Provider::Anthropic, url)
            .complete("sys", "user")
            .await
            .unwrap();
        assert_eq!(reply.text, "Six months.");
        assert_eq!((reply.input_tokens, reply.output_tokens), (10, 4));

        let request = seen.lock().unwrap()[0].clone();
        assert!(request.starts_with("POST /v1/messages"));
        let lower = request.to_lowercase();
        assert!(lower.contains("x-api-key: key"));
        assert!(lower.contains("anthropic-version: 2023-06-01"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["system"], "sys");
        assert_eq!(body["output_config"]["effort"], "high");
        assert!(body.get("temperature").is_none());
        assert!(body.get("fallbacks").is_none());
    }

    #[tokio::test]
    async fn test_refusal_is_returned_not_an_error() {
        let body = json!({"content": [], "stop_reason": "refusal", "usage": {}}).to_string();
        let (url, _) = serve(vec![(200, body)]).await;
        let reply = client(Provider::Anthropic, url)
            .complete("s", "u")
            .await
            .unwrap();
        assert_eq!(reply.stop_reason, "refusal");
        assert_eq!(reply.text, "");
    }

    #[tokio::test]
    async fn test_openai_compatible_request_and_reply() {
        let body = json!({
            "choices": [{"message": {"content": "Hi"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1},
        })
        .to_string();
        let (url, seen) = serve(vec![(200, body)]).await;
        let reply = client(Provider::OpenaiCompatible, url)
            .complete("s", "u")
            .await
            .unwrap();
        assert_eq!((reply.text.as_str(), reply.input_tokens), ("Hi", 3));
        let request = seen.lock().unwrap()[0].to_lowercase();
        assert!(request.starts_with("post /chat/completions"));
        assert!(request.contains("authorization: bearer key"));
    }

    #[tokio::test]
    async fn test_rate_limit_and_server_error_are_retried() {
        let (url, seen) = serve(vec![
            (429, "slow down".to_string()),
            (500, "oops".to_string()),
            (200, claude_reply()),
        ])
        .await;
        let reply = client(Provider::Anthropic, url)
            .complete("s", "u")
            .await
            .unwrap();
        assert_eq!(reply.text, "Six months.");
        assert_eq!(seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn test_bad_request_stops_without_retry() {
        let (url, seen) = serve(vec![(400, "bad".to_string())]).await;
        let error = client(Provider::Anthropic, url)
            .complete("s", "u")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("400"), "{error}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}
