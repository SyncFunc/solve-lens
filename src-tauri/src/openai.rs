use crate::domain::{AnswerResult, PromptPreset, QuestionDraft};
use crate::presets::build_prompt_with_addendum;
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::{env, fs, time::Duration};

pub struct OpenAiProvider {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
    pub prompt_addendum: String,
}

impl OpenAiProvider {
    pub fn solve(&self, draft: &QuestionDraft, preset: &PromptPreset) -> Result<AnswerResult> {
        let key = if self.api_key.trim().is_empty() { env::var("OPENAI_API_KEY").unwrap_or_default() } else { self.api_key.clone() };
        if key.trim().is_empty() { bail!("OpenAI API Key 未配置，请在设置中填写或设置 OPENAI_API_KEY") }
        let mut content = vec![json!({"type":"text", "text": build_prompt_with_addendum(preset, draft.images.len(), &self.prompt_addendum)})];
        for image in &draft.images {
            let bytes = fs::read(&image.path).with_context(|| format!("read screenshot {}", image.path.display()))?;
            content.push(json!({"type":"image_url", "image_url":{"url":format!("data:image/png;base64,{}", BASE64.encode(bytes))}}));
        }
        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));
        let response = Client::builder().timeout(Duration::from_secs(self.timeout_seconds.clamp(5, 600))).build()?.post(url)
            .bearer_auth(key.trim()).json(&json!({"model":self.model,"messages":[{"role":"user","content":content}]})).send()
            .context("request OpenAI Chat Completions")?;
        let status = response.status(); let body: Value = response.json().context("decode OpenAI response")?;
        if !status.is_success() { bail!("OpenAI API 请求失败（{}）：{}", status, body.pointer("/error/message").and_then(Value::as_str).unwrap_or("未知错误")); }
        let text = body.pointer("/choices/0/message/content").and_then(Value::as_str)
            .or_else(|| body.pointer("/choices/0/message/content/0/text").and_then(Value::as_str))
            .context("OpenAI response has no answer text")?;
        Ok(AnswerResult { text: text.to_owned() })
    }
}
