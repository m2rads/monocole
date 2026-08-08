//! Wi-Fi data plane client.
//!
//! The monocle listens, the app connects. Every message is
//!
//! ```text
//! [len: u32 big-endian][type: u8][payload ...]
//! ```
//!
//! where `len` counts the type byte plus the payload, so it is always >= 1.
//! Keep in sync with the firmware's `tcp_server.c` and docs/protocol.md.
//!
//! Connections are opened per operation and closed after, matching the power
//! model: the radio is up for a burst, not for a session.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const MONOCLE_TCP_PORT: u16 = 3333;

const FRAME_HEADER_LEN: usize = 5;
/// Mirrors MONOCLE_MAX_PAYLOAD on the device; also caps what a rogue peer can
/// make us allocate per frame.
const MAX_PAYLOAD: usize = 8192;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

const FRAME_ECHO_REQ: u8 = 1;
const FRAME_ECHO_RESP: u8 = 2;
const FRAME_BULK_REQ: u8 = 3;
const FRAME_BULK_DATA: u8 = 4;
const FRAME_BULK_END: u8 = 5;

/// Largest transfer a single benchmark may request (8 MB), so a typo cannot
/// tie up the device for minutes.
const MAX_BENCHMARK_BYTES: u32 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EchoResult {
    pub bytes: usize,
    pub round_trip_ms: u128,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkResult {
    pub bytes: u32,
    pub elapsed_ms: u128,
    pub kbps: u64,
    /// Frames received, so a pathologically chunked transfer is visible in the
    /// result rather than hidden behind an average.
    pub frames: u32,
}

fn encode_frame(frame_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
    out.extend_from_slice(&((payload.len() as u32) + 1).to_be_bytes());
    out.push(frame_type);
    out.extend_from_slice(payload);
    out
}

/// Splits a frame header into (type, payload length), rejecting lengths the
/// device would never send.
fn decode_header(header: &[u8; FRAME_HEADER_LEN]) -> Result<(u8, usize), String> {
    let len = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if len == 0 {
        return Err("frame length must include the type byte".into());
    }
    let payload_len = len - 1;
    if payload_len > MAX_PAYLOAD {
        return Err(format!(
            "frame payload of {payload_len} bytes exceeds the {MAX_PAYLOAD}-byte maximum"
        ));
    }
    Ok((header[4], payload_len))
}

async fn read_frame(stream: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|err| format!("reading frame header: {err}"))?;

    let (frame_type, payload_len) = decode_header(&header)?;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|err| format!("reading frame payload: {err}"))?;
    }
    Ok((frame_type, payload))
}

/// Resolves the address the device reported over BLE into a socket address.
fn device_addr(ip: &str) -> Result<SocketAddr, String> {
    let addr: IpAddr = ip
        .parse()
        .map_err(|_| format!("{ip} is not a valid IP address"))?;
    Ok(SocketAddr::new(addr, MONOCLE_TCP_PORT))
}

async fn connect(addr: SocketAddr) -> Result<TcpStream, String> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| {
            format!("timed out connecting to {addr} — is the monocle's Wi-Fi up?")
        })?
        .map_err(|err| format!("could not connect to {addr}: {err}"))?;

    // Small frames should go out immediately; Nagle would add latency to the
    // echo round trip for no benefit.
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

/// Round-trips a payload. Proves the pipe end to end.
#[tauri::command]
pub async fn socket_echo(ip: String, payload: String) -> Result<EchoResult, String> {
    echo_at(device_addr(&ip)?, payload.as_bytes()).await
}

async fn echo_at(addr: SocketAddr, bytes: &[u8]) -> Result<EchoResult, String> {
    if bytes.len() > MAX_PAYLOAD {
        return Err(format!(
            "payload of {} bytes exceeds the {MAX_PAYLOAD}-byte maximum",
            bytes.len()
        ));
    }

    let mut stream = connect(addr).await?;
    let started = Instant::now();

    stream
        .write_all(&encode_frame(FRAME_ECHO_REQ, bytes))
        .await
        .map_err(|err| format!("sending echo request: {err}"))?;

    let (frame_type, echoed) = tokio::time::timeout(READ_TIMEOUT, read_frame(&mut stream))
        .await
        .map_err(|_| "timed out waiting for the echo reply".to_string())??;

    if frame_type != FRAME_ECHO_RESP {
        return Err(format!("expected an echo reply, got frame type {frame_type}"));
    }
    if echoed != bytes {
        return Err("the echoed payload did not match what was sent".into());
    }

    Ok(EchoResult {
        bytes: echoed.len(),
        round_trip_ms: started.elapsed().as_millis(),
    })
}

/// Pulls `total_bytes` of synthetic data and times it.
///
/// This is the measurement the two-radio architecture rests on: whether a
/// Wi-Fi burst is fast enough, with BLE connected, to be worth a second radio.
#[tauri::command]
pub async fn socket_benchmark(ip: String, total_bytes: u32) -> Result<BenchmarkResult, String> {
    benchmark_at(device_addr(&ip)?, total_bytes).await
}

async fn benchmark_at(addr: SocketAddr, total_bytes: u32) -> Result<BenchmarkResult, String> {
    if total_bytes == 0 {
        return Err("ask for at least one byte".into());
    }
    if total_bytes > MAX_BENCHMARK_BYTES {
        return Err(format!(
            "{total_bytes} bytes exceeds the {MAX_BENCHMARK_BYTES}-byte limit"
        ));
    }

    let mut stream = connect(addr).await?;

    stream
        .write_all(&encode_frame(FRAME_BULK_REQ, &total_bytes.to_be_bytes()))
        .await
        .map_err(|err| format!("sending bulk request: {err}"))?;

    // Timed from the request going out, so the device's own setup counts.
    let started = Instant::now();
    let mut received: u32 = 0;
    let mut frames: u32 = 0;

    loop {
        let (frame_type, payload) = tokio::time::timeout(READ_TIMEOUT, read_frame(&mut stream))
            .await
            .map_err(|_| {
                format!("timed out after {received} of {total_bytes} bytes")
            })??;

        match frame_type {
            FRAME_BULK_DATA => {
                received = received.saturating_add(payload.len() as u32);
                frames += 1;
            }
            FRAME_BULK_END => {
                if payload.len() != 4 {
                    return Err("bulk end frame must carry a 4-byte count".into());
                }
                let claimed =
                    u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if claimed != received {
                    return Err(format!(
                        "device reported sending {claimed} bytes but {received} arrived"
                    ));
                }
                break;
            }
            other => return Err(format!("unexpected frame type {other} during transfer")),
        }
    }

    if received != total_bytes {
        return Err(format!(
            "asked for {total_bytes} bytes but received {received}"
        ));
    }

    let elapsed = started.elapsed();
    let elapsed_ms = elapsed.as_millis().max(1);
    let kbps = (u64::from(received) * 8) / elapsed_ms as u64;

    Ok(BenchmarkResult {
        bytes: received,
        elapsed_ms,
        kbps,
        frames,
    })
}

#[cfg(test)]
#[path = "../tests/socket_test.rs"]
mod tests;
