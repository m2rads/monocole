//! Integration tests for the download engine and llama-server client,
//! exercised against real local HTTP servers.
//!
//! Not covered here (by design): spawning a real llama-server (needs the
//! fetched binary plus a multi-GB model) and BLE (needs hardware). The
//! sidecar lifecycle is kept thin and its pure parts are unit-tested in
//! `src/llama.rs`.

use std::io::Read;
use std::sync::atomic::AtomicBool;

use app_lib::llama::{self, ChatMessage, StreamEvent};
use app_lib::manifest::ModelEntry;
use app_lib::models::{self, Outcome};

fn spawn_server(handler: impl Fn(tiny_http::Request) + Send + 'static) -> u16 {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            handler(request);
        }
    });
    port
}

fn test_payload() -> Vec<u8> {
    (0..100_000u32).map(|i| (i % 251) as u8).collect()
}

// tiny_http streams responses with chunked encoding (no Content-Length), so
// these tests also exercise the fallback where progress totals come from the
// manifest's size_bytes instead of the response headers.
fn entry_for(url: String) -> ModelEntry {
    ModelEntry {
        id: "test-model".into(),
        name: "Test model".into(),
        description: "test".into(),
        repo: "test/repo".into(),
        file: "test.gguf".into(),
        url,
        quant: "Q4_K_M".into(),
        size_bytes: 100_000,
        min_ram_gb: 1,
        sha256: None,
    }
}

#[tokio::test]
async fn fresh_download_completes() {
    let payload = test_payload();
    let body = payload.clone();
    let port = spawn_server(move |request| {
        let _ = request.respond(tiny_http::Response::from_data(body.clone()));
    });

    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("test.gguf");
    let entry = entry_for(format!("http://127.0.0.1:{port}/test.gguf"));
    let cancel = AtomicBool::new(false);
    let mut progress = Vec::new();

    let outcome = models::run_download(&entry, &final_path, &cancel, |d, t| {
        progress.push((d, t));
    })
    .await
    .unwrap();

    assert_eq!(outcome, Outcome::Completed { total: 100_000 });
    assert_eq!(std::fs::read(&final_path).unwrap(), payload);
    assert!(!dir.path().join("test.gguf.part").exists());
    assert_eq!(progress.first(), Some(&(0, 100_000)));
}

#[tokio::test]
async fn resume_sends_range_and_appends() {
    let payload = test_payload();
    let tail = payload[40_000..].to_vec();
    let port = spawn_server(move |request| {
        let range = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Range"))
            .map(|h| h.value.as_str().to_string());
        if range.as_deref() == Some("bytes=40000-") {
            let _ = request
                .respond(tiny_http::Response::from_data(tail.clone()).with_status_code(206));
        } else {
            // A resume that doesn't send the right Range header is a bug.
            let _ = request.respond(tiny_http::Response::from_data(vec![]).with_status_code(500));
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("test.gguf");
    std::fs::write(dir.path().join("test.gguf.part"), &payload[..40_000]).unwrap();
    let entry = entry_for(format!("http://127.0.0.1:{port}/test.gguf"));
    let cancel = AtomicBool::new(false);

    let outcome = models::run_download(&entry, &final_path, &cancel, |_, _| {})
        .await
        .unwrap();

    assert_eq!(outcome, Outcome::Completed { total: 100_000 });
    assert_eq!(std::fs::read(&final_path).unwrap(), payload);
}

#[tokio::test]
async fn restarts_cleanly_when_server_ignores_range() {
    let payload = test_payload();
    let body = payload.clone();
    // Server ignores Range and always replies 200 with the full file.
    let port = spawn_server(move |request| {
        let _ = request.respond(tiny_http::Response::from_data(body.clone()));
    });

    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("test.gguf");
    // Stale partial content that must be thrown away, not appended to.
    std::fs::write(dir.path().join("test.gguf.part"), vec![0xFF; 40_000]).unwrap();
    let entry = entry_for(format!("http://127.0.0.1:{port}/test.gguf"));
    let cancel = AtomicBool::new(false);

    let outcome = models::run_download(&entry, &final_path, &cancel, |_, _| {})
        .await
        .unwrap();

    assert_eq!(outcome, Outcome::Completed { total: 100_000 });
    assert_eq!(std::fs::read(&final_path).unwrap(), payload);
}

#[tokio::test]
async fn cancel_keeps_part_file_for_resume() {
    let payload = test_payload();
    let port = spawn_server(move |request| {
        let _ = request.respond(tiny_http::Response::from_data(payload.clone()));
    });

    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("test.gguf");
    let entry = entry_for(format!("http://127.0.0.1:{port}/test.gguf"));
    let cancel = AtomicBool::new(true); // cancelled before the first chunk

    let outcome = models::run_download(&entry, &final_path, &cancel, |_, _| {})
        .await
        .unwrap();

    assert!(matches!(outcome, Outcome::Cancelled { .. }));
    assert!(!final_path.exists());
    assert!(dir.path().join("test.gguf.part").exists());
}

#[tokio::test]
async fn download_fails_on_http_error() {
    let port = spawn_server(|request| {
        let _ = request.respond(tiny_http::Response::from_data(vec![]).with_status_code(404));
    });

    let dir = tempfile::tempdir().unwrap();
    let entry = entry_for(format!("http://127.0.0.1:{port}/missing.gguf"));
    let cancel = AtomicBool::new(false);

    let err = models::run_download(&entry, &dir.path().join("test.gguf"), &cancel, |_, _| {})
        .await
        .unwrap_err();
    assert!(err.contains("404"), "unexpected error: {err}");
}

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
    llama::stream_completion(port, &messages, |event| events.push(event))
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
    llama::stream_completion(port, &[], |event| events.push(event))
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

    let err = llama::stream_completion(port, &[], |_| {})
        .await
        .unwrap_err();
    assert!(err.contains("503"), "unexpected error: {err}");
}

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

    let title = llama::request_title(
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

    let err = llama::request_title(port, vec![]).await.unwrap_err();
    assert!(err.contains("500"), "unexpected error: {err}");
}
