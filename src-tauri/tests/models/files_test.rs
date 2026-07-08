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
