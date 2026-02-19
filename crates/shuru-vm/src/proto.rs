use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize)]
pub struct ExecRequest {
    pub argv: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Deserialize)]
pub struct ExecResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub data: Option<String>,
    pub code: Option<i32>,
}
