//! Minimal HTTP/1.1 client for the cloud-hypervisor API socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::error::{Result, VzError};

/// PUT `/api/v1/<endpoint>` with an optional JSON body.
pub(crate) fn put(socket: &str, endpoint: &str, body: Option<&str>) -> Result<()> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| VzError::new(format!("connect to {}: {}", socket, e)))?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));

    let body = body.unwrap_or("");
    let request = format!(
        "PUT /api/v1/{} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        endpoint,
        body.len(),
        body
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| VzError::new(format!("{}: send: {}", endpoint, e)))?;

    // Read the status line and headers, then a Content-Length body if any.
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| VzError::new(format!("{}: read: {}", endpoint, e)))?;
        if n == 0 {
            break buf.len();
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return Err(VzError::new(format!("{}: oversized response", endpoint)));
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VzError::new(format!("{}: malformed response", endpoint)))?;

    let content_length: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0);

    let mut rest = buf[header_end..].to_vec();
    while rest.len() < content_length {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| VzError::new(format!("{}: read body: {}", endpoint, e)))?;
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&chunk[..n]);
    }

    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(VzError::new(format!(
            "{}: HTTP {}: {}",
            endpoint,
            status,
            String::from_utf8_lossy(&rest).trim()
        )))
    }
}

/// Offset just past the `\r\n\r\n` separator, if present.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}
