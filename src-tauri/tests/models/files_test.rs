//! Tests for local model file naming and scanning.
//!
//! Compiled as a child module of `models::files` (see the `#[path]`
//! declaration at the bottom of src/models/files.rs), so private items are
//! accessible.

use super::*;

// ---------------------------------------------------------------------------
// File name validation
// ---------------------------------------------------------------------------

#[test]
fn valid_file_names_pass() {
    assert!(validate_file_name("model.gguf").is_ok());
    assert!(validate_file_name("Llama-3.2-3B-Instruct-Q4_K_M.gguf").is_ok());
    // Whisper's GGML speech models are .bin, and go in the same directory
    // through the same downloader.
    assert!(validate_file_name("ggml-base.en.bin").is_ok());
}

#[test]
fn invalid_file_names_fail() {
    for name in [
        "",
        "model.txt",
        "model.gguf.part",
        ".hidden.gguf",
        "../escape.gguf",
        "../escape.bin",
        "dir/model.gguf",
        "dir\\model.bin",
    ] {
        assert!(validate_file_name(name).is_err(), "should reject {name:?}");
    }
}

// ---------------------------------------------------------------------------
// Local model scanning
// ---------------------------------------------------------------------------

#[test]
fn scan_lists_speech_models_too() {
    // Whisper's model is .bin and shares this directory. If the scan skipped
    // it, a downloaded speech model would look absent and its card would
    // offer to download it again forever.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ggml-base.en.bin"), [0u8; 11]).unwrap();
    std::fs::write(dir.path().join("ggml-small.en.bin.part"), [0u8; 4]).unwrap();

    let models = scan_models_dir(dir.path()).unwrap();
    assert_eq!(
        models,
        vec![
            LocalModel {
                file: "ggml-base.en.bin".into(),
                size_bytes: 11,
                partial: false
            },
            LocalModel {
                file: "ggml-small.en.bin".into(),
                size_bytes: 4,
                partial: true
            },
        ]
    );
}

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
