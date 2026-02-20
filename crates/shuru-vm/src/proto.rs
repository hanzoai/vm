use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
}

#[derive(Deserialize)]
pub struct ExecResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub data: Option<String>,
    pub code: Option<i32>,
}

/// Host-to-guest control messages sent after the initial ExecRequest (TTY mode only).
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlMessage {
    #[serde(rename = "stdin")]
    Stdin { data: String },
    #[serde(rename = "resize")]
    Resize { rows: u16, cols: u16 },
}
