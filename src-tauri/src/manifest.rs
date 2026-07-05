use serde::{Deserialize, Serialize};

// TODO(config-server): this manifest is currently compiled into the binary.
// The plan is a three-tier fallback so the model catalog can change without
// shipping an app update:
//   1. remote:  fetch a signed models.json from a URL we control (static CDN
//      is enough), with ETag/If-None-Match so unchanged configs cost nothing
//   2. cached:  last successfully fetched copy, stored in the app data dir
//   3. bundled: the include_str! below, so first launch and offline work
// Merge order: remote > cached > bundled. When that lands, also populate
// `sha256` for every entry and verify checksums after download — the manifest
// tells clients what to fetch, so it must not be spoofable.
const BUNDLED_MANIFEST: &str = include_str!("../manifest/models.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelManifest {
    pub version: u32,
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Hugging Face repo the file comes from, e.g. "Qwen/Qwen3-4B-GGUF".
    pub repo: String,
    /// File name inside the repo, also used as the on-disk name.
    pub file: String,
    pub url: String,
    pub quant: String,
    /// Approximate download size; used for progress UI, not verification.
    pub size_bytes: u64,
    pub min_ram_gb: u32,
    /// TODO(config-server): required once the manifest is fetched remotely.
    pub sha256: Option<String>,
}

pub fn bundled() -> Result<ModelManifest, serde_json::Error> {
    serde_json::from_str(BUNDLED_MANIFEST)
}

#[tauri::command]
pub fn get_model_manifest() -> Result<ModelManifest, String> {
    bundled().map_err(|err| format!("invalid bundled model manifest: {err}"))
}

#[cfg(test)]
#[path = "../tests/manifest_test.rs"]
mod tests;
