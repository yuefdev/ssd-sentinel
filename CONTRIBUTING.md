# Contributing to SSD Sentinel

Thank you for helping improve **SSD Sentinel**! Whether you're fixing a SMART attribute parser edge case, refining UI localization, improving NTFS forensic recovery accuracy, or polishing documentation, your contributions are welcome.

---

## Development Setup

### Prerequisites
- **Windows 10 or Windows 11 (x64)**
- **Rust** stable toolchain (`cargo`, `rustfmt`, `clippy`)
- **Node.js** `>= 22` and **npm**
- **smartmontools 7.5** placed locally (not checked into Git; see [THIRD_PARTY.md](THIRD_PARTY.md)):
  - `tools/smartmontools/bin/smartctl.exe`
  - `tools/smartmontools/bin/drivedb.h`
  - `tools/smartmontools/doc/COPYING.txt`
  - `tools/smartmontools/doc/checksums64.txt`

> **Tip:** You can test and develop most UI and Rust features **without touching physical disks** using `--snapshot-dir src-tauri/tests/fixtures`.

### Install Dependencies & Verify

```powershell
# 1. Install frontend dependencies
npm ci

# 2. Run TypeScript unit tests and verify production bundle
npm test
npm run build

# 3. Run Rust formatting, linter, core tests, and desktop unit tests
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -j 1 -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --no-fail-fast -j 1
cargo test --manifest-path src-tauri/Cargo.toml --no-fail-fast -j 1

# 4. Run Playwright mock-IPC UI smoke tests across TR/EN views & 390px mobile layout
cargo run --manifest-path src-tauri/Cargo.toml --no-default-features -j 1 --example export_fixture -- results/rust-fixture.json
node tools/smoke_tauri.mjs results/smoke-dev
```

---

## Coding & Safety Standards

1. **Strict Read-Only Boundaries:** Scanning, SMART telemetry, forensic parsing (`$MFT`, `$UsnJrnl`, `$I30`, carving), and preview must never write to target volumes. Mutating operations must always require deterministic modal confirmation tokens verified inside Rust `core.rs`.
2. **Honest Telemetry:** Never fabricate a `100%` score or `up_to_date` status when a required SMART read or network check fails or lacks data. Surface partial warnings cleanly via `read_incomplete` / `partial_warnings`.
3. **Zero Privacy Leakage in Fixtures & Tests:** Never commit real disk serial numbers, WWN/EUI-64 identifiers, user paths, or machine telemetry captures. Keep fixtures synthetic (`src-tauri/tests/fixtures/`).
4. **Bilingual Parity (`TR` / `EN`):** User-facing UI strings, badges, tooltips, and dialogs should support both Turkish and English cleanly via `L(tr, en)` and `src/i18n.ts`.
5. **Minimal External Bloat:** Keep dependencies lean, respect CSP rules in `src-tauri/tauri.conf.json`, and preserve upstream license headers (MIT for project code; GPLv2+ notice for `smartmontools`).

---

## Submitting a Pull Request

1. Fork [`yuefdev/ssd-sentinel`](https://github.com/yuefdev/ssd-sentinel) and create a focused feature/fix branch from `main`.
2. Add or update unit tests (`src-tauri/tests/`, crate unit tests, or `tests/ui.test.ts`) for behavior changes.
3. Verify all CI checks pass locally (`npm test`, `npm run build`, `cargo fmt --check`, `cargo clippy`, `cargo test`).
4. Fill out the PR checklist and explain what hardware/snapshot scenarios you verified.
