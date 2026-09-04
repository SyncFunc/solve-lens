//! Transport-independent contracts for the LAN controller.
//! The concrete local server owns encrypted sessions; these contracts keep other transports decoupled.

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
