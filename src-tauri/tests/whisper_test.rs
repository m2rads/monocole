//! whisper-server HTTP client, exercised against a real local HTTP server.
//!
//! Not covered (by design): spawning a real whisper-server, which needs the
//! binary from scripts/fetch-whisper-server.sh and a 148 MB model. What is
//! covered is everything that can go wrong in the conversation with it —
//! which is where the bugs that reach a user actually live.

use crate::test_helpers::spawn_server;

use super::*;

fn json_response(body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_data(body.as_bytes().to_vec()).with_header(
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    )
}

/// A server that answers every request with the given JSON.
fn serving(body: &'static str) -> u16 {
    spawn_server(move |request| {
        let _ = request.respond(json_response(body));
    })
}

#[tokio::test]
async fn returns_the_transcript() {
    let port = serving(r#"{"text":"turn on the kitchen light"}"#);
    let text = transcribe_wav(port, vec![0; 64]).await.unwrap();
    assert_eq!(text, "turn on the kitchen light");
}

#[tokio::test]
async fn trims_the_leading_space_whisper_adds() {
    // whisper habitually prefixes a space. Left in, it shows up in the chat
    // bubble and in the session title generated from it.
    let port = serving(r#"{"text":"  hello there  "}"#);
    assert_eq!(transcribe_wav(port, vec![0; 64]).await.unwrap(), "hello there");
}

#[tokio::test]
async fn an_empty_transcript_is_not_an_error() {
    // Silence, or speech whisper could not make out. The caller decides what
    // to do with it; failing here would turn a quiet room into an error
    // dialog.
    let port = serving(r#"{"text":""}"#);
    assert_eq!(transcribe_wav(port, vec![0; 64]).await.unwrap(), "");
}

#[tokio::test]
async fn a_server_error_is_reported_with_its_body() {
    let port = spawn_server(|request| {
        let _ = request.respond(
            tiny_http::Response::from_string("model failed to load").with_status_code(500),
        );
    });

    let err = transcribe_wav(port, vec![0; 64]).await.unwrap_err();
    assert!(err.contains("500"), "{err}");
    assert!(err.contains("model failed to load"), "{err}");
}

#[tokio::test]
async fn a_response_without_a_text_field_is_an_error() {
    // A different whisper build, or /inference answering something else
    // entirely. Silently returning "" would look like the user said nothing.
    let port = serving(r#"{"segments":[]}"#);
    let err = transcribe_wav(port, vec![0; 64]).await.unwrap_err();
    assert!(err.contains("no text field"), "{err}");
}

#[tokio::test]
async fn unreachable_server_is_an_error() {
    // Port 1 is reserved and nothing listens there.
    assert!(transcribe_wav(1, vec![0; 64]).await.is_err());
}

#[tokio::test]
async fn posts_the_wav_as_multipart_to_inference() {
    use std::io::Read;
    use std::sync::{Arc, Mutex};

    let seen: Arc<Mutex<Option<(String, String, usize)>>> = Arc::new(Mutex::new(None));
    let captured = seen.clone();

    let port = spawn_server(move |mut request| {
        let url = request.url().to_string();
        let method = request.method().to_string();
        let mut body = Vec::new();
        let _ = request.as_reader().read_to_end(&mut body);
        *captured.lock().unwrap() = Some((method, url, body.len()));
        let _ = request.respond(json_response(r#"{"text":"ok"}"#));
    });

    let wav = vec![7u8; 512];
    transcribe_wav(port, wav).await.unwrap();

    let (method, url, len) = seen.lock().unwrap().clone().unwrap();
    assert_eq!(method, "POST");
    assert_eq!(url, "/inference");
    // Multipart framing adds headers and boundaries, so the body is larger
    // than the audio — but the audio has to actually be in there.
    assert!(len > 512, "body was {len} bytes; the WAV is missing");
}
