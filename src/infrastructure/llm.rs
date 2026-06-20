use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::domain::{Config, LlmProviderKind};
use crate::error::{AppError, Result};

use super::OllamaClient;

struct ProviderConfig<'a> {
    provider: LlmProviderKind,
    base_url: String,
    model: Option<&'a str>,
    api_key: Option<&'a str>,
}

#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    ollama: OllamaClient,
}

impl LlmClient {
    pub fn new(client: Client) -> Self {
        Self {
            ollama: OllamaClient::new(client.clone()),
            client,
        }
    }

    pub async fn models(&self, config: &Config, api_key: Option<&str>) -> Result<Vec<String>> {
        match config.provider {
            LlmProviderKind::Unset => Err(AppError::ProviderNotSelected),
            LlmProviderKind::Ollama => self.ollama.models().await,
            LlmProviderKind::OpenAi => self.openai_models(config, api_key).await,
            LlmProviderKind::Gemini => self.gemini_models(config, api_key).await,
            LlmProviderKind::Anthropic => self.anthropic_models(config, api_key).await,
        }
    }

    async fn openai_models(&self, config: &Config, api_key: Option<&str>) -> Result<Vec<String>> {
        let provider = self.ensure_connection(config, api_key)?;
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let payload = self
            .client
            .get(url)
            .bearer_auth(provider.api_key.unwrap_or_default())
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;

        Ok(payload
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect())
    }

