//! All tests for the models module: validation and scanning logic, plus the
//! download engine exercised against a real local HTTP server.
//!
//! Compiled as a child module of `models` (see the `#[path]` declaration at
//! the bottom of src/models.rs), so private items are accessible.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::manifest::ModelEntry;
use crate::test_helpers::spawn_server;

// ---------------------------------------------------------------------------
// File name validation
// ---------------------------------------------------------------------------

#[test]
fn valid_file_names_pass() {
    assert!(validate_file_name("model.gguf").is_ok());
    assert!(validate_file_name("Llama-3.2-3B-Instruct-Q4_K_M.gguf").is_ok());
}

#[test]
fn invalid_file_names_fail() {
    for name in [
        "",
        "model.bin",
        "model.gguf.part",
        ".hidden.gguf",
        "../escape.gguf",
        "dir/model.gguf",
        "dir\\model.gguf",
    ] {
        assert!(validate_file_name(name).is_err(), "should reject {name:?}");
    }
}

// ---------------------------------------------------------------------------
// File name derivation from download URLs
// ---------------------------------------------------------------------------

#[test]
fn file_name_from_url_takes_last_path_segment() {
    assert_eq!(
        file_name_from_url(
            "https://huggingface.co/Qwen/Qwen3-4B-GGUF/resolve/main/Qwen3-4B-Q4_K_M.gguf"
        )
        .unwrap(),
        "Qwen3-4B-Q4_K_M.gguf"
    );
    // Query strings must not leak into the file name.
    assert_eq!(
        file_name_from_url("https://example.com/models/m.gguf?download=true").unwrap(),
        "m.gguf"
    );
}

#[test]
fn file_name_from_url_rejects_bad_urls() {
    for url in [
        "not a url",
        "ftp://example.com/m.gguf",
        "https://example.com/",
        "https://example.com/model.bin",
        "https://example.com/.hidden.gguf",
    ] {
        assert!(file_name_from_url(url).is_err(), "should reject {url:?}");
    }
}

// ---------------------------------------------------------------------------
// Settings persistence
// ---------------------------------------------------------------------------

#[test]
fn settings_parse_defaults_and_roundtrip() {
    let empty: AppSettings = serde_json::from_str("{}").unwrap();
    assert!(empty.active_model_file.is_none());

    let settings = AppSettings {
        active_model_file: Some("model.gguf".into()),
    };
    let raw = serde_json::to_string(&settings).unwrap();
    let parsed: AppSettings = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed.active_model_file.as_deref(), Some("model.gguf"));

    // Unknown keys from future versions must not break parsing.
    let forward: AppSettings =
        serde_json::from_str(r#"{"activeModelFile":"a.gguf","newField":1}"#).unwrap();
    assert_eq!(forward.active_model_file.as_deref(), Some("a.gguf"));
}

// ---------------------------------------------------------------------------
// Local model scanning
// ---------------------------------------------------------------------------

#[test]
fn scan_lists_finals_and_partials() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.gguf"), [0u8; 5]).unwrap();
    std::fs::write(dir.path().join("b.gguf.part"), [0u8; 3]).unwrap();
    // Final and partial for the same model: final must win.
    std::fs::write(dir.path().join("c.gguf"), [0u8; 7]).unwrap();
    std::fs::write(dir.path().join("c.gguf.part"), [0u8; 2]).unwrap();
    // Unrelated files are ignored.
    std::fs::write(dir.path().join("notes.txt"), [0u8; 9]).unwrap();
    std::fs::write(dir.path().join("d.txt.part"), [0u8; 9]).unwrap();

    let models = scan_models_dir(dir.path()).unwrap();
    assert_eq!(
        models,
        vec![
            LocalModel {
                file: "a.gguf".into(),
                size_bytes: 5,
                partial: false
            },
            LocalModel {
                file: "b.gguf".into(),
                size_bytes: 3,
                partial: true
            },
            LocalModel {
                file: "c.gguf".into(),
                size_bytes: 7,
                partial: false
            },
        ]
    );
}

#[test]
fn scan_missing_dir_errors() {
    assert!(scan_models_dir(std::path::Path::new("/nonexistent/xyz")).is_err());
}

// ---------------------------------------------------------------------------
// Download engine (against a real local HTTP server)
// ---------------------------------------------------------------------------

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

    let outcome = run_download(&entry, &final_path, &cancel, |d, t| {
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
            let _ =
                request.respond(tiny_http::Response::from_data(vec![]).with_status_code(500));
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("test.gguf");
    std::fs::write(dir.path().join("test.gguf.part"), &payload[..40_000]).unwrap();
    let entry = entry_for(format!("http://127.0.0.1:{port}/test.gguf"));
    let cancel = AtomicBool::new(false);

    let outcome = run_download(&entry, &final_path, &cancel, |_, _| {})
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

    let outcome = run_download(&entry, &final_path, &cancel, |_, _| {})
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

    let outcome = run_download(&entry, &final_path, &cancel, |_, _| {})
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

    let err = run_download(&entry, &dir.path().join("test.gguf"), &cancel, |_, _| {})
        .await
        .unwrap_err();
    assert!(err.contains("404"), "unexpected error: {err}");
}
