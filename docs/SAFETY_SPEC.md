# SSD Sentinel — Safety & Data-Integrity Specification

This document defines the enforceable safety invariants, crash-consistency guarantees, and verification rules governing every disk operation in **SSD Sentinel**.

---

## 1. Core Safety Invariants (S1–S10)

### S1. Default Read-Only Guard (`read_only_guard`)
- Every fresh installation initializes `Core::read_only_guard = true` (persisted in `preferences.json`).
- While locked, **all** mutating IPC paths fail closed inside Rust (`Core::require_write_unlocked`) before any write handle or system modification is attempted:
  - Mutating jobs in `Core::start_job`: `benchmark`, `autopilot`, `smart_tier`, `defrag` (`/O`, `/X`), and mutating `surface` modes (`data_refresh`, `targeted_repair`, `magnetic_refresh`, `bad_zone_isolate`).
  - Mutating maintenance in `Core::disk_action`: `short_test`, `long_test`, `abort_test`, `retrim`.
  - Tier relocations & rollbacks: `Core::relocate_tier_item` (`to_hdd`, `to_ssd`, `to_archive`), `Core::rollback_tier_move`.
  - Developer workspace cleaning: `Core::clean_dev_artifact` (manifest-paired rebuildable artifacts & allowlisted package caches only; AI models never deletable).
  - System/partition tuning: `optimize::execute_unlocked` (including `long_paths`, `ntfs_8dot3`, `compact_wsl_vhdx`; except read-only `query_op_bounds` and user-level `autostart`).
- Strictly read-only diagnostics (`refresh`, `logs`, `recovery` including `dev` mode, `surface` `read_scan` & `physical_scan`, `defrag /A`, `analyze_storage_tiering`, `analyze_dev_workspace`) remain operational while Read-Only Guard is enabled.

### S2. Dry-Run Impact Preview & Deterministic IPC Confirmation Tokens
- Every mutating UI workflow renders an **Execution Manifest (Dry-Run)** box inside `#confirmation` listing the exact target volume/device, underlying Win32/CLI APIs, write budget, rollback mechanism, and deterministic IPC confirmation token (`<kind>:<disk_id>:<letter>[:<mode>]`, `optimize:<action>:<target>`, or `dev_clean:<disk_id>:<sha256_16(path)>`).
- Rust validates `request.confirmation == expected` before reserving `Permit` or touching storage. Unknown JSON keys are rejected via `#[serde(deny_unknown_fields)]`.

### S3. Unique Verified Physical Identity
- Before any live write, Rust `Core::live_disk` + `platform::verify_disk_mapping` require:
  1. `source == "live"` and `stale == false` (Snapshot Replay mode unconditionally refuses all mutating IPC calls).
  2. `stable_identity == true` and `read_incomplete == false` (partial SMART read failures never allow writes).
  3. Unique matching `os_disk_number`, `os_serial` (including NVMe EUI-64 normalization), and `os_model`. Duplicate or ambiguous drive identities abort telemetry recording and block jobs.

### S4. Single-Flight Concurrency & Graceful Shutdown
- `Core::reserve()` issues a single `Permit(Mutex<bool> + Condvar)`. Only one disk job, SMART refresh, optimization, or tier transfer may run at any time.
- `Core::shutdown()` raises `stop` and `job_cancel`, wakes background pollers, waits for active `Permit` holders to clean up temporary files/handles, and joins worker threads before exiting.

### S5. Strictly Read-Only Forensics & Recovery
- `forensics.rs` and `recovery.rs` open volumes and registry/shadow artifacts strictly read-only (`GENERIC_READ` / `File::open`).
- Restored files require an OS destination picker outside the source volume (anti-overwrite guard rejects writing recovered output back onto `source_letter` unless explicitly unresolvable/non-overwriting temp fallback rules block collision).

### S6. Hardware & SSD Wear Protections
- **Classic Defrag Block:** `Core::start_job` refuses `kind == "defrag"` whenever `disk.kind != "HDD"` to protect NAND flash endurance.
- **HDD-Only Isolation Lock:** `bad_zone_isolate` is rejected on SSDs (SSD controllers manage bad block retirement internally).
- **Thermal & Cascade Watchdog (`surface_heal.rs`):** During physical scans/repairs, SMART is polled every 60s; temperature `>= 53.0 °C` pauses for cooldown, and `reallocated_5` delta `>= 8` triggers immediate `AbortCascade`.
- **Write Budget Caps:** `smart_tier` caps extent consolidation at `2 GiB` (`ssd_extent_optimize`) and outer-track placement at `4 GiB` (`hdd_outer_optimize`). `apply_op` and `guard_tail` enforce a minimum `>= 10 GiB` remaining free-space floor.

### S7. Zero Silent Downgrades / Honest Telemetry
- Failed or incomplete SMART reads mark only the affected drive `read_incomplete: true` with `health.percent: null` and surface `.warning-banner` rather than synthesizing `100%` health or hiding errors.
- Unconfigured/unreachable update queries never claim `up_to_date`.

### S8. Protected Path Firewall (25 System Entries)
`tiering.rs` unconditionally blocks analysis or relocation — and `dev_workspace.rs` blocks scanning or cleaning — of any path containing or matching:
`windows`, `program files`, `program files (x86)`, `programdata`, `appdata`, `$recycle.bin`, `system volume information`, `recovery`, `perflogs`, `msocache`, `config.msi`, `ssd-sentinel-arsiv`, `.ssd-sentinel-quarantine`, `$windows.~bt`, `$windows.~ws`, `$sysreset`, `pagefile.sys`, `hiberfil.sys`, `swapfile.sys`, `dumpstack.log`, `dumpstack.log.tmp`, `bootmgr`, `bootnxt`, `ntuser.dat`, `usrclass.dat`, as well as any pre-existing reparse point / symlink.

