//! All tests for the llama module: SSE parsing and title cleaning, plus the
//! llama-server HTTP client (chat streaming, title generation) exercised
//! against a real local HTTP server.
//!
//! Compiled as a child module of `llama` (see the `#[path]` declaration at
//! the bottom of src/llama.rs), so private items are accessible.
//!
//! Not covered (by design): spawning a real llama-server — that needs the
//! fetched binary plus a multi-GB model, and stays a manual test.

use super::*;
use crate::test_helpers::spawn_server;

// ---------------------------------------------------------------------------
// Port picking
// ---------------------------------------------------------------------------

#[test]
fn free_port_returns_bindable_port() {
    let port = free_port().unwrap();
    assert_ne!(port, 0);
    // The port was released when the probe listener dropped, so binding it
    // again must work.
    std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
}

// ---------------------------------------------------------------------------
// SSE line parsing
// ---------------------------------------------------------------------------

#[test]
fn parse_sse_token_lines() {
    assert_eq!(
        parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Hi"}}]}"#),
        Some(StreamEvent::Token("Hi".into()))
    );
    assert_eq!(parse_sse_line("data: [DONE]"), Some(StreamEvent::Done));
}

#[test]
fn parse_sse_ignores_noise() {
    assert_eq!(parse_sse_line(""), None);
    assert_eq!(parse_sse_line(": keep-alive"), None);
    assert_eq!(parse_sse_line("event: ping"), None);
    assert_eq!(parse_sse_line("data: not-json"), None);
    // Empty delta (role-only chunk) produces no token.
    assert_eq!(
        parse_sse_line(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#),
        None
    );
    assert_eq!(
        parse_sse_line(r#"data: {"choices":[{"delta":{"content":""}}]}"#),
        None
    );
}

// ---------------------------------------------------------------------------
// Title cleaning
// ---------------------------------------------------------------------------

#[test]
fn clean_title_strips_quotes_and_punctuation() {
    assert_eq!(clean_title(r#""Local Llama Setup.""#), "Local Llama Setup");
    assert_eq!(
        clean_title("  Chatting   about\nrust!  "),
        "Chatting about rust"
    );
    assert_eq!(clean_title("“Fancy Quotes”"), "Fancy Quotes");
}

#[test]
fn clean_title_strips_reasoning_block() {
    assert_eq!(
        clean_title("<think>the user wants a name</think>\nBLE Firmware Plan"),
        "BLE Firmware Plan"
    );
}

#[test]
fn clean_title_truncates_and_defaults() {
    let long = "word ".repeat(40);
    assert!(clean_title(&long).chars().count() <= 60);
    assert_eq!(clean_title(""), "New Session");
    assert_eq!(clean_title("<think>only thoughts</think>"), "New Session");
    assert_eq!(clean_title("\"\""), "New Session");
}

// ---------------------------------------------------------------------------
// Chat streaming (against a real local HTTP server)
// ---------------------------------------------------------------------------

fn sse_response(body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_data(body.as_bytes().to_vec())
}

#[tokio::test]
async fn chat_stream_emits_tokens_and_done() {
    let body = concat!(
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let port = spawn_server(move |request| {
        let _ = request.respond(sse_response(body));
    });

    let messages = vec![ChatMessage {
        role: "user".into(),
        content: "hi".into(),
    }];
    let mut events = Vec::new();
    stream_completion(port, &messages, |event| events.push(event))
        .await
        .unwrap();

    assert_eq!(
        events,
        vec![
            StreamEvent::Token("Hel".into()),
            StreamEvent::Token("lo".into()),
            StreamEvent::Done,
        ]
    );
}

#[tokio::test]
async fn chat_stream_synthesizes_done_when_stream_ends() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n";
    let port = spawn_server(move |request| {
        let _ = request.respond(sse_response(body));
    });

    let mut events = Vec::new();
    stream_completion(port, &[], |event| events.push(event))
        .await
        .unwrap();

    assert_eq!(
        events,
        vec![StreamEvent::Token("Hi".into()), StreamEvent::Done]
    );
}

#[tokio::test]
async fn chat_stream_fails_on_http_error() {
    let port = spawn_server(|request| {
        let _ = request.respond(
            tiny_http::Response::from_data(b"model overloaded".to_vec()).with_status_code(503),
        );
    });

    let err = stream_completion(port, &[], |_| {}).await.unwrap_err();
    assert!(err.contains("503"), "unexpected error: {err}");
}

// ---------------------------------------------------------------------------
// Title generation (against a real local HTTP server)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_title_prepends_system_prompt_and_cleans_result() {
    let port = spawn_server(|mut request| {
        let mut body = String::new();
        request.as_reader().read_to_string(&mut body).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        // The command must prepend its own system prompt before the chat.
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][1]["role"], "user");
        assert_eq!(value["stream"], false);

        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "<think>naming…</think>\n \"Local Llama Setup.\" "
                }
            }]
        });
        let _ = request.respond(tiny_http::Response::from_data(
            serde_json::to_vec(&response).unwrap(),
        ));
    });

    let title = request_title(
        port,
        vec![ChatMessage {
            role: "user".into(),
            content: "help me set up llama.cpp".into(),
        }],
    )
    .await
    .unwrap();

    assert_eq!(title, "Local Llama Setup");
}

#[tokio::test]
async fn request_title_fails_on_http_error() {
    let port = spawn_server(|request| {
        let _ = request.respond(tiny_http::Response::from_data(vec![]).with_status_code(500));
    });

    let err = request_title(port, vec![]).await.unwrap_err();
    assert!(err.contains("500"), "unexpected error: {err}");
}
