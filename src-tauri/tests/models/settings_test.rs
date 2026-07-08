//! Tests for settings persistence.
//!
//! Compiled as a child module of `models::settings` (see the `#[path]`
//! declaration at the bottom of src/models/settings.rs), so private items
//! are accessible.

use super::*;

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