    async fn gemini_models(&self, config: &Config, api_key: Option<&str>) -> Result<Vec<String>> {
        let provider = self.ensure_connection(config, api_key)?;
        let url = format!(
            "{}/models?key={}",
            provider.base_url.trim_end_matches('/'),
            provider.api_key.unwrap_or_default()
        );
        let payload = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;

        Ok(payload
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("name").and_then(Value::as_str))
            .map(|name| name.trim_start_matches("models/").to_string())
            .collect())
    }

    async fn anthropic_models(
        &self,
        config: &Config,
        api_key: Option<&str>,
    ) -> Result<Vec<String>> {
        let provider = self.ensure_connection(config, api_key)?;
        let url = format!("{}/models", provider.base_url.trim_end_matches('/'));
        let payload = self
            .client
            .get(url)
            .header("x-api-key", provider.api_key.unwrap_or_default())
            .header("anthropic-version", "2023-06-01")
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;

        Ok(payload
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect())
    }

    pub async fn generate(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        num_ctx: Option<usize>,
    ) -> Result<String> {
        match config.provider {
            LlmProviderKind::Unset => Err(AppError::ProviderNotSelected),
            LlmProviderKind::Ollama => self.ollama.generate(model, prompt, num_ctx).await,
            LlmProviderKind::OpenAi => {
                self.openai_generate(config, api_key, model, prompt, false)
                    .await
            }
            LlmProviderKind::Gemini => {
                self.gemini_generate(config, api_key, model, prompt, false)
                    .await
            }
            LlmProviderKind::Anthropic => {
                self.anthropic_generate(config, api_key, model, prompt, false)
                    .await
            }
        }
    }

    pub async fn generate_json(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        num_ctx: Option<usize>,
    ) -> Result<String> {
        match config.provider {
            LlmProviderKind::Unset => Err(AppError::ProviderNotSelected),
            LlmProviderKind::Ollama => self.ollama.generate_json(model, prompt, num_ctx).await,
            LlmProviderKind::OpenAi => {
                self.openai_generate(config, api_key, model, prompt, true)
                    .await
            }
            LlmProviderKind::Gemini => {
                self.gemini_generate(config, api_key, model, prompt, true)
                    .await
            }
            LlmProviderKind::Anthropic => {
                self.anthropic_generate(config, api_key, model, prompt, true)
                    .await
            }
        }
    }

    pub async fn generate_stream(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        num_ctx: Option<usize>,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        match config.provider {
            LlmProviderKind::Unset => Err(AppError::ProviderNotSelected),
            LlmProviderKind::Ollama => {
                self.ollama
                    .generate_stream(model, prompt, num_ctx, tx)
                    .await
            }
            LlmProviderKind::OpenAi => {
                self.openai_generate_stream(config, api_key, model, prompt, tx)
                    .await
            }
            LlmProviderKind::Gemini => {
                self.gemini_generate_stream(config, api_key, model, prompt, tx)
                    .await
            }
            LlmProviderKind::Anthropic => {
                self.anthropic_generate_stream(config, api_key, model, prompt, tx)
                    .await
            }
        }
    }

    pub async fn health(&self, config: &Config, api_key: Option<&str>) -> Result<String> {
        let provider = self.ensure_ready(config, api_key)?;
        match provider.provider {
            LlmProviderKind::Unset => Err(AppError::ProviderNotSelected),
            LlmProviderKind::Ollama => self.ollama.version().await,
            LlmProviderKind::OpenAi => {
                let response = self
                    .client
                    .get(format!("{}/models", provider.base_url))
                    .bearer_auth(provider.api_key.unwrap_or_default())
                    .send()
                    .await
                    .map_err(|source| AppError::ProviderUnavailable {
                        provider: provider.provider.label().to_string(),
                        source,
                    })?;
                if !response.status().is_success() {
                    return Err(AppError::ProviderHttp {
                        provider: provider.provider.label().to_string(),
                        status: response.status().as_u16(),
                    });
                }
                Ok(provider.model.unwrap().to_string())
            }
            LlmProviderKind::Gemini => {
                let response = self
                    .client
                    .get(format!(
                        "{}/models/{}?key={}",
                        provider.base_url,
                        provider.model.unwrap(),
                        provider.api_key.unwrap_or_default()
                    ))
                    .send()
                    .await
                    .map_err(|source| AppError::ProviderUnavailable {
                        provider: provider.provider.label().to_string(),
                        source,
                    })?;
                if !response.status().is_success() {
                    return Err(AppError::ProviderHttp {
                        provider: provider.provider.label().to_string(),
                        status: response.status().as_u16(),
                    });
                }
                Ok(provider.model.unwrap().to_string())
            }
            LlmProviderKind::Anthropic => {
                let response = self
                    .client
                    .get(format!("{}/models", provider.base_url))
                    .header("x-api-key", provider.api_key.unwrap_or_default())
                    .header("anthropic-version", "2023-06-01")
                    .send()
                    .await
                    .map_err(|source| AppError::ProviderUnavailable {
                        provider: provider.provider.label().to_string(),
                        source,
                    })?;
                if !response.status().is_success() {
                    return Err(AppError::ProviderHttp {
                        provider: provider.provider.label().to_string(),
                        status: response.status().as_u16(),
                    });
                }
                Ok(provider.model.unwrap().to_string())
            }
        }
    }

    fn ensure_connection<'a>(
        &self,
        config: &'a Config,
        api_key: Option<&'a str>,
    ) -> Result<ProviderConfig<'a>> {
        let provider = config.provider;
        if !provider.is_selected() {
            return Err(AppError::ProviderNotSelected);
        }
        let base_url = provider_base_url(config)?;
        let api_key = api_key.map(str::trim).filter(|key| !key.is_empty());
        if provider.uses_api_key() && api_key.is_none() {
            return Err(AppError::MissingApiKey {
                provider: provider.label().to_string(),
            });
        }
        Ok(ProviderConfig {
            provider,
            base_url,
            model: config.model.as_deref(),
            api_key,
        })
    }

    fn ensure_ready<'a>(
        &self,
        config: &'a Config,
        api_key: Option<&'a str>,
    ) -> Result<ProviderConfig<'a>> {
        let mut pc = self.ensure_connection(config, api_key)?;
        pc.model = Some(pc.model.ok_or_else(|| AppError::MissingModel {
            provider: pc.provider.label().to_string(),
        })?);
        Ok(pc)
    }

    async fn openai_generate(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        json_mode: bool,
    ) -> Result<String> {
        let provider = self.ensure_ready(config, api_key)?;
        let url = format!("{}/chat/completions", provider.base_url);
        let mut body = json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": false
        });
        if json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }
        let response = self
            .client
            .post(url)
            .bearer_auth(provider.api_key.unwrap_or_default())
            .json(&body)
            .send()
            .await
            .map_err(|source| AppError::ProviderUnavailable {
                provider: provider.provider.label().to_string(),
                source,
            })?;
        if !response.status().is_success() {
            return Err(AppError::ProviderHttp {
                provider: provider.provider.label().to_string(),
                status: response.status().as_u16(),
            });
        }
        let payload =
            response
                .json::<Value>()
                .await
                .map_err(|source| AppError::ProviderDecode {
                    provider: provider.provider.label().to_string(),
                    source,
                })?;
        extract_openai_text(&payload).ok_or_else(|| {
            AppError::InvalidLlmResponse(format!(
                "{} returned no text content",
                provider.provider.label()
            ))
        })
    }

    async fn openai_generate_stream(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let provider = self.ensure_ready(config, api_key)?;
        let url = format!("{}/chat/completions", provider.base_url);
        let body = json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": true
        });
        let response = self
            .client
            .post(url)
            .bearer_auth(provider.api_key.unwrap_or_default())
            .json(&body)
            .send()
            .await
            .map_err(|source| AppError::ProviderUnavailable {
                provider: provider.provider.label().to_string(),
                source,
            })?;
        parse_sse_stream(response, provider.provider.label(), tx, |payload| {
            if payload == "[DONE]" {
                return Some(StreamEvent::Done);
            }
            let value = serde_json::from_str::<Value>(payload).ok()?;
            let text = extract_openai_delta_text(&value)?;
            Some(StreamEvent::Text(text))
        })
        .await
    }

    async fn gemini_generate(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        json_mode: bool,
    ) -> Result<String> {
        let provider = self.ensure_ready(config, api_key)?;
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            provider.base_url,
            model,
            provider.api_key.unwrap_or_default()
        );
        let mut body = json!({
            "contents": [{"role": "user", "parts": [{"text": prompt}]}]
        });
        if json_mode {
            body["generationConfig"] = json!({ "responseMimeType": "application/json" });
        }
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|source| AppError::ProviderUnavailable {
                provider: provider.provider.label().to_string(),
                source,
            })?;
        if !response.status().is_success() {
            return Err(AppError::ProviderHttp {
                provider: provider.provider.label().to_string(),
                status: response.status().as_u16(),
            });
        }
        let payload =
            response
                .json::<Value>()
                .await
                .map_err(|source| AppError::ProviderDecode {
                    provider: provider.provider.label().to_string(),
                    source,
                })?;
        extract_gemini_text(&payload).ok_or_else(|| {
            AppError::InvalidLlmResponse("Gemini returned no text content".to_string())
        })
    }

    async fn gemini_generate_stream(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let provider = self.ensure_ready(config, api_key)?;
        let url = format!(
            "{}/models/{}:streamGenerateContent?alt=sse&key={}",
            provider.base_url,
            model,
            provider.api_key.unwrap_or_default()
        );
        let body = json!({
            "contents": [{"role": "user", "parts": [{"text": prompt}]}]
        });
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|source| AppError::ProviderUnavailable {
                provider: provider.provider.label().to_string(),
                source,
            })?;
        parse_sse_stream(response, provider.provider.label(), tx, |payload| {
            let value = serde_json::from_str::<Value>(payload).ok()?;
            let text = extract_gemini_text(&value)?;
            Some(StreamEvent::Text(text))
        })
        .await
    }

    async fn anthropic_generate(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        _json_mode: bool,
    ) -> Result<String> {
        let provider = self.ensure_ready(config, api_key)?;
        let url = format!("{}/messages", provider.base_url);
        let body = json!({
            "model": model,
            "max_tokens": 2048,
            "messages": [{"role": "user", "content": prompt}]
        });
        let response = self
            .client
            .post(url)
            .header("x-api-key", provider.api_key.unwrap_or_default())
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|source| AppError::ProviderUnavailable {
                provider: provider.provider.label().to_string(),
                source,
            })?;
        if !response.status().is_success() {
            return Err(AppError::ProviderHttp {
                provider: provider.provider.label().to_string(),
                status: response.status().as_u16(),
            });
        }
        let payload =
            response
                .json::<Value>()
                .await
                .map_err(|source| AppError::ProviderDecode {
                    provider: provider.provider.label().to_string(),
                    source,
                })?;
        extract_anthropic_text(&payload).ok_or_else(|| {
            AppError::InvalidLlmResponse("Anthropic returned no text content".to_string())
        })
    }

    async fn anthropic_generate_stream(
        &self,
        config: &Config,
        api_key: Option<&str>,
        model: &str,
        prompt: &str,
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<String> {
        let provider = self.ensure_ready(config, api_key)?;
        let url = format!("{}/messages", provider.base_url);
        let body = json!({
            "model": model,
            "max_tokens": 2048,
            "messages": [{"role": "user", "content": prompt}],
            "stream": true
        });
        let response = self
            .client
            .post(url)
            .header("x-api-key", provider.api_key.unwrap_or_default())
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|source| AppError::ProviderUnavailable {
                provider: provider.provider.label().to_string(),
                source,
            })?;
        parse_sse_stream(response, provider.provider.label(), tx, |payload| {
            let value = serde_json::from_str::<Value>(payload).ok()?;
            match value.get("type").and_then(Value::as_str) {
                Some("content_block_delta") => value
                    .get("delta")
                    .and_then(|delta| delta.get("text"))
                    .and_then(Value::as_str)
                    .map(|text| StreamEvent::Text(text.to_string())),
                Some("message_stop") => Some(StreamEvent::Done),
                _ => None,
            }
        })
        .await
    }
}

