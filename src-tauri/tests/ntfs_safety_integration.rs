use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use ssd_sentinel_core::{
    core::{Config, Core, JobRequest},
    dev_workspace, jobs, model,
    optimize::OptimizeRequest,
    platform, process, tiering,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

fn sha256_bytes(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

fn sha256_file(path: &Path) -> String {
    sha256_bytes(&std::fs::read(path).expect("read file for sha256"))
}

fn is_reparse_point(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn live_test_core(dir: &Path) -> (std::sync::Arc<Core>, model::Disk, model::Disk) {
    let ssd_raw: Value = serde_json::from_str(include_str!("fixtures/smart-1.json")).unwrap();
    let hdd_raw: Value = serde_json::from_str(include_str!("fixtures/smart-0.json")).unwrap();
    let ssd = model::normalise(&ssd_raw);
    let hdd = model::normalise(&hdd_raw);
    let inv = json!({
        "disks": [
            {
                "number": 0,
                "serial": hdd.serial,
                "model": hdd.model,
                "volumes": [{ "letter": "D", "filesystem": "NTFS", "size_bytes": 1_000_000_000_000u64, "free_bytes": 400_000_000_000u64 }]
            },
            {
                "number": 1,
                "serial": ssd.nvme_eui64.first().cloned().or(ssd.serial.clone()),
                "model": ssd.model,
                "volumes": [{ "letter": "C", "filesystem": "NTFS", "size_bytes": 512_000_000_000u64, "free_bytes": 120_000_000_000u64 }]
            }
        ]
    });
    let mut disks = vec![hdd, ssd];
    platform::attach_volumes(&mut disks, &inv);
    let hdd = disks[0].clone();
    let ssd = disks[1].clone();

    let core = Core::new(Config {
        smartctl: "unused.exe".into(),
        data_dir: dir.into(),
        snapshot_dir: None,
    })
    .unwrap();
    {
        let mut data = core.data.lock().unwrap();
        data["stale"] = json!(false);
        data["disks"] = json!(disks);
    }
    (core, hdd, ssd)
}

#[test]
fn read_only_guard_defaults_on_blocks_mutations_and_persists_unlock() {
    assert!(!Core::is_mutating_job(
        "recovery",
        &json!({"mode": "quick"})
    ));
    assert!(!Core::is_mutating_job("recovery", &json!({"mode": "deep"})));
    assert!(!Core::is_mutating_job(
        "surface",
        &json!({"mode": "read_scan"})
    ));
    assert!(!Core::is_mutating_job(
        "surface",
        &json!({"mode": "physical_scan"})
    ));
    assert!(!Core::is_mutating_job(
        "defrag",
        &json!({"mode": "analyze"})
    ));

    assert!(Core::is_mutating_job("benchmark", &json!({"mode": "read"})));
    assert!(Core::is_mutating_job(
        "surface",
        &json!({"mode": "data_refresh"})
    ));
    assert!(Core::is_mutating_job(
        "surface",
        &json!({"mode": "magnetic_refresh"})
    ));
    assert!(Core::is_mutating_job(
        "surface",
        &json!({"mode": "bad_zone_isolate"})
    ));
    assert!(Core::is_mutating_job("defrag", &json!({"mode": "defrag"})));
    assert!(Core::is_mutating_job(
        "autopilot",
        &json!({"profile": "auto"})
    ));
    assert!(Core::is_mutating_job(
        "smart_tier",
        &json!({"mode": "ssd_extent_optimize"})
    ));
    assert!(Core::is_mutating_job("unknown_kind", &json!({})));

    let dir = tempfile::tempdir().unwrap();
    let (core, hdd, ssd) = live_test_core(dir.path());
    // The host locale differs between dev machines and CI, so pin the UI language explicitly.
    core.set_preferences(false, None, Some("tr"), None).unwrap();
    assert_eq!(core.state()["read_only_guard"], true);
    assert!(core.require_write_unlocked("test").is_err());

    let bench_err = core
        .start_job(JobRequest {
            kind: "benchmark".into(),
            disk_id: ssd.id.clone(),
            letter: "C".into(),
            confirmation: format!("benchmark:{}:C", ssd.id),
            options: json!({
                "profile": "standard",
                "mode": "read",
                "size_mib": 256,
                "repeats": 1,
                "seconds": 5,
                "interval": 0,
                "data": "random",
                "row": "all"
            }),
        })
        .unwrap_err();
    assert!(
        bench_err.contains("Salt-okunur koruma etkin"),
        "Unexpected error: {bench_err}"
    );

    let repair_err = core
        .start_job(JobRequest {
            kind: "surface".into(),
            disk_id: hdd.id.clone(),
            letter: "D".into(),
            confirmation: format!("surface:{}:D:magnetic_refresh", hdd.id),
            options: json!({"mode": "magnetic_refresh", "size_mib": 64}),
        })
        .unwrap_err();
    assert!(repair_err.contains("Salt-okunur koruma etkin"));

    let defrag_err = core
        .start_job(JobRequest {
            kind: "defrag".into(),
            disk_id: hdd.id.clone(),
            letter: "D".into(),
            confirmation: format!("defrag:{}:D:defrag", hdd.id),
            options: json!({"mode": "defrag"}),
        })
        .unwrap_err();
    assert!(defrag_err.contains("Salt-okunur koruma etkin"));

    let auto_err = core
        .start_job(JobRequest {
            kind: "autopilot".into(),
            disk_id: ssd.id.clone(),
            letter: "C".into(),
            confirmation: format!("autopilot:{}:C", ssd.id),
            options: json!({"profile": "auto"}),
        })
        .unwrap_err();
    assert!(auto_err.contains("Salt-okunur koruma etkin"));

    let tier_job_err = core
        .start_job(JobRequest {
            kind: "smart_tier".into(),
            disk_id: ssd.id.clone(),
            letter: "C".into(),
            confirmation: format!("smart_tier:{}:C:ssd_extent_optimize", ssd.id),
            options: json!({"mode": "ssd_extent_optimize"}),
        })
        .unwrap_err();
    assert!(tier_job_err.contains("Salt-okunur koruma etkin"));

    let retrim_err = core
        .disk_action(
            &ssd.id,
            "retrim",
            Some(&format!("retrim:{}", ssd.id)),
            Some("C"),
        )
        .unwrap_err();
    assert!(retrim_err.contains("Salt-okunur koruma etkin"));

    let opt_err = core
        .optimize_action(OptimizeRequest {
            action: "ntfs_trim".into(),
            target: "system".into(),
            value: "enable".into(),
            confirmation: "optimize:ntfs_trim:system".into(),
        })
        .unwrap_err();
    assert!(opt_err.contains("Salt-okunur koruma etkin"));

    let reloc_err = core
        .relocate_tier_item(
            &ssd.id,
            "C:\\Users\\demo\\Videos\\sample.mkv",
            "D",
            "to_hdd",
            &format!("tier_relocate:{}:D", ssd.id),
        )
        .unwrap_err();
    assert!(reloc_err.contains("Salt-okunur koruma etkin"));

    let rb_err = core
        .rollback_tier_move("move-1", "tier_rollback:move-1")
        .unwrap_err();
    assert!(rb_err.contains("Salt-okunur koruma etkin"));

    // English UI: the same guard must answer in clean English (no Turkish leakage).
    core.set_preferences(false, None, Some("en"), None).unwrap();
    let en_guard = core.require_write_unlocked("test").unwrap_err();
    assert!(
        en_guard.contains("Read-Only Guard is active"),
        "Unexpected error: {en_guard}"
    );
    let en_bench = core
        .start_job(JobRequest {
            kind: "benchmark".into(),
            disk_id: ssd.id.clone(),
            letter: "C".into(),
            confirmation: format!("benchmark:{}:C", ssd.id),
            options: json!({
                "profile": "standard",
                "mode": "read",
                "size_mib": 256,
                "repeats": 1,
                "seconds": 5,
                "interval": 0,
                "data": "random",
                "row": "all"
            }),
        })
        .unwrap_err();
    for text in [&en_guard, &en_bench] {
        assert!(
            !text.chars().any(|c| "çğıöşüÇĞİÖŞÜ".contains(c)),
            "Turkish leaked into EN error: {text}"
        );
    }
    core.set_preferences(false, None, Some("tr"), None).unwrap();

    // Unlock write mode and verify persistence across restart.
    core.set_preferences(false, None, None, Some(false))
        .unwrap();
    assert_eq!(core.state()["read_only_guard"], false);
    assert!(core.require_write_unlocked("test").is_ok());
    drop(core);

    let reopened = Core::new(Config {
        smartctl: "unused.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: None,
    })
    .unwrap();
    assert_eq!(reopened.state()["read_only_guard"], false);

    reopened
        .set_preferences(false, None, None, Some(true))
        .unwrap();
    assert_eq!(reopened.state()["read_only_guard"], true);
}

#[test]
fn tiering_fault_injection_mid_copy_cancel_and_sha256_roundtrip() {
    let src_root = tempfile::tempdir().unwrap();
    let dst_root = tempfile::tempdir().unwrap();
    let folder = src_root.path().join("MediaBundle");
    let nested = folder.join("Clips").join("2026");
    std::fs::create_dir_all(&nested).unwrap();

    let payload_a: Vec<u8> = (0..(2 * 1024 * 1024 + 317))
        .map(|i| ((i * 31) & 0xff) as u8)
        .collect();
    let payload_b: Vec<u8> = (0..(1024 * 1024 + 911))
        .map(|i| ((i * 17 + 7) & 0xff) as u8)
        .collect();
    let file_a = folder.join("footage.bin");
    let file_b = nested.join("render.bin");
    std::fs::write(&file_a, &payload_a).unwrap();
    std::fs::write(&file_b, &payload_b).unwrap();

    let hash_a = sha256_bytes(&payload_a);
    let hash_b = sha256_bytes(&payload_b);

    // Fault injection: cancel right when the first chunk progress callback arrives.
    let cancel = AtomicBool::new(false);
    let err = tiering::relocate_item_into_dir_with_cancel(
        &folder,
        dst_root.path(),
        "to_hdd",
        true,
        0,
        &cancel,
        &mut |_, _, _, _, _| {
            cancel.store(true, Ordering::SeqCst);
        },
    )
    .unwrap_err();
    assert_eq!(err, process::CANCELLED);
    assert!(folder.is_dir());
    assert!(!is_reparse_point(&folder));
    assert_eq!(sha256_file(&file_a), hash_a);
    assert_eq!(sha256_file(&file_b), hash_b);
    assert!(!dst_root.path().join("MediaBundle").exists());

    // Complete move with NTFS Directory Junction + SHA-256 verification.
    let rec = tiering::relocate_item_into_dir(&folder, dst_root.path(), "to_hdd", true, 0).unwrap();
    assert_eq!(rec["is_dir"], true);
    assert_eq!(rec["junction_created"], true);
    assert!(is_reparse_point(&folder));
    assert_eq!(sha256_file(&file_a), hash_a);
    assert_eq!(sha256_file(&file_b), hash_b);

    let archived = PathBuf::from(rec["target_path"].as_str().unwrap());
    assert!(archived.is_dir());
    assert!(!is_reparse_point(&archived));
    assert_eq!(sha256_file(&archived.join("footage.bin")), hash_a);

    // Rollback must remove only the junction, restore real directory, and match SHA-256 hashes.
    let rb = tiering::rollback_move(&rec).unwrap();
    assert_eq!(rb["rolled_back"], true);
    assert!(!archived.exists());
    assert!(folder.is_dir());
    assert!(!is_reparse_point(&folder));
    assert_eq!(sha256_file(&file_a), hash_a);
    assert_eq!(sha256_file(&file_b), hash_b);
}

#[test]
fn forbidden_system_path_firewall_blocks_all_protected_components() {
    let src_root = tempfile::tempdir().unwrap();
    let dst_root = tempfile::tempdir().unwrap();

    for forbidden_dir in [
        "Windows",
        "Program Files",
        "Program Files (x86)",
        "ProgramData",
        "System Volume Information",
        "$Recycle.Bin",
        "Recovery",
    ] {
        let dir = src_root.path().join(forbidden_dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sentinel = dir.join("protected.dat");
        std::fs::write(&sentinel, b"do-not-move").unwrap();
        assert!(
            tiering::relocate_item_into_dir(&dir, dst_root.path(), "to_hdd", true, 0).is_err(),
            "Forbidden directory {forbidden_dir} should be rejected"
        );
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"do-not-move");
    }

    let appdata = src_root.path().join("Users").join("demo").join("AppData");
    std::fs::create_dir_all(&appdata).unwrap();
    std::fs::write(appdata.join("prefs.db"), b"keep").unwrap();
    assert!(
        tiering::relocate_item_into_dir(&appdata, dst_root.path(), "to_hdd", true, 0).is_err(),
        "Nested AppData directory must be rejected"
    );
    assert_eq!(std::fs::read(appdata.join("prefs.db")).unwrap(), b"keep");

    for sys_file in ["pagefile.sys", "NTUSER.DAT", "hiberfil.sys"] {
        let f = src_root.path().join(sys_file);
        std::fs::write(&f, b"kernel-artifact").unwrap();
        assert!(
            tiering::relocate_item_into_dir(&f, dst_root.path(), "to_hdd", false, 0).is_err(),
            "Protected file {sys_file} must be rejected"
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"kernel-artifact");
    }
}

#[test]
fn readonly_scanners_do_not_mutate_source_files() {
    let root = tempfile::tempdir().unwrap();
    let sub = root.path().join("datasets");
    std::fs::create_dir_all(&sub).unwrap();
    let f1 = sub.join("alpha.bin");
    let f2 = sub.join("beta.bin");
    let d1: Vec<u8> = (0..65_536).map(|i| (i % 199) as u8).collect();
    let d2: Vec<u8> = (0..131_072).map(|i| (i % 233) as u8).collect();
    std::fs::write(&f1, &d1).unwrap();
    std::fs::write(&f2, &d2).unwrap();
    let h1 = sha256_bytes(&d1);
    let h2 = sha256_bytes(&d2);

    let samples = jobs::candidate_samples(root.path(), 4 * 1024 * 1024).unwrap();
    assert!(!samples.is_empty());
    assert_eq!(sha256_file(&f1), h1);
    assert_eq!(sha256_file(&f2), h2);
}

#[test]
#[ignore = "Requires mounted temporary NTFS VHDX volumes; run via tools/vhdx_integration_test.ps1"]
fn vhdx_real_ntfs_cross_volume_junction_and_rollback() {
    let src_letter = std::env::var("SSD_SENTINEL_VHDX_SRC")
        .expect("SSD_SENTINEL_VHDX_SRC env var required (e.g. V)");
    let dst_letter = std::env::var("SSD_SENTINEL_VHDX_DST")
        .expect("SSD_SENTINEL_VHDX_DST env var required (e.g. W)");
    assert_eq!(src_letter.len(), 1);
    assert_eq!(dst_letter.len(), 1);
    assert_ne!(src_letter, dst_letter);

    let folder = PathBuf::from(format!(
        "{src_letter}:\\Users\\ci\\Videos\\SentinelVhdxSuite"
    ));
    let nested = folder.join("nested");
    std::fs::create_dir_all(&nested).unwrap();

    let bytes_a: Vec<u8> = (0..(1536 * 1024))
        .map(|i| ((i * 29 + 3) & 0xff) as u8)
        .collect();
    let bytes_b: Vec<u8> = (0..(768 * 1024))
        .map(|i| ((i * 43 + 11) & 0xff) as u8)
        .collect();
    let path_a = folder.join("capture_a.bin");
    let path_b = nested.join("capture_b.bin");
    std::fs::write(&path_a, &bytes_a).unwrap();
    std::fs::write(&path_b, &bytes_b).unwrap();
    let hash_a = sha256_bytes(&bytes_a);
    let hash_b = sha256_bytes(&bytes_b);

    // 1. Live 10 GiB reserve guard must reject relocating onto a 128 MiB VHDX without modifying source.
    let reserve_err = tiering::relocate_item(&folder, &dst_letter, "to_hdd").unwrap_err();
    assert!(
        reserve_err.contains("yeterli boş alan yok") || reserve_err.contains("GiB"),
        "Expected 10 GiB free-space reserve guard, got: {reserve_err}"
    );
    assert_eq!(sha256_file(&path_a), hash_a);
    assert_eq!(sha256_file(&path_b), hash_b);

    // 2. Real cross-volume NTFS relocation with Win32 FSCTL_SET_REPARSE_POINT junction.
    let dst_parent = PathBuf::from(format!(
        "{dst_letter}:\\SSD-Sentinel-Arsiv\\{src_letter}\\Users\\ci\\Videos"
    ));
    let rec = tiering::relocate_item_into_dir(&folder, &dst_parent, "to_hdd", true, 0).unwrap();
    assert_eq!(rec["is_dir"], true);
    assert_eq!(rec["junction_created"], true);
    assert!(
        is_reparse_point(&folder),
        "Source must be an NTFS Directory Junction"
    );

    // Reading via source drive letter transparently serves bytes from destination VHDX!
    assert_eq!(sha256_file(&path_a), hash_a);
    assert_eq!(sha256_file(&path_b), hash_b);

    let archived = PathBuf::from(rec["target_path"].as_str().unwrap());
    assert!(archived.starts_with(&dst_parent));
    assert!(!is_reparse_point(&archived));
    assert_eq!(sha256_file(&archived.join("capture_a.bin")), hash_a);

    // 3. Cancellation guard on a second folder across real NTFS volumes.
    let folder2 = PathBuf::from(format!("{src_letter}:\\Users\\ci\\Videos\\CancelCase"));
    std::fs::create_dir_all(&folder2).unwrap();
    let p2 = folder2.join("partial_guard.bin");
    std::fs::write(&p2, &bytes_a).unwrap();
    let cancel = AtomicBool::new(false);
    let cancel_err = tiering::relocate_item_into_dir_with_cancel(
        &folder2,
        &dst_parent,
        "to_hdd",
        true,
        0,
        &cancel,
        &mut |_, _, _, _, _| {
            cancel.store(true, Ordering::SeqCst);
        },
    )
    .unwrap_err();
    assert_eq!(cancel_err, process::CANCELLED);
    assert_eq!(sha256_file(&p2), hash_a);
    assert!(!dst_parent.join("CancelCase").exists());

    // 4. Rollback restores original directory on source VHDX and cleans destination VHDX.
    let rb = tiering::rollback_move(&rec).unwrap();
    assert_eq!(rb["rolled_back"], true);
    assert!(!archived.exists());
    assert!(folder.is_dir());
    assert!(!is_reparse_point(&folder));
    assert_eq!(sha256_file(&path_a), hash_a);
    assert_eq!(sha256_file(&path_b), hash_b);
}

#[test]
fn dev_workspace_and_dev_optimizations_respect_read_only_guard_and_to_archive_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (core, _hdd, ssd) = live_test_core(dir.path());
    core.set_preferences(false, None, Some("tr"), None).unwrap();
    assert_eq!(core.state()["read_only_guard"], true);

    // 1. Read-only dev workspace analysis works while guard is active.
    let analysis = core.analyze_dev_workspace(&ssd.id, "C", None).unwrap();
    assert_eq!(analysis["disk_id"], ssd.id);
    assert_eq!(analysis["letter"], "C");
    assert_eq!(analysis["replay"], false);

    // 2. Snapshot replay mode also stays safe and never touches disks.
    let replay_dir = tempfile::tempdir().unwrap();
    let replay_core = Core::new(Config {
        smartctl: "unused.exe".into(),
        data_dir: replay_dir.path().into(),
        snapshot_dir: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    replay_core.refresh_now().unwrap();
    let replay_ssd_id = replay_core.disks().unwrap()[1].id.clone();
    let replay_dev = replay_core
        .analyze_dev_workspace(&replay_ssd_id, "C", None)
        .unwrap();
    assert_eq!(replay_dev["replay"], true);

    // 3. Mutating dev operations fail closed while read_only_guard = true.
    let clean_path = "C:\\Users\\demo\\Projects\\web\\node_modules";
    let clean_err = core
        .clean_dev_artifact(
            &ssd.id,
            clean_path,
            &dev_workspace::clean_token(&ssd.id, clean_path),
        )
        .unwrap_err();
    assert!(
        clean_err.contains("Salt-okunur koruma etkin"),
        "{clean_err}"
    );

    for (action, target, value) in [
        ("long_paths", "system", "enable"),
        ("ntfs_8dot3", "system", "disable"),
        (
            "compact_wsl_vhdx",
            "C:\\Users\\demo\\AppData\\Local\\wsl\\ext4.vhdx",
            "compact",
        ),
    ] {
        let err = core
            .optimize_action(OptimizeRequest {
                action: action.into(),
                target: target.into(),
                value: value.into(),
                confirmation: format!("optimize:{action}:{target}"),
            })
            .unwrap_err();
        assert!(
            err.contains("Salt-okunur koruma etkin"),
            "{action} should be blocked by read_only_guard: {err}"
        );
    }

    let archive_err = core
        .relocate_tier_item(
            &ssd.id,
            "C:\\Users\\demo\\.ollama\\models",
            "D",
            "to_archive",
            &format!("tier_relocate:{}:D", ssd.id),
        )
        .unwrap_err();
    assert!(archive_err.contains("Salt-okunur koruma etkin"));

    // 4. to_archive relocation & SHA-256 junction rollback roundtrip in sandbox.
    let src_root = tempfile::tempdir().unwrap();
    let dst_root = tempfile::tempdir().unwrap();
    let models = src_root.path().join("models");
    std::fs::create_dir_all(&models).unwrap();
    let blob = models.join("weights.bin");
    let data: Vec<u8> = (0..(512 * 1024 + 99)).map(|i| (i % 251) as u8).collect();
    std::fs::write(&blob, &data).unwrap();
    let digest = sha256_bytes(&data);

    let rec =
        tiering::relocate_item_into_dir(&models, dst_root.path(), "to_archive", true, 0).unwrap();
    assert_eq!(rec["direction"], "to_archive");
    assert_eq!(rec["junction_created"], true);
    assert!(is_reparse_point(&models));
    assert_eq!(sha256_file(&blob), digest);

    let rb = tiering::rollback_move(&rec).unwrap();
    assert_eq!(rb["rolled_back"], true);
    assert!(!is_reparse_point(&models));
    assert_eq!(sha256_file(&blob), digest);
}
