use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Session {
    pub model: String,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub text: String,
}

impl Session {
    pub fn open(model: &str) -> Result<Self> {
        Ok(Self { model: model.into(), messages: Vec::new() })
    }

    pub fn push(&mut self, role: &str, text: &str) {
        self.messages.push(Message { role: role.into(), text: text.into() });
    }
}
