## Summary
<!-- Briefly describe what this PR changes and why. -->

## Related Issue
<!-- Fixes #... -->

## Type of Change
- [ ] Bug fix (non-breaking)
- [ ] New feature / hardware support
- [ ] Translation / UI polish
- [ ] Documentation / CI / build improvement

## Safety & Verification Checklist
- [ ] Zero real drive serial numbers, paths, or machine telemetry added to tests/fixtures
- [ ] Any disk-mutating operation requires UI confirmation + Rust IPC token validation
- [ ] Bilingual `TR` / `EN` copy updated (`src/i18n.ts`, `src/main.ts`, or `desktop.rs`)
- [ ] `npm test` and `npm run build` pass cleanly
- [ ] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` pass cleanly
- [ ] Tested with `--snapshot-dir src-tauri/tests/fixtures` (and/or live read-only check if hardware-specific)
