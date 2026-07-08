//! Unit tests for the bundled model manifest. Guards every future edit of
//! src-tauri/manifest/models.json.

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
        // File names become paths inside the models dir; they must pass the
        // same validation the commands enforce.
        crate::models::files::validate_file_name(&entry.file)
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
