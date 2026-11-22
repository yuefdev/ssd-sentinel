# Security Policy

## Scope & Threat Model

**SSD Sentinel** is a local-only Windows desktop disk utility. Formal architecture and data-safety guarantees are documented in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) and [`docs/SAFETY_SPEC.md`](docs/SAFETY_SPEC.md). By design:

- It does **not** expose any network listeners, remote APIs, or telemetry endpoints.
- It does **not** transmit disk serial numbers, SMART metrics, filenames, or crash dumps off the device.
- **Default Read-Only Guard (`read_only_guard`):** Fresh installs start with Read-Only Guard enabled (`true`). Mutating disk operations (benchmarks, surface repair/isolation, partition shrink/extend, optimization toggles, NTFS tier relocation/rollback) are blocked inside `src-tauri/src/core.rs` and `src-tauri/src/optimize.rs` until explicitly unlocked by the user.
- **Dry-Run Manifest & Confirmation Gate:** Every mutating operation requires explicit modal confirmation displaying the target, underlying Win32/CLI calls, write scope, rollback path, and deterministic IPC confirmation token checked in Rust.
- **Strict Read-Only Scans & Forensics:** SMART polling, physical LBA surface scanning (`physical_scan`), NTFS forensic parsing (`$MFT`, `$UsnJrnl`, `$I30`, VSS, carving), and file preview open storage read-only; restored files save only to user-picked external locations.
- **User-Initiated HTTPS Updater:** **Check for updates** in Settings is strictly user-triggered and defaults to the official repository (`yuefdev/ssd-sentinel`; set build/runtime `SSD_SENTINEL_UPDATE_REPO=none` to disable). Downloads allow HTTPS redirects only across official GitHub origins and enforce cryptographic `SHA256SUMS.txt` verification before launch.
- **Redacted Exports:** Snapshot exports redact device serial numbers and WWN/EUI-64 identifiers by default (`redact: true`).

## Supply-Chain & Binary Verification

- Pull requests and pushes run automated `npm audit --omit=dev` and `cargo audit` dependency vulnerability scans.
- Official release installers (`v*` tags) publish `SHA256SUMS.txt` and SLSA GitHub Build Provenance attestations (`actions/attest-build-provenance@v2`). Verify any downloaded installer with:

```powershell
gh attestation verify "SSD-Sentinel_<version>_x64-setup.exe" --owner yuefdev
```

## Reporting a Vulnerability

If you discover a security or data-safety vulnerability—especially:
- Unexpected write/destructive behavior during read-only operations or while Read-Only Guard is active,
- Path traversal or unsafe symlink/junction handling in storage tiering or file restoration,
- Arbitrary command execution or unvalidated IPC parameters bridging WebView2 to Rust,
- Accidental leakage of private machine identifiers in fixture/export pipelines—

please **do not** disclose live exploitation steps publicly before a patch is available. Instead:

1. Open a private GitHub **Security Advisory** (`Security` -> `Advisories` -> `Report a vulnerability`) if hosted on GitHub, **or**
2. Contact the repository maintainer privately with:
   - Affected SSD Sentinel version (`vX.Y.Z`),
   - Reproduction steps (using `--snapshot-dir` fixture mode when possible),
   - Impact assessment (e.g., race condition, NTFS junction edge case, privilege boundary).

## Hardening Commitments

- Keep `smartctl` and Windows `System32` arguments strictly constructed as fixed argument vectors (`Command::arg`)—never via shell interpolation.
- Never silently downgrade a failing write, hash check, or incomplete SMART read into a synthetic success state.
- Keep WebView2 CSP restrictive (`default-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'`).
