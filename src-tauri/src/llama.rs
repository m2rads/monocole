use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex;

use crate::models;

pub const CHAT_EVENT: &str = "chat-stream";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct LlamaState(Mutex<Option<LlamaProcess>>);

pub struct LlamaProcess {
    child: Child,
    port: u16,
    model_path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatEvent {
    session_id: String,
    /// "token" | "done" | "error"
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlamaStatus {
    running: bool,
    port: Option<u16>,
    model_file: Option<String>,
}

fn server_binary(app: &AppHandle) -> Result<PathBuf, String> {
    // Dev: the folder scripts/fetch-llama-server.sh populates.
    // TODO(packaging): for bundled builds, ship binaries/llama/ as a
    // resource (or switch to externalBin + a static build) and resolve via
    // app.path().resource_dir() here.
    let path = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries/llama/llama-server")
    } else {
        app.path()
            .resource_dir()
            .map_err(|err| err.to_string())?
            .join("binaries/llama/llama-server")
    };
    if !path.is_file() {
        return Err(format!(
            "llama-server not found at {} — run scripts/fetch-llama-server.sh",
            path.display()
        ));
    }
    Ok(path)
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|err| err.to_string())
}

fn emit(app: &AppHandle, session_id: &str, kind: &'static str, content: Option<String>, message: Option<String>) {
    let _ = app.emit(
        CHAT_EVENT,
        ChatEvent {
            session_id: session_id.to_string(),
            kind,
            content,
            message,
        },
    );
}

/// Kill the sidecar synchronously; called from the app exit handler.
pub fn shutdown(app: &AppHandle) {
    if let Some(state) = app.try_state::<LlamaState>() {
        if let Ok(mut guard) = state.0.try_lock() {
            if let Some(mut proc) = guard.take() {
                let _ = proc.child.kill();
                let _ = proc.child.wait();
            }
        }
    }
}

/// Returns the port of a healthy llama-server running the active model,
/// (re)spawning it if needed (first call, crash, or model switch).
async fn ensure_running(app: &AppHandle, state: &LlamaState) -> Result<u16, String> {
    let model_path = models::active_model_path(app)?.ok_or(
        "No model selected. Download and activate a model under Settings → Models.",
    )?;

    let mut guard = state.0.lock().await;

    if let Some(proc) = guard.as_mut() {
        let alive = matches!(proc.child.try_wait(), Ok(None));
        if alive && proc.model_path == model_path {
            return Ok(proc.port);
        }
        let _ = proc.child.kill();
        let _ = proc.child.wait();
        *guard = None;
    }

    let binary = server_binary(app)?;
    let port = free_port()?;
    let mut child = Command::new(&binary)
        .args([
            "--model",
            model_path.to_str().ok_or("model path is not valid UTF-8")?,
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--ctx-size",
            "8192",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("failed to spawn llama-server: {err}"))?;

    // Wait for /health to go green; large models take a while to load.
    let client = reqwest::Client::new();
    let health_url = format!("http://127.0.0.1:{port}/health");
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("llama-server exited during startup ({status})"));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("llama-server did not become healthy in time".into());
        }
        let healthy = client
            .get(&health_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .map(|resp| resp.status().is_success())
            .unwrap_or(false);
        if healthy {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    *guard = Some(LlamaProcess {
        child,
        port,
        model_path,
    });
    Ok(port)
}

#[tauri::command]
pub async fn llama_status(
    _app: AppHandle,
    state: State<'_, LlamaState>,
) -> Result<LlamaStatus, String> {
    let mut guard = state.0.lock().await;
    let running = guard
        .as_mut()
        .map(|proc| matches!(proc.child.try_wait(), Ok(None)))
        .unwrap_or(false);
    Ok(LlamaStatus {
        running,
        port: guard.as_ref().map(|proc| proc.port),
        model_file: guard.as_ref().and_then(|proc| {
            proc.model_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        }),
    })
}

/// Streams a chat completion; tokens arrive on the `chat-stream` event tagged
/// with `session_id`. The command resolves when the stream ends.
#[tauri::command]
pub async fn chat_stream(
    app: AppHandle,
    state: State<'_, LlamaState>,
    session_id: String,
    messages: Vec<ChatMessage>,
) -> Result<(), String> {
    let result = run_chat_stream(&app, &state, &session_id, messages).await;
    if let Err(err) = &result {
        emit(&app, &session_id, "error", None, Some(err.clone()));
    }
    result
}

async fn run_chat_stream(
    app: &AppHandle,
    state: &LlamaState,
    session_id: &str,
    messages: Vec<ChatMessage>,
) -> Result<(), String> {
    let port = ensure_running(app, state).await?;
    stream_completion(port, &messages, |event| match event {
        StreamEvent::Token(token) => emit(app, session_id, "token", Some(token), None),
        StreamEvent::Done => emit(app, session_id, "done", None, None),
    })
    .await
}

#[derive(Debug, PartialEq, Eq)]
pub enum StreamEvent {
    Token(String),
    Done,
}

/// Parses one SSE line from llama-server's OpenAI-compatible stream.
/// Returns None for keep-alives, empty deltas, and unparseable lines.
fn parse_sse_line(line: &str) -> Option<StreamEvent> {
    let data = line.trim().strip_prefix("data: ")?;
    if data == "[DONE]" {
        return Some(StreamEvent::Done);
    }
    let value = serde_json::from_str::<serde_json::Value>(data).ok()?;
    let token = value["choices"][0]["delta"]["content"].as_str()?;
    if token.is_empty() {
        None
    } else {
        Some(StreamEvent::Token(token.to_string()))
    }
}

/// Streams a chat completion from a llama-server on `port`, invoking
/// `on_event` per token and once for Done. Separated from the sidecar
/// lifecycle so it can be exercised against any HTTP server in tests.
pub async fn stream_completion(
    port: u16,
    messages: &[ChatMessage],
    mut on_event: impl FnMut(StreamEvent),
) -> Result<(), String> {
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&json!({ "messages": messages, "stream": true }))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("llama-server returned {status}: {body}"));
    }

    // TODO: support cancelling generation (drop the stream + POST /v1/…
    // abort) when the UI grows a stop button.
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(newline) = buffer.find('\n') {
            let line = buffer[..newline].to_string();
            buffer.drain(..=newline);
            match parse_sse_line(&line) {
                Some(StreamEvent::Done) => {
                    on_event(StreamEvent::Done);
                    return Ok(());
                }
                Some(event) => on_event(event),
                None => {}
            }
        }
    }

    on_event(StreamEvent::Done);
    Ok(())
}

