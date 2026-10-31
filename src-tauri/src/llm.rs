//! One small client for three chat-completion shapes: Ollama, OpenAI-compatible,
//! and Anthropic. Streaming is delivered through a callback.

use crate::settings::{LlmConfig, Provider};
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Newer models (gpt-5.x, Claude 5) reject `temperature`; once a provider says
/// so, every later call leaves sampling parameters out.
static OPENAI_NO_SAMPLING: AtomicBool = AtomicBool::new(false);
static ANTHROPIC_NO_SAMPLING: AtomicBool = AtomicBool::new(false);

fn rejects_sampling(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("temperature") && (t.contains("unsupported") || t.contains("deprecated") || t.contains("not supported") || t.contains("does not support"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

pub fn msg(role: &str, content: impl Into<String>) -> Message {
    Message { role: role.into(), content: content.into() }
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(180)).build()?)
}

/// Non-streaming completion. With `json` the provider is asked for a JSON object.
pub async fn complete(cfg: &LlmConfig, system: &str, messages: &[Message], json: bool, temperature: f32) -> Result<String> {
    match cfg.provider {
        Provider::Ollama => ollama_complete(cfg, system, messages, json, temperature).await,
        Provider::OpenAI => openai_complete(cfg, system, messages, json, temperature).await,
        Provider::Anthropic => anthropic_complete(cfg, system, messages, json, temperature).await,
        Provider::Publik => publik_complete(cfg, system, messages, json, temperature).await,
    }
}

/// Streaming completion; `on_delta` is called with each text chunk. Returns the full text.
pub async fn stream<F: FnMut(&str)>(cfg: &LlmConfig, system: &str, messages: &[Message], temperature: f32, mut on_delta: F) -> Result<String> {
    match cfg.provider {
        Provider::Ollama => ollama_stream(cfg, system, messages, temperature, &mut on_delta).await,
        Provider::OpenAI => openai_stream(cfg, system, messages, temperature, &mut on_delta).await,
        Provider::Anthropic => anthropic_stream(cfg, system, messages, temperature, &mut on_delta).await,
        Provider::Publik => publik_stream(cfg, system, messages, temperature, &mut on_delta).await,
    }
}

/// Ask for a JSON object and parse it, tolerating prose around the braces.
pub async fn complete_json(cfg: &LlmConfig, system: &str, user: &str, temperature: f32) -> Result<Value> {
    let messages = vec![msg("user", user)];
    let mut last_err = None;
    for attempt in 0..2 {
        let sys = if attempt == 0 { system.to_string() } else { format!("{system}\nReturn ONLY a valid JSON object. No prose, no markdown fences.") };
        match complete(cfg, &sys, &messages, true, temperature).await {
            Ok(text) => match extract_json(&text) {
                Some(v) => return Ok(v),
                None => last_err = Some(anyhow!("model did not return JSON: {}", text.chars().take(200).collect::<String>())),
            },
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("no response")))
}

pub fn extract_json(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        if v.is_object() || v.is_array() {
            return Some(v);
        }
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&text[start..=end]).ok()
}

fn with_system(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut v = Vec::with_capacity(messages.len() + 1);
    if !system.is_empty() {
        v.push(json!({"role": "system", "content": system}));
    }
    for m in messages {
        v.push(json!({"role": m.role, "content": m.content}));
    }
    v
}

fn base(cfg: &LlmConfig) -> String {
    cfg.base_url.trim_end_matches('/').to_string()
}

// ---------------- Ollama ----------------

async fn ollama_complete(cfg: &LlmConfig, system: &str, messages: &[Message], json: bool, temperature: f32) -> Result<String> {
    let mut body = json!({
        "model": cfg.model,
        "messages": with_system(system, messages),
        "stream": false,
        // Thinking models (qwen3, glm, deepseek-r1) would reason at length first; edits do not need it.
        "think": false,
        "options": {"temperature": temperature, "num_ctx": 8192},
    });
    if json {
        body["format"] = json!("json");
    }
    let res = client()?.post(format!("{}/api/chat", base(cfg))).json(&body).send().await.context("Ollama is not reachable")?;
    let status = res.status();
    let v: Value = res.json().await?;
    if !status.is_success() {
        return Err(anyhow!("Ollama error {}: {}", status, v));
    }
    Ok(v["message"]["content"].as_str().unwrap_or("").to_string())
}