---

## 2. Crash-Consistency & Power-Loss Matrix

| Operation | Failure Mode (Cancel / Crash / Power Loss) | Source Data Guarantee & Recovery State |
|---|---|---|
| **Benchmark (`jobs.rs`)** | Cancelled or killed mid-run | Only operates on a dedicated temporary file inside target root; `cancel_job` / drop cleanup deletes temp payload if its identity/ownership still matches. User files are untouched. |
| **Full Surface Scan (`physical_scan`)** | Killed or rebooted mid-scan | Strictly read-only `GENERIC_READ`. Progress checkpoint saved every 5 zones in `surface_checkpoints`; next launch offers clean resume or reset. |
| **Pinpoint Repair (`magnetic_refresh`)** | Power loss or cancel during region repair | Step 1 moves affected user files off damaged clusters via NTFS `FSCTL_MOVE_FILE` (atomic NTFS cluster remap) **before** sector rewrite tests run. Source clusters with unplaced system/locked files are never overwritten. |
| **Tier Relocation (`tiering::relocate_item`)** | Interrupted during cross-volume copy | Source remains completely untouched while copying chunk-by-chunk with streaming SHA-256 verification. On cancel or read/hash mismatch, partial destination directory is removed and source is 100% intact. |
| **Directory Junction Swap (`tiering.rs`)** | Interrupted after SHA-256 copy passes, during junction creation | Source folder is renamed to `<name>-<id>.sentinel-moving` sibling on the same NTFS volume before `FSCTL_SET_REPARSE_POINT` creates the junction. If junction creation fails, the sibling is immediately renamed back to the original path and the archive copy is cleaned up. Source is only deleted **after** junction creation succeeds. |
| **Tier Rollback (`tiering::rollback_move`)** | Interrupted during restore to original drive | Restores data into a sibling staging path and verifies byte-for-byte SHA-256 match against target archive **before** removing the NTFS junction and swapping into place. Archive copy is deleted last. |
| **System Optimizations (`optimize.rs`)** | User wishes to revert OS/controller policy | Prior state (`previous_value`) is committed to SQLite `optimizations` table before/upon applying change, enabling exact rollback (`write_cache`, `ntfs_trim`, `pcie_aspm`, `hibernation`, `hdd_apm`). |

---

## 3. Supply-Chain & Binary Verification Guarantees

1. **In-App Updater Allowlist (`desktop.rs`):**
   - Strictly user-initiated (zero background polling).
   - Accepts HTTPS redirects **only** within `github.com`, `api.github.com`, `objects.githubusercontent.com`, `release-assets.githubusercontent.com`, and `github-releases.githubusercontent.com`.
   - Enforces cryptographic verification against release `SHA256SUMS.txt` before `ShellExecuteW`; any hash mismatch or missing checksum deletes the temporary download and aborts.
2. **Build Provenance & Audit:**
   - Every PR runs `npm audit --omit=dev` and `cargo audit` against `Cargo.lock`.
   - Tagged GitHub Releases (`v*`) generate SLSA Build Provenance attestations via `actions/attest-build-provenance@v2`, verifiable locally with GitHub CLI:
     ```powershell
     gh attestation verify "SSD-Sentinel_<version>_x64-setup.exe" --owner yuefdev
     ```
3. **Code Signing Transparency (SmartScreen):**
   - Open-source community builds are currently **unsigned** (no commercial EV Authenticode certificate), so Windows Defender SmartScreen may display an "Unknown Publisher" prompt on first launch. Users should always verify the installer's SHA-256 against `SHA256SUMS.txt` and/or `gh attestation verify`, or build directly from source via CI.

---

## 4. Reproducible Verification Protocol

Every PR and release runs deterministic, non-mocked safety tests alongside mocked UI checks:

1. **Always-On Core Safety Suite (`ntfs_safety_integration.rs`):**
   - Verifies `read_only_guard` defaults `true` and blocks all mutating APIs until unlocked.
   - Injects mid-copy cancellation faults during `tiering::relocate_item_into_dir_with_cancel` and validates SHA-256 integrity across relocate + junction + rollback.
   - Fuzz-checks the 25-entry forbidden path firewall and read-only scanner invariance.
2. **Real Cross-Volume NTFS VHDX Integration (`tools/vhdx_integration_test.ps1`):**
   - Runs on `windows-latest` CI runners (Administrator) by mounting two ephemeral 128 MB NTFS `.vhdx` volumes via `diskpart`, verifying live `10 GiB` reserve rejection, real Win32 `FSCTL_SET_REPARSE_POINT` cross-volume NTFS Directory Junction creation, transparent SHA-256 reads through the junction, mid-copy fault cancellation, and full rollback + cleanup.
3. **Playwright Mock-IPC & Native WebView2 Smoke (`tools/smoke_tauri.mjs`, `tools/smoke_tauri_native.mjs`):**
   - Verifies default `read_only_guard: true`, unlock confirmation, Dry-Run `.dry-run-box` rendering, TR/EN localization parity, and 0 horizontal overflow at 390px across all 8 views.
