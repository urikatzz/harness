use crate::interrupt;
use crate::error::{Error, Result};
use crate::provider::{Message, Provider, Reply};
use serde_json::{json, Value};
use std::time::Duration;

/// HTTP client with timeouts so a dead or overloaded server can't hang the CLI forever.
/// The read timeout is generous: a local server may need minutes to load a model.
fn http(read_timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(read_timeout)
        .build()
}

/// How a server wants reasoning effort in the request body.
#[derive(Clone, Copy)]
pub enum EffortStyle {
    /// OpenRouter: `"reasoning": {"effort": "high"}`
    OpenRouter,
    /// OpenAI / LM Studio / Ollama: `"reasoning_effort": "high"`
    OpenAi,
}

/// Any server speaking the OpenAI `/chat/completions` format:
/// OpenRouter, LM Studio, Ollama, llama.cpp, vLLM, OpenAI itself.
pub struct OpenAiCompatible {
    base_url: String,
    api_key: Option<String>,
    model: String,
    effort: Option<String>,
    effort_style: EffortStyle,
}

impl OpenAiCompatible {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
        effort: Option<String>,
        effort_style: EffortStyle,
    ) -> Self {
        Self { base_url: base_url.into(), api_key, model: model.into(), effort, effort_style }
    }

    fn request_body(&self, messages: Vec<Value>) -> Value {
        let mut body = json!({ "model": self.model, "messages": messages });
        if let Some(e) = &self.effort {
            match self.effort_style {
                EffortStyle::OpenRouter => body["reasoning"] = json!({ "effort": e }),
                EffortStyle::OpenAi => body["reasoning_effort"] = json!(e),
            }
        }
        body
    }
}

/// Raw entries a server advertises at `/models` (one network call).
pub fn list_models(base_url: &str) -> Result<Vec<Value>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let body: Value = http(Duration::from_secs(10)).get(&url).call()?.into_json()?;
    Ok(body["data"].as_array().cloned().unwrap_or_default())
}

/// LM Studio's load state for `model` ("loaded" / "not-loaded"), from its native
/// `/api/v0/models` endpoint. `None` when the server doesn't offer it (Ollama, vLLM, ...).
pub fn lmstudio_model_state(base_url: &str, model: &str) -> Option<String> {
    let root = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let body: Value = http(Duration::from_secs(3)).get(&format!("{root}/api/v0/models")).call().ok()?.into_json().ok()?;
    body["data"].as_array()?.iter().find(|m| m["id"] == model)?["state"].as_str().map(str::to_owned)
}

/// Ids of the chat (non-embedding) models a server advertises.
pub fn chat_model_ids(base_url: &str) -> Result<Vec<String>> {
    Ok(list_models(base_url)?
        .iter()
        .filter_map(|m| m["id"].as_str())
        .filter(|id| !id.contains("embed"))
        .map(str::to_owned)
        .collect())
}

/// First chat model a server advertises.
pub fn first_chat_model(base_url: &str) -> Result<String> {
    chat_model_ids(base_url)?
        .into_iter()
        .next()
        .ok_or_else(|| format!("no chat models listed at {base_url}/models").into())
}

impl Provider for OpenAiCompatible {
    fn complete(&self, history: &[Message]) -> Result<String> {
        Ok(self.complete_with_reasoning(history)?.text)
    }

    fn complete_with_reasoning(&self, history: &[Message]) -> Result<Reply> {
        let messages: Vec<Value> = history
            .iter()
            .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
            .collect();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut req = http(Duration::from_secs(600)).post(&url);
        if let Some(key) = &self.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }
        let payload = self.request_body(messages);

        // Runs off-thread so Ctrl-C can abandon a request that's taking too long.
        let sent = interrupt::cancellable(move || match req.send_json(payload) {
            Ok(resp) => resp.into_json::<Value>().map_err(Error::from),
            // Surface the server's error body; it usually says what's wrong (bad model, bad key).
            Err(ureq::Error::Status(code, resp)) => {
                Err(Error::Http(format!("HTTP {code}: {}", resp.into_string().unwrap_or_default())))
            }
            Err(e) => Err(Error::from(e)),
        })?;
        let body = sent?;

        let message = &body["choices"][0]["message"];
        let content = message["content"].as_str().ok_or_else(|| format!("unexpected response: {body}"))?;
        // LM Studio / Ollama use `reasoning_content`, OpenRouter uses `reasoning`.
        let field = message["reasoning_content"].as_str().or(message["reasoning"].as_str());
        Ok(split_reasoning(content, field))
    }
}

/// Separates reasoning from the answer. Some servers put it in its own field; others
/// leave it inline as `<think>…</think>` at the start of the content.
fn split_reasoning(content: &str, field: Option<&str>) -> Reply {
    let mut reasoning = field.map(str::trim).filter(|r| !r.is_empty()).map(str::to_owned);
    let mut text = content;
    let trimmed = content.trim_start();
    if let Some(after_open) = trimmed.strip_prefix("<think>") {
        if let Some((inner, rest)) = after_open.split_once("</think>") {
            let inner = inner.trim();
            if reasoning.is_none() && !inner.is_empty() {
                reasoning = Some(inner.to_owned());
            }
            text = rest;
        }
    }
    Reply { text: text.trim_start().to_owned(), reasoning }
}

#[cfg(test)]
mod tests {
    use super::split_reasoning;

    #[test]
    fn reasoning_from_field() {
        let r = split_reasoning("answer", Some("because"));
        assert_eq!((r.text.as_str(), r.reasoning.as_deref()), ("answer", Some("because")));
    }

    #[test]
    fn reasoning_from_think_tags() {
        let r = split_reasoning("<think>\nhmm\n</think>\n\nanswer", None);
        assert_eq!((r.text.as_str(), r.reasoning.as_deref()), ("answer", Some("hmm")));
    }

    #[test]
    fn no_reasoning() {
        let r = split_reasoning("plain", Some("  "));
        assert_eq!((r.text.as_str(), r.reasoning), ("plain", None));
    }
}
