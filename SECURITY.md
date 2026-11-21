# Security Policy

## Scope & Threat Model

**SSD Sentinel** is a local-only Windows desktop disk utility. By design:

- It does **not** expose any network listeners, remote APIs, or telemetry endpoints.
- It does **not** transmit disk serial numbers, SMART metrics, filenames, or crash dumps off the device.
- The **Check for Updates** button in Settings is strictly user-initiated and defaults to `unconfigured` unless the build/environment configures `SSD_SENTINEL_UPDATE_REPO`.
- Mutating disk operations (benchmark file allocation, surface repair, partition shrink/extend, optimization toggles, NTFS tier relocation) execute exclusively after explicit modal UI confirmation and IPC confirmation token validation inside the Rust core (`src-tauri/src/core.rs`).
- All filesystem scans and NTFS forensics run read-only; recovered outputs never write back to the source volume by default unless explicitly overridden by OS destination picker selection.
- Snapshot exports redact device serial numbers when requested (`redact: true`).

## Reporting a Vulnerability

If you discover a security or data-safety vulnerability—especially:
- Unexpected write/destructive behavior during read-only operations (SMART read, physical surface scan, recovery preview/scan, snapshot replay mode),
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

- Keep `smartctl` arguments strictly constructed as fixed argument vectors (`Command::arg`)—never via shell interpolation.
- Never silently downgrade a failing write, hash check, or incomplete SMART read into a synthetic success state.
- Keep WebView2 CSP restrictive (`default-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'`).
