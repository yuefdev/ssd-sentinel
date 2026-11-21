# Changelog

All notable changes to **SSD Sentinel** are documented in this file.

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