async fn ollama_stream<F: FnMut(&str)>(cfg: &LlmConfig, system: &str, messages: &[Message], temperature: f32, on_delta: &mut F) -> Result<String> {
    let body = json!({
        "model": cfg.model,
        "messages": with_system(system, messages),
        "stream": true,
        "think": false,
        "options": {"temperature": temperature, "num_ctx": 8192},
    });
    let res = client()?.post(format!("{}/api/chat", base(cfg))).json(&body).send().await.context("Ollama is not reachable")?;
    if !res.status().is_success() {
        let t = res.text().await.unwrap_or_default();
        return Err(anyhow!("Ollama error: {t}"));
    }
    let mut full = String::new();
    let mut buf = String::new();
    let mut s = res.bytes_stream();
    while let Some(chunk) = s.next().await {
        let chunk = chunk?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if let Some(t) = v["message"]["content"].as_str() {
                    if !t.is_empty() {
                        full.push_str(t);
                        on_delta(t);
                    }
                }
                if v["done"].as_bool() == Some(true) {
                    return Ok(full);
                }
            }
        }
    }
    Ok(full)
}

pub async fn ollama_models(base_url: &str) -> Result<Vec<String>> {
    let v: Value = client()?.get(format!("{}/api/tags", base_url.trim_end_matches('/'))).send().await?.json().await?;
    Ok(v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect()).unwrap_or_default())
}

// ---------------- OpenAI-compatible ----------------

fn openai_url(cfg: &LlmConfig) -> String {
    openai_endpoint(&base(cfg), "chat/completions")
}

/// OpenAI-compatible roots differ: `https://api.openai.com` and
/// `https://api.groq.com/openai` want `/v1/…` appended, OpenRouter's root
/// already ends in `/v1`, Google's compatibility root is `…/v1beta/openai`.
fn openai_endpoint(base: &str, path: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.ends_with("/v1") || b.contains("/v1beta") {
        format!("{b}/{path}")
    } else {
        format!("{b}/v1/{path}")
    }
}

/// `GET /models` on an OpenAI-compatible root (ids only).
pub async fn openai_models(cfg: &LlmConfig) -> Result<Vec<String>> {
    let res = client()?.get(openai_endpoint(&base(cfg), "models")).bearer_auth(&cfg.api_key).send().await?;
    let status = res.status();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(anyhow!("API error {}: {}", status, v["error"]["message"].as_str().unwrap_or("")));
    }
    Ok(v["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect()).unwrap_or_default())
}

/// `GET /v1/models` on Anthropic (ids only).
pub async fn anthropic_models(cfg: &LlmConfig) -> Result<Vec<String>> {
    let b = if cfg.base_url.contains("api.anthropic.com") || cfg.base_url.trim().is_empty() { "https://api.anthropic.com".to_string() } else { base(cfg) };
    let res = client()?.get(format!("{}/v1/models?limit=100", b.trim_end_matches('/'))).header("x-api-key", &cfg.api_key).header("anthropic-version", "2023-06-01").send().await?;
    let status = res.status();
    let v: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(anyhow!("Anthropic error {}: {}", status, v["error"]["message"].as_str().unwrap_or("")));
    }
    Ok(v["data"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect()).unwrap_or_default())
}

async fn openai_complete(cfg: &LlmConfig, system: &str, messages: &[Message], json: bool, temperature: f32) -> Result<String> {
    for attempt in 0..2 {
        let mut body = json!({"model": cfg.model, "messages": with_system(system, messages)});
        if !OPENAI_NO_SAMPLING.load(Ordering::Relaxed) {
            body["temperature"] = json!(temperature);
        }
        if json {
            body["response_format"] = json!({"type": "json_object"});
        }
        let res = client()?.post(openai_url(cfg)).bearer_auth(&cfg.api_key).json(&body).send().await?;
        let status = res.status();
        let v: Value = res.json().await?;
        if !status.is_success() {
            if attempt == 0 && rejects_sampling(&v.to_string()) {
                OPENAI_NO_SAMPLING.store(true, Ordering::Relaxed);
                continue;
            }
            return Err(anyhow!("API error {}: {}", status, v["error"]["message"].as_str().unwrap_or(&v.to_string())));
        }
        return Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string());
    }
    unreachable!()
}

