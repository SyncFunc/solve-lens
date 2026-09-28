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

/// A completed OpenAI request returns the exact user message that was sent so
/// the desktop application can keep the conversation locally.  Chat
/// Completions has no server-side thread ID, therefore a continuous session
/// must resend these messages on the next request.
pub struct OpenAiSolveResult {
    pub answer: AnswerResult,
    pub user_message: Value,
}

impl OpenAiProvider {
    pub fn solve(
        &self,
        draft: &QuestionDraft,
        preset: &PromptPreset,
        history: &[Value],
    ) -> Result<OpenAiSolveResult> {
        let key = if self.api_key.trim().is_empty() {
            env::var("OPENAI_API_KEY").unwrap_or_default()
        } else {
            self.api_key.clone()
        };
        if key.trim().is_empty() {
            bail!("OpenAI API Key 未配置，请在设置中填写或设置 OPENAI_API_KEY")
        }
        let prompt = prompt_for_turn(
            build_prompt_with_addendum(preset, draft.images.len(), &self.prompt_addendum),
            !history.is_empty(),
        );
        let mut content = vec![json!({"type":"text", "text": prompt})];
        for image in &draft.images {
            let bytes = fs::read(&image.path)
                .with_context(|| format!("read screenshot {}", image.path.display()))?;
            content.push(json!({"type":"image_url", "image_url":{"url":format!("data:image/png;base64,{}", BASE64.encode(bytes))}}));
        }
        let user_message = json!({"role":"user", "content":content});
        let mut messages = history.to_vec();
        messages.push(user_message.clone());
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let response = Client::builder()
            .timeout(Duration::from_secs(self.timeout_seconds.clamp(5, 600)))
            .build()?
            .post(url)
            .bearer_auth(key.trim())
            .json(&json!({"model":self.model,"messages":messages}))
            .send()
            .context("request OpenAI Chat Completions")?;
        let status = response.status();
        let body: Value = response.json().context("decode OpenAI response")?;
        if !status.is_success() {
            bail!(
                "OpenAI API 请求失败（{}）：{}",
                status,
                body.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误")
            );
        }
        let text = body
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .or_else(|| {
                body.pointer("/choices/0/message/content/0/text")
                    .and_then(Value::as_str)
            })
            .context("OpenAI response has no answer text")?;
        Ok(OpenAiSolveResult {
            answer: AnswerResult {
                text: text.to_owned(),
            },
            user_message,
        })
    }
}

pub fn append_history(history: &mut Vec<Value>, user_message: Value, answer: &AnswerResult) {
    history.push(user_message);
    history.push(json!({"role":"assistant", "content":answer.text}));
}

fn prompt_for_turn(base: String, has_history: bool) -> String {
    if !has_history {
        return base;
    }
    format!(
        "{base}\n\n连续对话上下文：本次请求前面的历史消息、题图和回答都属于同一段对话，必须结合它们回答。当前截图可能是对上一题的追问、补充或让你复述先前结论；不要因为它没有重复展示旧题图就忽略历史。安全规则仍然适用于历史与当前所有图片。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_a_complete_user_assistant_turn() {
        let mut history = Vec::new();
        append_history(
            &mut history,
            json!({"role":"user", "content":"question"}),
            &AnswerResult {
                text: "answer".into(),
            },
        );
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["role"], "user");
        assert_eq!(history[1]["content"], "answer");
    }

    #[test]
    fn continuous_turn_prompt_explicitly_preserves_prior_context() {
        let prompt = prompt_for_turn("base rules".into(), true);
        assert!(prompt.contains("必须结合它们回答"));
        assert!(prompt.contains("复述先前结论"));
    }
}
