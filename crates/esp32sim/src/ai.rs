//! AI assistant providers (streaming chat).

use crate::ide::AiSettings;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc::Sender;

pub const SYSTEM_PROMPT: &str = "You are an expert embedded-systems assistant built into esp32sim, an IDE + emulator for ESP32 boards. \
The user writes Arduino / PlatformIO (C++) or ESP-IDF firmware that runs on an emulated ESP32 wired to a simulated circuit. \
You receive the circuit (YAML: components and connections between component pins and esp32 pins), the current file, build errors and the serial log. \
Be concrete and correct about ESP32 pins (input-only GPIO34-39, strapping pins 0/2/12/15, ADC2 unusable with WiFi, flash pins 6-11). \
When you propose a change to a file, return the COMPLETE new file in a fenced block whose info string is the language followed by file=<relative path>, \
for example ```cpp file=src/main.cpp. When you propose circuit changes, return the complete project YAML in ```yaml file=esp32sim.yaml. Keep explanations short.";

pub fn default_model(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "claude-sonnet-4-5",
        "gemini" => "gemini-2.5-flash",
        "ollama" => "qwen2.5-coder",
        _ => "gpt-4o-mini",
    }
}

fn base_url(s: &AiSettings) -> String {
    if !s.base_url.trim().is_empty() {
        return s.base_url.trim().trim_end_matches('/').to_string();
    }
    match s.provider.as_str() {
        "anthropic" => "https://api.anthropic.com".into(),
        "gemini" => "https://generativelanguage.googleapis.com".into(),
        "ollama" => "http://localhost:11434".into(),
        _ => "https://api.openai.com/v1".into(),
    }
}

fn key(s: &AiSettings) -> String {
    s.keys.get(&s.provider).cloned().unwrap_or_default()
}

/// List models available from the configured provider.
pub async fn list_models(s: &AiSettings) -> Result<Vec<String>, String> {
    let client = reqwest::Client::new();
    let base = base_url(s);
    let req = match s.provider.as_str() {
        "anthropic" => client.get(format!("{}/v1/models", base)).header("x-api-key", key(s)).header("anthropic-version", "2023-06-01"),
        "gemini" => client.get(format!("{}/v1beta/models?key={}", base, key(s))),
        "ollama" => client.get(format!("{}/api/tags", base)),
        _ => client.get(format!("{}/models", base)).bearer_auth(key(s)),
    };
    let v: Value = req.send().await.map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
    let mut out: Vec<String> = Vec::new();
    if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        out.extend(arr.iter().filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(|x| x.to_string())));
    }
    if let Some(arr) = v.get("models").and_then(|d| d.as_array()) {
        out.extend(arr.iter().filter_map(|m| m.get("name").and_then(|x| x.as_str()).map(|x| x.trim_start_matches("models/").to_string())));
    }
    if out.is_empty() {
        if let Some(e) = v.get("error") {
            return Err(e.to_string());
        }
    }
    out.sort();
    Ok(out)
}

