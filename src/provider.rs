use std::{collections::BTreeMap, env};

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc::Sender;

use crate::{
    config::{ModelConfig, Provider, WireApi},
    error::AppError,
    model::{Manifest, Message, ToolCall},
    prompt_policy,
};

/// Bounded HTTP provider adapters. Application tools never execute here.
pub struct ProviderClient {
    client: reqwest::Client,
}

impl ProviderClient {
    pub fn new() -> Result<Self, AppError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .user_agent("asystant-ai/0.1")
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| AppError::Internal)?,
        })
    }

    async fn read_bounded(response: reqwest::Response) -> Result<Vec<u8>, AppError> {
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| AppError::Provider)?;
            if bytes.len() + chunk.len() > 2_000_000 {
                return Err(AppError::Provider);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub fn openai_messages(manifest: &Manifest, messages: &[Message]) -> Vec<Value> {
        let mut output = vec![json!({
            "role": "system",
            "content": prompt_policy::instructions(&manifest.prompts),
        })];
        for m in messages {
            let mut value = json!({"role":m.role,"content":m.content});
            if !m.calls.is_empty() {
                value["tool_calls"] = json!(
                    m.calls
                        .iter()
                        .map(|call| json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": call.arguments.to_string(),
                            },
                        }))
                        .collect::<Vec<_>>()
                );
            }
            if m.role == "tool" {
                value["tool_call_id"] = json!(m.call_id)
            }
            output.push(value)
        }
        output
    }

    pub async fn infer(
        &self,
        model: &ModelConfig,
        manifest: &Manifest,
        messages: &[Message],
        session: &str,
        events: &Sender<Result<actix_web::web::Bytes, actix_web::Error>>,
    ) -> Result<(Message, i64), AppError> {
        let key = env::var(&model.key_env).map_err(|_| AppError::Internal)?;
        if model.provider == Provider::Anthropic || model.wire_api == WireApi::Messages {
            return self
                .anthropic(model, manifest, messages, &key, session)
                .await;
        }
        if model.wire_api == WireApi::Responses {
            return self
                .responses(model, manifest, messages, &key, session)
                .await;
        }
        let url = match model.provider {
            Provider::Openrouter => "https://openrouter.ai/api/v1/chat/completions",
            Provider::Openai => "https://api.openai.com/v1/chat/completions",
            Provider::Gemini => {
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
            }
            Provider::OpencodeZen => "https://opencode.ai/zen/v1/chat/completions",
            Provider::OpencodeGo => "https://opencode.ai/zen/go/v1/chat/completions",
            Provider::Anthropic => return Err(AppError::Invalid),
        };
        let mut body = json!({
            "model": model.model,
            "messages": Self::openai_messages(manifest, messages),
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        body[if model.provider == Provider::Openai {
            "max_completion_tokens"
        } else {
            "max_tokens"
        }] = json!(model.max_output_tokens);
        if !manifest.tools.is_empty() {
            body["tools"] = json!(
                manifest
                    .tools
                    .iter()
                    .map(|tool| json!({"type":"function","function":tool}))
                    .collect::<Vec<_>>()
            );
        }
        if model.provider == Provider::Openrouter {
            body["provider"] = json!({
                "require_parameters": true,
                "max_price": {
                    "prompt": model.input_micros_per_million as f64 / 1_000_000.0,
                    "completion": model.output_micros_per_million as f64 / 1_000_000.0,
                },
            });
        }
        let response = self
            .client
            .post(url)
            .bearer_auth(key)
            .header("x-opencode-session", session)
            .json(&body)
            .send()
            .await
            .map_err(|_| AppError::Provider)?;
        if !response.status().is_success() {
            return Err(AppError::Provider);
        }
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut text = String::new();
        let mut calls = BTreeMap::<usize, (String, String, String)>::new();
        let mut charge = None;
        let mut finished = false;
        let mut done = false;
        let mut received = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| AppError::Provider)?;
            received += chunk.len();
            if received > 2_000_000 {
                return Err(AppError::Provider);
            }
            buffer.extend_from_slice(&chunk);
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line = buffer.drain(..=end).collect::<Vec<_>>();
                let line = std::str::from_utf8(&line)
                    .map_err(|_| AppError::Provider)?
                    .trim();
                let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                    continue;
                };
                if data == "[DONE]" {
                    done = true;
                    continue;
                }
                if data.is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(data).map_err(|_| AppError::Provider)?;
                if value.get("error").is_some() {
                    return Err(AppError::Provider);
                }
                if let Some(usage) = value.get("usage") {
                    charge = Self::usage_charge(model, usage);
                }
                if let Some(choice) = value
                    .get("choices")
                    .and_then(Value::as_array)
                    .and_then(|a| a.first())
                {
                    if let Some(reason) = choice["finish_reason"].as_str() {
                        if reason != "stop" && reason != "tool_calls" {
                            return Err(AppError::Provider);
                        }
                        finished = true;
                    }
                    let delta = &choice["delta"];
                    if let Some(part) = delta["content"].as_str() {
                        text.push_str(part);
                        let _ = events.try_send(Ok(actix_web::web::Bytes::from(format!(
                            "data: {}\n\n",
                            json!({"type":"text_delta","text":part})
                        ))));
                    }
                    if let Some(deltas) = delta["tool_calls"].as_array() {
                        for c in deltas {
                            let index = c["index"].as_u64().ok_or(AppError::Provider)? as usize;
                            if index >= 16 {
                                return Err(AppError::Provider);
                            }
                            let entry = calls.entry(index).or_default();
                            if let Some(id) = c["id"].as_str() {
                                entry.0.push_str(id)
                            }
                            if let Some(name) = c["function"]["name"].as_str() {
                                entry.1.push_str(name)
                            }
                            if let Some(arguments) = c["function"]["arguments"].as_str() {
                                entry.2.push_str(arguments)
                            }
                        }
                    }
                }
            }
        }
        if !finished || !done {
            return Err(AppError::Provider);
        }
        let calls = calls
            .into_values()
            .map(|(id, name, args)| {
                Ok(ToolCall {
                    id,
                    name,
                    arguments: serde_json::from_str(&args).map_err(|_| AppError::Provider)?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        Ok((
            Message {
                role: "assistant".into(),
                content: text,
                calls,
                call_id: String::new(),
            },
            charge.ok_or(AppError::Provider)?,
        ))
    }

    pub fn usage_charge(model: &ModelConfig, usage: &Value) -> Option<i64> {
        if model.provider == Provider::Openrouter
            && let Some(cost) = usage["cost"].as_f64()
            && cost.is_finite()
            && (0.0..1_000_000.0).contains(&cost)
        {
            return Some((cost * 1_000_000.0).ceil() as i64);
        }
        // Conservative: charge all input tokens at uncached rate, including cached input.
        let input = usage["prompt_tokens"]
            .as_i64()
            .or_else(|| usage["input_tokens"].as_i64())?;
        let output = usage["completion_tokens"]
            .as_i64()
            .or_else(|| usage["output_tokens"].as_i64())?;
        if input < 0 || output < 0 {
            return None;
        }
        let cached_write = usage["cache_creation_input_tokens"].as_i64().unwrap_or(0);
        let cached_read = usage["cache_read_input_tokens"].as_i64().unwrap_or(0);
        let input = input.checked_add(cached_write)?.checked_add(cached_read)?;
        input
            .checked_mul(model.input_micros_per_million)?
            .checked_add(output.checked_mul(model.output_micros_per_million)?)?
            .checked_add(999_999)
            .map(|n| n / 1_000_000)
    }

    fn anthropic_message(message: &Message) -> Value {
        if message.role == "tool" {
            return json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": message.call_id,
                    "content": message.content,
                }],
            });
        }
        let mut content = Vec::new();
        if !message.content.is_empty() {
            content.push(json!({"type": "text", "text": message.content}));
        }
        for call in &message.calls {
            content.push(json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.name,
                "input": call.arguments,
            }));
        }
        json!({"role": message.role, "content": content})
    }

    async fn anthropic(
        &self,
        model: &ModelConfig,
        manifest: &Manifest,
        messages: &[Message],
        key: &str,
        session: &str,
    ) -> Result<(Message, i64), AppError> {
        let converted = messages
            .iter()
            .map(Self::anthropic_message)
            .collect::<Vec<_>>();
        let tools = manifest
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool["name"],
                    "description": tool["description"],
                    "input_schema": tool["parameters"],
                })
            })
            .collect::<Vec<_>>();
        let url = match model.provider {
            Provider::Anthropic => "https://api.anthropic.com/v1/messages",
            Provider::OpencodeZen => "https://opencode.ai/zen/v1/messages",
            Provider::OpencodeGo => "https://opencode.ai/zen/go/v1/messages",
            _ => return Err(AppError::Invalid),
        };
        let body = json!({
            "model": model.model,
            "system": prompt_policy::instructions(&manifest.prompts),
            "messages": converted,
            "tools": tools,
            "max_tokens": model.max_output_tokens,
        });
        let response = self
            .client
            .post(url)
            .header("x-opencode-session", session)
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|_| AppError::Provider)?;
        if !response.status().is_success() {
            return Err(AppError::Provider);
        }
        let bytes = Self::read_bounded(response).await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| AppError::Provider)?;
        if !matches!(value["stop_reason"].as_str(), Some("end_turn" | "tool_use")) {
            return Err(AppError::Provider);
        }
        let mut text = String::new();
        let mut calls = vec![];
        for part in value["content"].as_array().ok_or(AppError::Provider)? {
            match part["type"].as_str() {
                Some("text") => text.push_str(part["text"].as_str().ok_or(AppError::Provider)?),
                Some("tool_use") => calls.push(ToolCall {
                    id: part["id"].as_str().ok_or(AppError::Provider)?.into(),
                    name: part["name"].as_str().ok_or(AppError::Provider)?.into(),
                    arguments: part["input"].clone(),
                }),
                _ => {}
            }
        }
        Ok((
            Message {
                role: "assistant".into(),
                content: text,
                calls,
                call_id: String::new(),
            },
            Self::usage_charge(model, &value["usage"]).ok_or(AppError::Provider)?,
        ))
    }

    async fn responses(
        &self,
        model: &ModelConfig,
        manifest: &Manifest,
        messages: &[Message],
        key: &str,
        session: &str,
    ) -> Result<(Message, i64), AppError> {
        let mut input = Vec::new();
        for m in messages {
            if m.role == "tool" {
                input.push(
                    json!({"type":"function_call_output","call_id":m.call_id,"output":m.content}),
                );
                continue;
            }
            if !m.content.is_empty() {
                input.push(json!({"role":m.role,"content":m.content}));
            }
            for c in &m.calls {
                input.push(json!({"type":"function_call","call_id":c.id,"name":c.name,"arguments":c.arguments.to_string()}));
            }
        }
        let tools = manifest
            .tools
            .iter()
            .map(|tool| {
                let mut value = tool.clone();
                value["type"] = json!("function");
                value
            })
            .collect::<Vec<_>>();
        let url = match model.provider {
            Provider::Openai => "https://api.openai.com/v1/responses",
            Provider::OpencodeZen => "https://opencode.ai/zen/v1/responses",
            Provider::OpencodeGo => "https://opencode.ai/zen/go/v1/responses",
            _ => return Err(AppError::Invalid),
        };
        let body = json!({
            "model": model.model,
            "instructions": prompt_policy::instructions(&manifest.prompts),
            "input": input,
            "tools": tools,
            "max_output_tokens": model.max_output_tokens,
            "store": false,
        });
        let response = self
            .client
            .post(url)
            .bearer_auth(key)
            .header("x-opencode-session", session)
            .json(&body)
            .send()
            .await
            .map_err(|_| AppError::Provider)?;
        if !response.status().is_success() {
            return Err(AppError::Provider);
        }
        let bytes = Self::read_bounded(response).await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| AppError::Provider)?;
        if value["status"] != "completed" {
            return Err(AppError::Provider);
        }
        let mut text = String::new();
        let mut calls = Vec::new();
        for item in value["output"].as_array().ok_or(AppError::Provider)? {
            match item["type"].as_str() {
                Some("message") => {
                    for part in item["content"].as_array().ok_or(AppError::Provider)? {
                        if let Some(part) = part["text"].as_str() {
                            text.push_str(part)
                        }
                    }
                }
                Some("function_call") => calls.push(ToolCall {
                    id: item["call_id"].as_str().ok_or(AppError::Provider)?.into(),
                    name: item["name"].as_str().ok_or(AppError::Provider)?.into(),
                    arguments: serde_json::from_str(
                        item["arguments"].as_str().ok_or(AppError::Provider)?,
                    )
                    .map_err(|_| AppError::Provider)?,
                }),
                _ => {}
            }
        }
        Ok((
            Message {
                role: "assistant".into(),
                content: text,
                calls,
                call_id: String::new(),
            },
            Self::usage_charge(model, &value["usage"]).ok_or(AppError::Provider)?,
        ))
    }
}

#[async_trait::async_trait(?Send)]
pub trait InferenceProvider: Send + Sync {
    async fn complete(
        &self,
        model: &ModelConfig,
        manifest: &Manifest,
        messages: &[Message],
        session: &str,
        events: &Sender<Result<actix_web::web::Bytes, actix_web::Error>>,
    ) -> Result<(Message, i64), AppError>;
}
#[async_trait::async_trait(?Send)]
impl InferenceProvider for ProviderClient {
    async fn complete(
        &self,
        model: &ModelConfig,
        manifest: &Manifest,
        messages: &[Message],
        session: &str,
        events: &Sender<Result<actix_web::web::Bytes, actix_web::Error>>,
    ) -> Result<(Message, i64), AppError> {
        self.infer(model, manifest, messages, session, events).await
    }
}
