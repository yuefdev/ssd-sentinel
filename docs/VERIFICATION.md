# SSD Sentinel v1.9.0 Verification Report

All checks below were executed on **Windows (x64)** against the final `v1.9.0` release commit (`yuefdev/ssd-sentinel`).

---

## 1. Automated Verification Summary

| Suite | Command | Result |
|---|---|---|
| Rust Formatting | `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | Passed (exit `0`) |
| Rust Linter | `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -j 1 -- -D warnings` | Passed (`0` warnings) |
| Rust Core Tests (`--no-default-features`) | `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --no-fail-fast -j 1` | `109` passed (`75` lib + `9` `model_store` + `25` `platform_jobs`), `0` failed |
| Rust Full Desktop Tests (`desktop` feature) | `cargo test --manifest-path src-tauri/Cargo.toml --no-fail-fast -j 1` | `114` passed (`80` lib + `9` `model_store` + `25` `platform_jobs`), `0` failed |
| Frontend Unit Tests | `npm test` | `20` passed, `0` failed |
| TypeScript & Vite Production Build | `npm run build` (`tsc --noEmit && vite build`) | Passed (exit `0`) |
| Playwright Mock-IPC UI Smoke (TR + EN + 390px + GitHub Updater UI) | `node tools/smoke_tauri.mjs results/tauri-ui-v19` | `23` checks passed, `0` errors |
| Tauri Release + NSIS Build (`SSD_SENTINEL_UPDATE_REPO=yuefdev/ssd-sentinel`) | `npm run tauri -- build --bundles nsis` | Built `ssd-sentinel.exe` and `SSD Sentinel_1.9.0_x64-setup.exe` |
| Native WebView2 Release Replay Smoke | `node tools/smoke_tauri_native.mjs results/tauri-native-v19` | `8` checks passed, `0` errors, clean `WM_CLOSE` exit `0` |
| Live Hardware Read-Only CDP Smoke | `node results/live_readonly_check.mjs results/tauri-live-v19` | Passed (`source="live"`, `stale=false`, NVMe `read_incomplete=false`, non-admin SATA warning banner isolated, `WM_CLOSE` exit `0`) |

---

## 2. Live Hardware Behavior Notes (Read-Only Probe)

Testing the un-elevated (`admin=false`) `v1.9.0` binary against real physical storage confirmed per-disk SMART fault isolation:

- `WDC WD10EZEX-21M2NA0` (`/dev/sda`, SATA HDD behind non-admin controller restriction): flagged `read_incomplete: true`, `health_percent: null`, reported in `.warning-banner` (`partial_warnings_count: 1`).
- `GIGABYTE AG4512G-SI B10` (NVMe SSD): remained live (`read_incomplete: false`, `70%`).
- `HS-SSD-Desire(P) 1024G` (NVMe SSD): remained live (`read_incomplete: false`, `98%`).
- Global telemetry state remained `stale: false`. No destructive writes, benchmarks, or optimizations were executed during the live probe.

---

## 3. Local Build Artifacts & SHA-256 Checksums

Built locally for `yuefdev/ssd-sentinel` (`v1.9.0`):

| Artifact | Size | SHA-256 |
|---|---|---|
| `src-tauri/target/release/ssd-sentinel.exe` | `12,976,640` bytes | `BF0530556086E80A3EF4D5023E6AFEE334A5ED14C9F046467F433B27077DE354` |
| `src-tauri/target/release/bundle/nsis/SSD Sentinel_1.9.0_x64-setup.exe` | `3,719,036` bytes | `9AEDEC624AFF70F8057CCB1F32512653D5228964AE7113D919F6D60314BAD1EB` |

Public GitHub Releases publish CI-built `SSD-Sentinel_<version>_x64-setup.exe`, upstream `smartmontools-7.5-RELEASE_7_5-source.tar.gz`, and `SHA256SUMS.txt` verified in-app before launching updates.
