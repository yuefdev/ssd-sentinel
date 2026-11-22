# Changelog

All notable changes to **SSD Sentinel** are documented in this file.

## [v1.10.0-rc.1] — 2026-11-22

### Added
- **Default-on Read-Only Guard:** `read_only_guard` is enforced at the Rust core boundary for every mutating path (`start_job`, `disk_action`, tier relocate/rollback, `optimize`), persisted in `preferences.json`, and exposed through a toolbar + Settings **Write Guard** toggle. Every confirmation dialog now shows a structured `DryRunSpec` execution manifest before anything is written.
- **NTFS safety test suite:** `ntfs_safety_integration` (fault-injected mid-copy cancel, forbidden-path firewall, read-only scanner immutability, read-only guard persistence) plus a real cross-volume VHDX NTFS junction/rollback harness (`tools/vhdx_integration_test.ps1`).
- **Formal docs:** `docs/ARCHITECTURE.md` and `docs/SAFETY_SPEC.md`; updated `SECURITY.md`, `README.md`, `CONTRIBUTING.md`.
- **CI/CD & supply chain:** Playwright UI smoke gate, VHDX NTFS CI job, `cargo-audit` + `npm audit`, `src-tauri/deny.toml`, RC/beta/alpha pre-release channel and manual staging artifacts, SLSA build provenance attestation on tagged releases.

### Changed
- Renamed the `surface_ai` module to `surface_triage`; the diagnosis engine is a deterministic statistics + rules engine (offline) and is labelled as such.
- **Precision Storage Lab UI:** steel-cobalt interactive accent separated from emerald health status, monospace tabular telemetry, flatter bordered panels, new shield mark, refreshed screenshots; removed consumer/AI copy and sparkle icons.

### Security
- Updated `rusqlite` to 0.43 (`libsqlite3-sys` 0.40, RUSTSEC-2026-0459), `tokio` to 1.54.1 (RUSTSEC-2026-0384), `url` to 2.5.9 (RUSTSEC-2026-0592), and `rustix` to 1.1.6.

## [v1.9.0] — 2026-11-21

### Added
- **Bilingual TR / EN Interface & Tray:** Instant `TR | EN` switcher in the sidebar and Settings (`#language-setting`) persisted across `localStorage`, `preferences.json`, and Windows system tray menus/tooltips.
- **Per-Disk SMART Fault Isolation (`read_incomplete`):** Failing or unprivileged ATA/SATA SMART reads stay isolated in a non-blocking `.warning-banner` (`partial_warnings`) while healthy NVMe/SATA drives remain live (`stale: false`).
- **In-App GitHub Updater (`yuefdev/ssd-sentinel`):** Queries GitHub Releases (`check_updates`), validates download hosts against strict GitHub domains, downloads `-setup.exe`, verifies against published `SHA256SUMS.txt` before launch (`download_and_launch_update`), and opens official release notes (`open_release_url`).
- **Minimalist Desktop Icon & Matching Brand Mark:** Custom geometric obsidian + emerald squircle app icon (`icon.ico`, `icon.png`, StoreLogos, `assets/sentinel.svg`).
- **Chunked Tiering Cancellation & Cleanup Warnings:** Real-time progress reporting (`CopyProgress`), `AtomicBool` cancellation between chunks, direct non-blocked Stop button (`cancel_job`), and non-fatal `cleanup_warning` surfacing after junction/restore commits.
- **Launch & CI Infrastructure:** Automated `ci.yml` and tag-triggered `release.yml` workflows (attaching NSIS installer, `smartmontools 7.5` GPLv2+ source archive, and `SHA256SUMS.txt`).

### Changed
- Removed legacy Python v0.4 scaffolding and internal research artifacts into `_archive/` (gitignored).
- Replaced private serial fixtures with synthetic `smart-{0,1,2}.json` demo snapshots.
- Cleaned all `"AI"` promotional badges and legacy third-party naming labels across frontend views and TypeScript helpers.
- Fixed tooltip formatting so unknown metrics render `—` instead of `null`.
