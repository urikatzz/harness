use crate::provider::{Message, Provider, Result};
use serde_json::{json, Value};

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
    let body: Value = ureq::get(&url).call()?.into_json()?;
    Ok(body["data"].as_array().cloned().unwrap_or_default())
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
        let messages: Vec<Value> = history
            .iter()
            .map(|m| json!({ "role": m.role.as_str(), "content": m.content }))
            .collect();
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

        let mut req = ureq::post(&url);
        if let Some(key) = &self.api_key {
            req = req.set("Authorization", &format!("Bearer {key}"));
        }

        let body: Value = match req.send_json(self.request_body(messages)) {
            Ok(resp) => resp.into_json()?,
            // Surface the server's error body; it usually says what's wrong (bad model, bad key).
            Err(ureq::Error::Status(code, resp)) => {
                return Err(format!("HTTP {code}: {}", resp.into_string().unwrap_or_default()).into())
            }
            Err(e) => return Err(e.into()),
        };

        body["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("unexpected response: {body}").into())
    }
}