async fn openai_stream<F: FnMut(&str)>(cfg: &LlmConfig, system: &str, messages: &[Message], temperature: f32, on_delta: &mut F) -> Result<String> {
    let mut res = None;
    for attempt in 0..2 {
        let mut body = json!({"model": cfg.model, "messages": with_system(system, messages), "stream": true});
        if !OPENAI_NO_SAMPLING.load(Ordering::Relaxed) {
            body["temperature"] = json!(temperature);
        }
        let r = client()?.post(openai_url(cfg)).bearer_auth(&cfg.api_key).json(&body).send().await?;
        if !r.status().is_success() {
            let t = r.text().await.unwrap_or_default();
            if attempt == 0 && rejects_sampling(&t) {
                OPENAI_NO_SAMPLING.store(true, Ordering::Relaxed);
                continue;
            }
            return Err(anyhow!("API error: {t}"));
        }
        res = Some(r);
        break;
    }
    let res = res.ok_or_else(|| anyhow!("no response"))?;
    let mut full = String::new();
    let mut buf = String::new();
    let mut s = res.bytes_stream();
    while let Some(chunk) = s.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk?));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(full);
                }
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                        full.push_str(t);
                        on_delta(t);
                    }
                }
            }
        }
    }
    Ok(full)
}

// ---------------- publik API ----------------
// OpenAI Chat Completions on the gateway root; the `x-publik-*` headers carry
// the balance, and a 402/401/429/503 is a typed outcome the panel renders as
// the gateway's message plus exactly one link.

fn publik_body(cfg: &LlmConfig, system: &str, messages: &[Message], json: bool, temperature: f32, stream: bool) -> Value {
    let mut body = json!({"model": cfg.model, "messages": with_system(system, messages), "temperature": temperature});
    if json {
        body["response_format"] = json!({"type": "json_object"});
    }
    if stream {
        body["stream"] = json!(true);
    }
    body
}

fn publik_fail(status: u16, headers: &reqwest::header::HeaderMap, text: &str) -> anyhow::Error {
    let e = crate::publik::map_error(status, headers, text, None);
    crate::publik::note_error(&e);
    anyhow!("{}", e.message)
}

async fn publik_complete(cfg: &LlmConfig, system: &str, messages: &[Message], json: bool, temperature: f32) -> Result<String> {
    let url = format!("{}/chat/completions", base(cfg));
    let res = match client()?.post(&url).bearer_auth(&cfg.api_key).json(&publik_body(cfg, system, messages, json, temperature, false)).send().await {
        Ok(r) => r,
        Err(e) => {
            crate::publik::note_transport_error(&e.to_string());
            return Err(anyhow!("Could not reach publik API ({e}). Nothing was charged."));
        }
    };
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    let text = res.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(publik_fail(status, &headers, &text));
    }
    crate::publik::note_response(&headers);
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string())
}

async fn publik_stream<F: FnMut(&str)>(cfg: &LlmConfig, system: &str, messages: &[Message], temperature: f32, on_delta: &mut F) -> Result<String> {
    let url = format!("{}/chat/completions", base(cfg));
    let res = match client()?.post(&url).bearer_auth(&cfg.api_key).json(&publik_body(cfg, system, messages, false, temperature, true)).send().await {
        Ok(r) => r,
        Err(e) => {
            crate::publik::note_transport_error(&e.to_string());
            return Err(anyhow!("Could not reach publik API ({e}). Nothing was charged."));
        }
    };
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    if !(200..300).contains(&status) {
        let t = res.text().await.unwrap_or_default();
        return Err(publik_fail(status, &headers, &t));
    }
    crate::publik::note_response(&headers);
    let mut full = String::new();
    let mut buf = String::new();
    let mut s = res.bytes_stream();
    while let Some(chunk) = s.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk?));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            if let Some(data) = line.strip_prefix("data:") {
                let data = data.trim();
                if data == "[DONE]" {
                    return Ok(full);
                }
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                        full.push_str(t);
                        on_delta(t);
                    }
                }
            }
        }
    }
    Ok(full)
}

// ---------------- Anthropic ----------------

fn anthropic_url(cfg: &LlmConfig) -> String {
    let b = if cfg.base_url.contains("api.anthropic.com") || cfg.base_url.trim().is_empty() { "https://api.anthropic.com".to_string() } else { base(cfg) };
    format!("{}/v1/messages", b.trim_end_matches('/'))
}

fn anthropic_messages(messages: &[Message]) -> Vec<Value> {
    messages.iter().map(|m| json!({"role": if m.role == "assistant" { "assistant" } else { "user" }, "content": m.content})).collect()
}

