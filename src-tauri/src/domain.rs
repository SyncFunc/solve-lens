use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub const MAX_IMAGES_PER_DRAFT: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptPreset {
    pub id: String,
    pub name: String,
    pub built_in: bool,
    pub hotkey: String,
    pub task_template: String,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedImage {
    #[serde(skip_serializing)]
    pub path: PathBuf,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionDraft {
    pub id: Uuid,
    pub preset_id: String,
    pub images: Vec<CapturedImage>,
    pub created_at: DateTime<Utc>,
}

impl QuestionDraft {
    pub fn new(preset_id: String, first: CapturedImage) -> Self {
        Self {
            id: Uuid::new_v4(),
            preset_id,
            images: vec![first],
            created_at: Utc::now(),
        }
    }

    pub fn try_push(&mut self, image: CapturedImage) -> Result<(), DomainError> {
        if self.images.len() >= MAX_IMAGES_PER_DRAFT {
            return Err(DomainError::ImageLimit);
        }
        self.images.push(image);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnswerResult {
    /// Provider 原样返回的可读文本，不在应用层强制套结构化 schema。
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftView {
    pub id: Uuid,
    pub preset_id: String,
    pub image_count: usize,
    pub thumbnails: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl From<&QuestionDraft> for DraftView {
    fn from(draft: &QuestionDraft) -> Self {
        Self {
            id: draft.id,
            preset_id: draft.preset_id.clone(),
            image_count: draft.images.len(),
            thumbnails: Vec::new(),
            created_at: draft.created_at,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("a draft can contain at most {MAX_IMAGES_PER_DRAFT} screenshots")]
    ImageLimit,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(name: &str) -> CapturedImage {
        CapturedImage {
            path: PathBuf::from(name),
            captured_at: Utc::now(),
        }
    }

    #[test]
    fn keeps_capture_order_and_rejects_the_seventh_image() {
        let mut draft = QuestionDraft::new("math".into(), image("1.png"));
        for index in 2..=6 {
            draft.try_push(image(&format!("{index}.png"))).unwrap();
        }
        assert_eq!(draft.images[0].path, PathBuf::from("1.png"));
        assert!(matches!(
            draft.try_push(image("7.png")),
            Err(DomainError::ImageLimit)
        ));
    }
}
