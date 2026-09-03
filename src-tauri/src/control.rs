//! Transport-independent contracts for a future paired LAN controller.
//! The desktop application deliberately does not bind a network listener yet.

use crate::domain::AnswerResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlCommand {
    Capture { preset_id: String },
    Submit,
    Cancel,
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlEvent {
    DraftChanged {
        preset_id: Option<String>,
        image_count: usize,
    },
    AnswerReady {
        answer: AnswerResult,
    },
    Status {
        status: String,
    },
}

pub trait ControlTransport: Send + Sync {
    fn publish(&self, event: ControlEvent);
}