/// One-shot, non-streaming completion that names a session from its first
/// exchange. Returns a cleaned-up short title.
#[tauri::command]
pub async fn generate_session_title(
    app: AppHandle,
    state: State<'_, LlamaState>,
    messages: Vec<ChatMessage>,
) -> Result<String, String> {
    let port = ensure_running(&app, &state).await?;
    request_title(port, messages).await
}

/// One-shot, non-streaming completion against a llama-server on `port`.
/// Separated from the sidecar lifecycle for testability.
pub async fn request_title(port: u16, messages: Vec<ChatMessage>) -> Result<String, String> {
    let mut prompt_messages = vec![ChatMessage {
        role: "system".into(),
        content: "You name conversations. Reply with a title for the conversation, \
                  3 to 6 words, plain text only: no quotes, no trailing punctuation, \
                  no explanations."
            .into(),
    }];
    prompt_messages.extend(messages);

    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .json(&json!({
            "messages": prompt_messages,
            "stream": false,
            "max_tokens": 400,
            "temperature": 0.3,
        }))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("llama-server returned {}", response.status()));
    }

    let value: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    let raw = value["choices"][0]["message"]["content"]
        .as_str()
        .ok_or("no title in response")?;
    Ok(clean_title(raw))
}

/// Strips reasoning blocks, quotes and length so model output is safe to use
/// as a sidebar title.
fn clean_title(raw: &str) -> String {
    let mut text = raw.to_string();
    // Reasoning models (e.g. Qwen3) may prepend a <think>…</think> block.
    if let (Some(start), Some(end)) = (text.find("<think>"), text.find("</think>")) {
        if start < end {
            text.replace_range(start..end + "</think>".len(), "");
        }
    }
    let cleaned = text
        .trim()
        .trim_matches(['"', '\'', '“', '”'])
        .trim_end_matches(['.', '!'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut title: String = cleaned.chars().take(60).collect();
    if title.is_empty() {
        title = "New Session".into();
    }
    title
}

#[cfg(test)]
#[path = "../tests/llama_test.rs"]
mod tests;
