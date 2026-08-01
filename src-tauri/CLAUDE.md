# src-tauri

## Test layout (non-standard)

Test files live in `src-tauri/tests/` but are compiled as **in-crate modules**
via `#[path]` hooks at the bottom of each src file, so they get private access.
`autotests = false` in Cargo.toml — top-level `tests/*.rs` are NOT separate
integration-test crates here.

One file per domain, mirroring src, plus a shared `tests/helpers.rs` exposed as
`crate::test_helpers`. Download and chat tests run against real local
`tiny_http` servers rather than mocks.
