use std::io::{self, BufRead, Write};

use anyhow::Result;
use base64::Engine;
use serde::{Deserialize, Serialize};

use shuru_vm::Sandbox;

use crate::vm::{self, PreparedVm};

const BASE64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

mod method {
    pub const EXEC: &str = "exec";
    pub const READ_FILE: &str = "read_file";
    pub const WRITE_FILE: &str = "write_file";
    pub const CHECKPOINT: &str = "checkpoint";
}

// JSON-RPC 2.0 error codes
const PARSE_ERROR: i32 = -32700;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const SERVER_ERROR: i32 = -32000;

// --- JSON-RPC 2.0 types ---

#[derive(Deserialize)]
struct JsonRpcRequest {
    id: serde_json::Value,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize)]
struct JsonRpcNotification {
    jsonrpc: &'static str,
    method: &'static str,
}

#[derive(Serialize)]
struct JsonRpcResult<T: Serialize> {
    jsonrpc: &'static str,
    id: serde_json::Value,
    result: T,
}

#[derive(Serialize)]
struct JsonRpcErrorResp {
    jsonrpc: &'static str,
    id: serde_json::Value,
    error: RpcError,
}

#[derive(Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

// --- Result payloads ---

#[derive(Serialize)]
struct ExecResultPayload {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[derive(Serialize)]
struct ReadFileResultPayload {
    content: String,
}

#[derive(Serialize)]
struct EmptyResult {}

// --- Param types ---

#[derive(Deserialize)]
struct ExecParams {
    argv: Vec<String>,
}

#[derive(Deserialize)]
struct ReadFileParams {
    path: String,
}

#[derive(Deserialize)]
struct WriteFileParams {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct CheckpointParams {
    name: String,
}

fn send_json(w: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let line = serde_json::to_string(value)?;
    writeln!(w, "{}", line)?;
    w.flush()?;
    Ok(())
}

fn send_result<T: Serialize>(
    w: &mut impl Write,
    id: serde_json::Value,
    result: T,
) -> Result<()> {
    send_json(
        w,
        &JsonRpcResult {
            jsonrpc: "2.0",
            id,
            result,
        },
    )
}

fn send_error(
    w: &mut impl Write,
    id: serde_json::Value,
    code: i32,
    message: String,
) -> Result<()> {
    send_json(
        w,
        &JsonRpcErrorResp {
            jsonrpc: "2.0",
            id,
            error: RpcError { code, message },
        },
    )
}

macro_rules! parse_params {
    ($params:expr, $id:expr, $out:expr) => {
        match serde_json::from_value($params) {
            Ok(p) => p,
            Err(e) => {
                send_error($out, $id, INVALID_PARAMS, format!("invalid params: {}", e))?;
                continue;
            }
        }
    };
}

pub(crate) fn run_stdio(prepared: &PreparedVm) -> Result<i32> {
    let mut out = io::stdout().lock();

    let sandbox = vm::build_sandbox(prepared, false, None)?;
    sandbox.start()?;

    let _fwd = if !prepared.forwards.is_empty() {
        Some(sandbox.start_port_forwarding(&prepared.forwards)?)
    } else {
        None
    };

    send_json(
        &mut out,
        &JsonRpcNotification {
            jsonrpc: "2.0",
            method: "ready",
        },
    )?;

    let stdin = io::stdin().lock();
    for line in stdin.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                send_error(
                    &mut out,
                    serde_json::Value::Null,
                    PARSE_ERROR,
                    format!("parse error: {}", e),
                )?;
                continue;
            }
        };

        match req.method.as_str() {
            method::EXEC => {
                let params: ExecParams = parse_params!(req.params, req.id, &mut out);
                handle_exec(&sandbox, req.id, &params.argv, &mut out)?;
            }
            method::READ_FILE => {
                let params: ReadFileParams = parse_params!(req.params, req.id, &mut out);
                handle_read_file(&sandbox, req.id, &params.path, &mut out)?;
            }
            method::WRITE_FILE => {
                let params: WriteFileParams = parse_params!(req.params, req.id, &mut out);
                handle_write_file(&sandbox, req.id, &params.path, &params.content, &mut out)?;
            }
            method::CHECKPOINT => {
                let params: CheckpointParams = parse_params!(req.params, req.id, &mut out);
                handle_checkpoint(&sandbox, prepared, req.id, &params.name, &mut out)?;
                let _ = sandbox.stop();
                return Ok(0);
            }
            _ => {
                send_error(
                    &mut out,
                    req.id,
                    METHOD_NOT_FOUND,
                    format!("method not found: {}", req.method),
                )?;
            }
        }
    }

    let _ = sandbox.stop();
    Ok(0)
}

fn handle_exec(
    sandbox: &Sandbox,
    id: serde_json::Value,
    argv: &[String],
    out: &mut impl Write,
) -> Result<()> {
    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();

    let exit_code = match sandbox.exec(argv, &mut stdout_buf, &mut stderr_buf) {
        Ok(code) => code,
        Err(e) => {
            return send_error(out, id, SERVER_ERROR, format!("exec failed: {}", e));
        }
    };

    send_result(
        out,
        id,
        ExecResultPayload {
            stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
            exit_code,
        },
    )
}

fn handle_read_file(
    sandbox: &Sandbox,
    id: serde_json::Value,
    path: &str,
    out: &mut impl Write,
) -> Result<()> {
    match sandbox.read_file(path) {
        Ok(data) => send_result(
            out,
            id,
            ReadFileResultPayload {
                content: BASE64.encode(&data),
            },
        ),
        Err(e) => send_error(out, id, SERVER_ERROR, format!("{}", e)),
    }
}

fn handle_write_file(
    sandbox: &Sandbox,
    id: serde_json::Value,
    path: &str,
    content: &str,
    out: &mut impl Write,
) -> Result<()> {
    let data = match BASE64.decode(content) {
        Ok(d) => d,
        Err(e) => {
            return send_error(out, id, INVALID_PARAMS, format!("invalid base64: {}", e));
        }
    };

    match sandbox.write_file(path, &data) {
        Ok(()) => send_result(out, id, EmptyResult {}),
        Err(e) => send_error(out, id, SERVER_ERROR, format!("{}", e)),
    }
}

fn handle_checkpoint(
    sandbox: &Sandbox,
    prepared: &PreparedVm,
    id: serde_json::Value,
    name: &str,
    out: &mut impl Write,
) -> Result<()> {
    let mut discard_out = Vec::new();
    let mut discard_err = Vec::new();
    if let Err(e) = sandbox.exec(&["sync"], &mut discard_out, &mut discard_err) {
        return send_error(out, id, SERVER_ERROR, format!("checkpoint sync failed: {}", e));
    }

    let data_dir = shuru_vm::default_data_dir();
    let checkpoints_dir = format!("{}/checkpoints", data_dir);
    let checkpoint_path = format!("{}/{}.ext4", checkpoints_dir, name);

    if let Err(e) = std::fs::create_dir_all(&checkpoints_dir) {
        return send_error(
            out,
            id,
            SERVER_ERROR,
            format!("failed to create checkpoints dir: {}", e),
        );
    }

    if let Err(e) = vm::clone_file(&prepared.work_rootfs, &checkpoint_path) {
        return send_error(out, id, SERVER_ERROR, format!("checkpoint clone failed: {}", e));
    }

    send_result(out, id, EmptyResult {})
}
