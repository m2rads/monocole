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
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_parses() {
        let manifest = bundled().expect("bundled manifest must parse");
        assert_eq!(manifest.version, 1);
        assert!(!manifest.models.is_empty());
    }

    #[test]
    fn entries_are_well_formed() {
        let manifest = bundled().unwrap();
        for entry in &manifest.models {
            assert!(!entry.id.is_empty());
            assert!(!entry.name.is_empty());
            assert!(!entry.description.is_empty());
            assert!(!entry.quant.is_empty());
            assert!(entry.size_bytes > 0, "{}: sizeBytes must be set", entry.id);
            assert!(entry.min_ram_gb > 0, "{}: minRamGb must be set", entry.id);
            // File names become paths inside the models dir; they must pass
            // the same validation the commands enforce.
            crate::models::validate_file_name(&entry.file)
                .unwrap_or_else(|err| panic!("{}: {err}", entry.id));
            assert!(
                entry.url.starts_with("https://huggingface.co/"),
                "{}: unexpected download host: {}",
                entry.id,
                entry.url
            );
            assert!(
                entry.url.ends_with(&entry.file),
                "{}: url should point at `file`",
                entry.id
            );
            assert!(
                entry.url.contains(&entry.repo),
                "{}: url should belong to `repo`",
                entry.id
            );
        }
    }

    #[test]
    fn ids_and_files_are_unique() {
        let manifest = bundled().unwrap();
        let mut ids: Vec<_> = manifest.models.iter().map(|m| &m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), manifest.models.len(), "duplicate model id");

        let mut files: Vec<_> = manifest.models.iter().map(|m| &m.file).collect();
        files.sort();
        files.dedup();
        assert_eq!(files.len(), manifest.models.len(), "duplicate model file");
    }
}