enum StreamEvent {
    Text(String),
    Done,
}

async fn parse_sse_stream<F>(
    mut response: reqwest::Response,
    provider: &str,
    tx: mpsc::UnboundedSender<String>,
    mut parse_payload: F,
) -> Result<String>
where
    F: FnMut(&str) -> Option<StreamEvent>,
{
    if !response.status().is_success() {
        return Err(AppError::ProviderHttp {
            provider: provider.to_string(),
            status: response.status().as_u16(),
        });
    }

    let mut full_text = String::new();
    let mut buffer = String::new();
    let mut done = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| AppError::ProviderDecode {
            provider: provider.to_string(),
            source,
        })?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(newline) = buffer.find('\n') {
            let line = buffer[..newline].trim().to_string();
            buffer = buffer[newline + 1..].to_string();
            if !line.starts_with("data:") {
                continue;
            }
            let payload = line.trim_start_matches("data:").trim();
            let Some(event) = parse_payload(payload) else {
                continue;
            };
            match event {
                StreamEvent::Text(text) => {
                    full_text.push_str(&text);
                    let _ = tx.send(text);
                }
                StreamEvent::Done => done = true,
            }
        }
    }
    if done || !full_text.is_empty() {
        Ok(full_text)
    } else {
        Err(AppError::Custom(format!(
            "{} stream ended without content",
            provider
        )))
    }
}