/// Stream a chat completion; text chunks are sent to `tx`. `messages` are {role, content}.
pub async fn chat(s: &AiSettings, system: &str, messages: Vec<Value>, tx: Sender<String>) -> Result<(), String> {
    let client = reqwest::Client::new();
    let base = base_url(s);
    let model = if s.model.trim().is_empty() { default_model(&s.provider).to_string() } else { s.model.clone() };
    let temp = s.temperature.unwrap_or(0.2);
    let provider = if s.provider.is_empty() { "openai" } else { s.provider.as_str() };
    if provider != "ollama" && key(s).is_empty() && s.base_url.is_empty() {
        return Err(format!("no API key configured for '{}' (open AI settings)", provider));
    }
    let resp = match provider {
        "anthropic" => {
            let body = json!({"model": model, "max_tokens": 4096, "system": system, "messages": messages, "stream": true, "temperature": temp});
            client.post(format!("{}/v1/messages", base)).header("x-api-key", key(s)).header("anthropic-version", "2023-06-01").json(&body).send().await
        }
        "gemini" => {
            let contents: Vec<Value> = messages
                .iter()
                .map(|m| json!({"role": if m["role"] == "assistant" { "model" } else { "user" }, "parts": [{"text": m["content"]}]}))
                .collect();
            let body = json!({"contents": contents, "systemInstruction": {"parts": [{"text": system}]}, "generationConfig": {"temperature": temp}});
            client.post(format!("{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}", base, model, key(s))).json(&body).send().await
        }
        "ollama" => {
            let mut msgs = vec![json!({"role": "system", "content": system})];
            msgs.extend(messages);
            let body = json!({"model": model, "messages": msgs, "stream": true, "options": {"temperature": temp}});
            client.post(format!("{}/api/chat", base)).json(&body).send().await
        }
        _ => {
            let mut msgs = vec![json!({"role": "system", "content": system})];
            msgs.extend(messages);
            let body = json!({"model": model, "messages": msgs, "stream": true, "temperature": temp});
            let mut r = client.post(format!("{}/chat/completions", base)).json(&body);
            let k = key(s);
            if !k.is_empty() {
                r = r.bearer_auth(k);
            }
            r.send().await
        }
    }
    .map_err(|e| format!("request failed: {}", e))?;
    if !resp.status().is_success() {
        let st = resp.status();
        let t = resp.text().await.unwrap_or_default();
        return Err(format!("{} {}", st, t.chars().take(600).collect::<String>()));
    }
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf.drain(..=nl);
            if line.is_empty() {
                continue;
            }
            let data = line.strip_prefix("data:").map(|x| x.trim()).unwrap_or(&line);
            if data == "[DONE]" {
                return Ok(());
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            let text = match provider {
                "anthropic" => v.get("delta").and_then(|d| d.get("text")).and_then(|t| t.as_str()).map(|s| s.to_string()),
                "gemini" => v.pointer("/candidates/0/content/parts/0/text").and_then(|t| t.as_str()).map(|s| s.to_string()),
                "ollama" => v.pointer("/message/content").and_then(|t| t.as_str()).map(|s| s.to_string()),
                _ => v.pointer("/choices/0/delta/content").and_then(|t| t.as_str()).map(|s| s.to_string()),
            };
            if let Some(e) = v.get("error") {
                return Err(e.to_string());
            }
            if let Some(t) = text {
                if tx.send(t).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(provider: &str, base: &str) -> AiSettings {
        let mut s = AiSettings { provider: provider.into(), base_url: base.into(), ..Default::default() };
        s.keys.insert(provider.into(), "k".into());
        s
    }

    #[test]
    fn provider_defaults() {
        assert_eq!(default_model("anthropic"), "claude-sonnet-4-5");
        assert_eq!(default_model("gemini"), "gemini-2.5-flash");
        assert_eq!(default_model("ollama"), "qwen2.5-coder");
        assert_eq!(default_model("openai"), "gpt-4o-mini");
        assert_eq!(default_model("anything-else"), "gpt-4o-mini");
        assert_eq!(base_url(&settings("anthropic", "")), "https://api.anthropic.com");
        assert_eq!(base_url(&settings("gemini", "")), "https://generativelanguage.googleapis.com");
        assert_eq!(base_url(&settings("ollama", "")), "http://localhost:11434");
        assert_eq!(base_url(&settings("openai", "")), "https://api.openai.com/v1");
        assert_eq!(base_url(&settings("openai-compatible", " http://lm:1234/v1/ ")), "http://lm:1234/v1", "custom endpoints are trimmed");
        assert_eq!(key(&settings("openai", "")), "k");
        assert_eq!(key(&AiSettings { provider: "openai".into(), ..Default::default() }), "");
    }

    #[test]
    fn system_prompt_mentions_the_file_protocol() {
        assert!(SYSTEM_PROMPT.contains("file=<relative path>"));
        assert!(SYSTEM_PROMPT.contains("esp32sim.yaml"));
        assert!(SYSTEM_PROMPT.contains("GPIO34-39"));
    }

    #[tokio::test]
    async fn missing_key_is_an_error_before_any_request() {
        let s = AiSettings { provider: "anthropic".into(), ..Default::default() };
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let e = chat(&s, "sys", vec![], tx).await.unwrap_err();
        assert!(e.contains("no API key configured for 'anthropic'"), "{}", e);
    }
}