async fn anthropic_complete(cfg: &LlmConfig, system: &str, messages: &[Message], json: bool, temperature: f32) -> Result<String> {
    let sys = if json { format!("{system}\nRespond with a single JSON object and nothing else.") } else { system.to_string() };
    for attempt in 0..2 {
        let mut body = json!({"model": cfg.model, "max_tokens": 4096, "system": sys, "messages": anthropic_messages(messages)});
        if !ANTHROPIC_NO_SAMPLING.load(Ordering::Relaxed) {
            body["temperature"] = json!(temperature);
        }
        let res = client()?
            .post(anthropic_url(cfg))
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;
        let status = res.status();
        let v: Value = res.json().await?;
        if !status.is_success() {
            if attempt == 0 && rejects_sampling(&v.to_string()) {
                ANTHROPIC_NO_SAMPLING.store(true, Ordering::Relaxed);
                continue;
            }
            return Err(anyhow!("Anthropic error {}: {}", status, v["error"]["message"].as_str().unwrap_or(&v.to_string())));
        }
        return Ok(v["content"][0]["text"].as_str().unwrap_or("").to_string());
    }
    unreachable!()
}

async fn anthropic_stream<F: FnMut(&str)>(cfg: &LlmConfig, system: &str, messages: &[Message], temperature: f32, on_delta: &mut F) -> Result<String> {
    let mut res = None;
    for attempt in 0..2 {
        let mut body = json!({"model": cfg.model, "max_tokens": 4096, "system": system, "messages": anthropic_messages(messages), "stream": true});
        if !ANTHROPIC_NO_SAMPLING.load(Ordering::Relaxed) {
            body["temperature"] = json!(temperature);
        }
        let r = client()?
            .post(anthropic_url(cfg))
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;
        if !r.status().is_success() {
            let t = r.text().await.unwrap_or_default();
            if attempt == 0 && rejects_sampling(&t) {
                ANTHROPIC_NO_SAMPLING.store(true, Ordering::Relaxed);
                continue;
            }
            return Err(anyhow!("Anthropic error: {t}"));
        }
        res = Some(r);
        break;
    }
    let res = res.ok_or_else(|| anyhow!("no response"))?;
    let mut full = String::new();
    let mut buf = String::new();
    let mut s = res.bytes_stream();
    while let Some(chunk) = s.next().await {
        buf.push_str(&String::from_utf8_lossy(&chunk?));
        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim().to_string();
            buf.drain(..=pos);
            if let Some(data) = line.strip_prefix("data:") {
                if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                    if v["type"] == "content_block_delta" {
                        if let Some(t) = v["delta"]["text"].as_str() {
                            full.push_str(t);
                            on_delta(t);
                        }
                    }
                    if v["type"] == "message_stop" {
                        return Ok(full);
                    }
                }
            }
        }
    }
    Ok(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_tolerates_prose_and_fences() {
        let v = extract_json("Sure! ```json\n{\"a\": 1}\n```").unwrap();
        assert_eq!(v["a"], 1);
        assert!(extract_json("no braces here").is_none());
        let v = extract_json("{\"x\":[1,2]}").unwrap();
        assert_eq!(v["x"][1], 2);
    }

    #[test]
    fn sampling_rejections_are_recognised() {
        assert!(rejects_sampling(r#"{"error":{"message":"`temperature` is deprecated for this model.","type":"invalid_request_error"}}"#));
        assert!(rejects_sampling(r#"{"error":{"message":"Unsupported value: 'temperature' does not support 0.2 with this model."}}"#));
        assert!(!rejects_sampling(r#"{"error":{"message":"Rate limit reached"}}"#));
    }

    #[test]
    fn openai_url_handles_v1_suffix() {
        let mut c = LlmConfig { provider: Provider::OpenAI, base_url: "https://openrouter.ai/api/v1".into(), api_key: "".into(), model: "m".into() };
        assert_eq!(openai_url(&c), "https://openrouter.ai/api/v1/chat/completions");
        c.base_url = "https://api.openai.com".into();
        assert_eq!(openai_url(&c), "https://api.openai.com/v1/chat/completions");
        c.base_url = "https://api.groq.com/openai".into();
        assert_eq!(openai_url(&c), "https://api.groq.com/openai/v1/chat/completions");
        c.base_url = "https://generativelanguage.googleapis.com/v1beta/openai".into();
        assert_eq!(openai_url(&c), "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions");
        assert_eq!(openai_endpoint("https://openrouter.ai/api/v1/", "models"), "https://openrouter.ai/api/v1/models");
    }
}
