# Contributing to SSD Sentinel

Thank you for helping improve **SSD Sentinel**! Before making changes—especially to storage, recovery, or Win32 I/O modules—please review [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/SAFETY_SPEC.md`](docs/SAFETY_SPEC.md).

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

# 5. (Elevated PowerShell) Run real cross-volume NTFS VHDX integration suite
pwsh -NoProfile -File tools/vhdx_integration_test.ps1
```

---

## Coding & Safety Standards

1. **Default Read-Only Guard & Strict Read-Only Boundaries:** Keep `Core::read_only_guard` defaulting to `true` and enforce `require_write_unlocked` on every mutating IPC command. Scanning, SMART telemetry, forensic parsing (`$MFT`, `$UsnJrnl`, `$I30`, carving), and preview must never write to target volumes.
2. **Dry-Run Preview & Confirmation Tokens:** Mutating UI flows must present an accurate `DryRunSpec` manifest inside `#confirmation` and pass deterministic IPC confirmation tokens validated in Rust `core.rs`.
3. **Honest Telemetry:** Never fabricate a `100%` score or `up_to_date` status when a required SMART read or network check fails or lacks data. Surface partial warnings cleanly via `read_incomplete` / `partial_warnings`.
4. **Zero Privacy Leakage in Fixtures & Tests:** Never commit real disk serial numbers, WWN/EUI-64 identifiers, user paths, or machine telemetry captures. Keep fixtures synthetic (`src-tauri/tests/fixtures/`).
5. **Bilingual Parity (`TR` / `EN`):** User-facing UI strings, badges, tooltips, and dialogs must support both Turkish and English cleanly via `L(tr, en)` and `src/i18n.ts`.
6. **Clean-Room & Minimal External Bloat:** Implement storage logic against documented Windows APIs only, keep dependencies lean, pass `npm audit` / `cargo audit`, respect CSP rules in `src-tauri/tauri.conf.json`, and preserve upstream license headers (MIT for project code; GPLv2+ notice for `smartmontools`).

---

## Release & Staging Channels

- **Staging Dry-Run:** Trigger `.github/workflows/release.yml` via `workflow_dispatch` to validate NSIS bundling and inspect staged build artifacts without publishing a public GitHub Release.
- **Release Candidates (`v*-rc.*`, `v*-beta.*`):** Pushing an RC/beta tag builds, verifies, attests (`attest-build-provenance`), and publishes a GitHub **Pre-Release** for staging verification.
- **Production (`vX.Y.Z`):** Pushing a stable semver tag publishes a full GitHub Release with `SHA256SUMS.txt` and Build Provenance attestation.

---

## Submitting a Pull Request

1. Fork [`yuefdev/ssd-sentinel`](https://github.com/yuefdev/ssd-sentinel) and create a focused feature/fix branch from `main`.
2. Add or update unit/integration tests (`src-tauri/tests/`, crate unit tests, or `tests/ui.test.ts`) for behavior changes.
3. Verify all CI checks pass locally (`npm test`, `npm run build`, `cargo fmt --check`, `cargo clippy`, `cargo test`, `smoke_tauri.mjs`).
4. Fill out the PR checklist and explain what hardware/VHDX/snapshot scenarios you verified.