fn provider_base_url(config: &Config) -> Result<String> {
    let provider = config.provider;
    if !provider.is_selected() {
        return Err(AppError::ProviderNotSelected);
    }
    if provider == LlmProviderKind::Ollama
        && let Some(base_url) = config
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        return Ok(base_url.trim_end_matches('/').to_string());
    }
    provider
        .default_base_url()
        .map(|value| value.to_string())
        .ok_or_else(|| AppError::MissingBaseUrl {
            provider: provider.label().to_string(),
        })
}

fn extract_openai_text(payload: &Value) -> Option<String> {
    payload
        .get("choices")?
        .as_array()?
        .first()?
        .get("message")?
        .get("content")
        .and_then(Value::as_str)
        .map(|text| text.to_string())
}

fn extract_openai_delta_text(payload: &Value) -> Option<String> {
    payload
        .get("choices")?
        .as_array()?
        .first()?
        .get("delta")?
        .get("content")
        .and_then(Value::as_str)
        .map(|text| text.to_string())
}

fn extract_gemini_text(payload: &Value) -> Option<String> {
    let parts = payload
        .get("candidates")?
        .as_array()?
        .first()?
        .get("content")?
        .get("parts")?
        .as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

fn extract_anthropic_text(payload: &Value) -> Option<String> {
    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}
