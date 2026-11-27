use serde_json::{json, Value};
use ssd_sentinel_core::{
    core::{Config, Core, JobRequest},
    jobs::{self, Row},
    model,
    optimize::OptimizeRequest,
    platform, process,
    surface_triage::{
        decide, diagnose, plan_regions, slow_threshold_ms, Action, LatencyBaseline, Target,
        TargetKind,
    },
    tiering,
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

fn disk() -> model::Disk {
    model::normalise(&serde_json::from_str::<Value>(include_str!("fixtures/smart-1.json")).unwrap())
}

#[test]
fn eui_mapping_requires_unique_model_and_rejects_collision() {
    let d = disk();
    let eui = d.nvme_eui64[0].clone();
    let inventory = json!({"disks":[{"number":1,"serial":format!("{eui}."),"model":d.model,"volumes":[{"letter":"C","filesystem":"NTFS"}]}]});
    let mut disks = vec![d.clone()];
    platform::attach_volumes(&mut disks, &inventory);
    assert_eq!(disks[0].os_disk_number, Some(1));
    let mut disks = vec![d.clone(), d.clone()];
    platform::attach_volumes(&mut disks, &inventory);
    assert!(disks.iter().all(|d| d.os_disk_number.is_none()));
    let mut wrong = inventory.clone();
    wrong["disks"][0]["model"] = json!("different disk");
    let mut disks = vec![d];
    platform::attach_volumes(&mut disks, &wrong);
    assert_eq!(disks[0].os_disk_number, None);
}

#[test]
fn raw_latency_needs_two_valid_non_reset_non_idle_samples() {
    let a = json!({"ticks":"10000","operations":"100","frequency":"1000000"});
    let b = json!({"ticks":"20000","operations":"110","frequency":"1000000"});
    assert_eq!(platform::latency_ms(&a, &b), Some(1.0));
    assert_eq!(platform::latency_ms(&Value::Null, &b), None);
    assert_eq!(platform::latency_ms(&b, &a), None);
    assert_eq!(platform::latency_ms(&b, &b), None);
}

#[test]
fn mutations_require_fresh_unique_windows_identity_even_without_a_volume() {
    let mut first = disk();
    let inventory =
        json!({"disks":[{"number":1,"serial":first.serial,"model":first.model,"volumes":[]}]});
    let mut disks = vec![first.clone()];
    platform::attach_volumes(&mut disks, &inventory);
    first = disks[0].clone();
    assert!(platform::verify_disk_mapping(&disks, &first, &inventory).is_ok());
    let mut collision = first.clone();
    collision.id = "different-id".into();
    collision.serial = Some("different-serial".into());
    let eui = first.nvme_eui64[0].clone();
    let eui_inventory =
        json!({"disks":[{"number":1,"serial":eui,"model":first.model,"volumes":[]}]});
    assert!(
        platform::verify_disk_mapping(&[first.clone(), collision], &first, &eui_inventory).is_err()
    );
    let refs = json!({"filesystem":"ReFS"});
    assert!(platform::check_filesystem(&refs, true).is_err());
    assert!(platform::check_filesystem(&refs, false).is_ok());
}

#[test]
fn duplicate_smart_identity_is_rejected_before_history_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let snapshots = tempfile::tempdir().unwrap();
    let value: Value = serde_json::from_str(include_str!("fixtures/smart-1.json")).unwrap();
    std::fs::write(snapshots.path().join("smart-0.json"), value.to_string()).unwrap();
    let mut later = value.clone();
    later["local_time"]["time_t"] = json!(value["local_time"]["time_t"].as_i64().unwrap() + 60);
    later["nvme_smart_health_information_log"]["percentage_used"] = json!(30);
    std::fs::write(snapshots.path().join("smart-1.json"), later.to_string()).unwrap();
    let core = Core::new(Config {
        smartctl: "unused.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(snapshots.path().into()),
    })
    .unwrap();
    assert!(core.refresh_now().is_err());
    assert!(core.refresh_now().is_err());
    let db = rusqlite::Connection::open(dir.path().join("history.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM samples", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn shutdown_waits_for_admitted_maintenance_scope_and_rejects_new_actions() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "unused.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: None,
    })
    .unwrap();
    let permit = core.reserve().unwrap();
    let released = Arc::new(AtomicBool::new(false));
    let flag = released.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        drop(permit);
        flag.store(true, Ordering::SeqCst);
    });
    let start = Instant::now();
    core.shutdown();
    assert!(start.elapsed() >= Duration::from_millis(150));
    assert!(released.load(Ordering::SeqCst));
    assert!(core.reserve().is_err());
    worker.join().unwrap();
}

#[test]
fn discovery_honours_an_already_cancelled_scan() {
    let root = tempfile::tempdir().unwrap();
    let stop = AtomicBool::new(true);
    assert_eq!(
        jobs::candidate_samples_with_cancel(root.path(), 64 * 1024 * 1024, &stop).unwrap_err(),
        process::CANCELLED
    );
}

#[test]
fn benchmark_contract_rejects_wrong_target_block_direction_and_dtd() {
    let path = Path::new("C:\\owned\\payload.bin");
    let row = Row {
        id: "r".into(),
        name: "r".into(),
        block_kib: 4,
        queue: 1,
        threads: 1,
        random: true,
    };
    let xml = format!("<Results><TimeSpan><TestTimeSeconds>5</TestTimeSeconds><Thread><Target><Path>{}</Path><ReadBytes>8192</ReadBytes><WriteBytes>0</WriteBytes><ReadCount>2</ReadCount><WriteCount>0</WriteCount></Target></Thread><Latency><AverageReadMilliseconds>0.12</AverageReadMilliseconds></Latency></TimeSpan></Results>\nScore: 10", path.display());
    let result = jobs::parse_benchmark(&xml, path, "read", &row).unwrap();
    assert_eq!(result["iops"], 0.4);
    assert_eq!(result["latency_us"], 120.0);
    assert!(jobs::parse_benchmark(&xml, Path::new("C:\\other.bin"), "read", &row).is_err());
    assert!(jobs::parse_benchmark(&xml.replace("8192", "8193"), path, "read", &row).is_err());
    assert!(jobs::parse_benchmark(&xml, path, "write", &row).is_err());
    assert!(jobs::parse_benchmark(&format!("<!DOCTYPE bad>{xml}"), path, "read", &row).is_err());
    assert!(jobs::parse_benchmark(
        &xml.replace("<TestTimeSeconds>5", "<TestTimeSeconds>NaN"),
        path,
        "read",
        &row
    )
    .is_err());
}

#[test]
fn empty_volume_yields_no_fake_blocks_and_sampling_never_reads_past_file() {
    let root = tempfile::tempdir().unwrap();
    assert!(jobs::candidate_samples(root.path(), 64 * 1024 * 1024)
        .unwrap()
        .is_empty());
    std::fs::write(root.path().join("tiny.bin"), [7u8; 300]).unwrap();
    let samples = jobs::candidate_samples(root.path(), 64 * 1024 * 1024).unwrap();
    assert!(!samples.is_empty());
    assert!(samples.iter().all(|s| s.offset + s.bytes as u64 <= 300));
    assert!(samples.iter().map(|s| s.bytes as u64).sum::<u64>() <= 300);
}

#[test]
fn readable_file_must_resolve_within_the_verified_volume() {
    let root = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let path = elsewhere.path().join("foreign.bin");
    std::fs::write(&path, b"private foreign data").unwrap();
    let file = std::fs::File::open(&path).unwrap();
    assert!(platform::file_within(&file, root.path()).is_err());
    let local = root.path().join("local.bin");
    std::fs::write(&local, b"local").unwrap();
    let file = std::fs::File::open(&local).unwrap();
    assert!(platform::file_within(&file, root.path()).is_ok());
}

#[test]
fn malformed_options_are_rejected_at_the_rust_boundary() {
    assert!(jobs::validate_options("surface", &json!({"mode":"read_scan","size_mib":64})).is_ok());
    assert!(jobs::validate_options("surface", &json!({"mode":"erase","size_mib":64})).is_err());
    assert!(
        jobs::validate_options("surface", &json!({"mode":"read_scan","size_mib":true})).is_err()
    );
    assert!(jobs::validate_options(
        "surface",
        &json!({"mode":"read_scan","size_mib":64,"path":"C:\\"})
    )
    .is_err());
}

#[test]
fn auto_budget_is_accepted_and_resolves_to_the_most_thorough_safe_value() {
    assert!(jobs::validate_options(
        "surface",
        &json!({"mode":"physical_scan","size_mib":"auto"})
    )
    .is_ok());
    assert!(jobs::validate_options("recovery", &json!({"mode":"deep","size_mib":"auto"})).is_ok());
    assert!(
        jobs::validate_options("surface", &json!({"mode":"read_scan","size_mib":"max"})).is_err()
    );
    let gib = 1024 * 1024 * 1024u64;
    // Deep recovery carves every free cluster.
    assert_eq!(
        jobs::auto_budget_mib("recovery", "deep", false, Some(500 * gib)),
        0
    );
    // Physical scan saturates the per-zone sample clamp at 256 MiB.
    assert_eq!(
        jobs::auto_budget_mib("surface", "physical_scan", true, None),
        256
    );
    assert_eq!(
        jobs::auto_budget_mib("surface", "read_scan", false, Some(gib)),
        1024
    );
    // Write-verify spares SSD NAND, uses more on HDD.
    assert_eq!(
        jobs::auto_budget_mib("surface", "data_refresh", false, Some(500 * gib)),
        256
    );
    assert_eq!(
        jobs::auto_budget_mib("surface", "data_refresh", true, Some(500 * gib)),
        1024
    );
    // Write modes never take more than a tenth of free space (floor 64 MiB).
    assert_eq!(
        jobs::auto_budget_mib("surface", "data_refresh", true, Some(2 * gib)),
        204
    );
    assert_eq!(
        jobs::auto_budget_mib("surface", "magnetic_refresh", true, Some(gib / 4)),
        64
    );
}

#[test]
fn replay_disables_every_real_disk_job() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let state = core.state();
    assert_eq!(state["source"], "replay");
    assert_eq!(state["disks"].as_array().unwrap().len(), 3);
    let id = state["disks"][0]["id"].as_str().unwrap();
    assert!(core
        .start_job(JobRequest {
            kind: "surface".into(),
            disk_id: id.into(),
            letter: "C".into(),
            confirmation: format!("surface:{id}:C:read_scan"),
            options: json!({"mode":"read_scan","size_mib":64})
        })
        .is_err());
}

#[test]
fn timeout_and_cancellation_reap_the_owned_child() {
    #[cfg(windows)]
    let (exe, args) = (
        Path::new("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
        vec![
            "-NoProfile".into(),
            "-Command".into(),
            "Start-Sleep -Seconds 20".into(),
        ],
    );
    #[cfg(not(windows))]
    let (exe, args) = (Path::new("/bin/sleep"), vec!["20".into()]);
    let cancel = Arc::new(AtomicBool::new(false));
    let start = Instant::now();
    assert!(process::run(exe, &args, Duration::from_millis(150), &cancel).is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
    let signal = cancel.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        signal.store(true, Ordering::SeqCst);
    });
    assert!(process::run(exe, &args, Duration::from_secs(20), &cancel).is_err());
    worker.join().unwrap();
}

#[test]
fn pcie_bus_link_attaches_only_to_nvme_and_optimization_metadata_attaches() {
    let hdd_val: Value = serde_json::from_str(include_str!("fixtures/smart-0.json")).unwrap();
    let hdd = model::normalise(&hdd_val);
    let nvme = disk();

    let inventory = json!({
        "disks": [
            {
                "number": 0,
                "serial": hdd.serial,
                "model": hdd.model,
                "bus_type": 11,
                "pcie": { "current_gen": 3, "max_gen": 4, "current_width": 1, "max_width": 4 },
                "write_cache": { "enabled": true, "changeable": true },
                "layout": { "allocated_bytes": 1_000_000_000_000u64, "misaligned_partitions": 0 },
                "volumes": [{ "letter": "D", "filesystem": "NTFS" }]
            },
            {
                "number": 1,
                "serial": nvme.serial,
                "model": nvme.model,
                "bus_type": 17,
                "pcie": { "current_gen": 3, "max_gen": 4, "current_width": 2, "max_width": 4 },
                "write_cache": { "enabled": true, "changeable": false },
                "layout": { "allocated_bytes": 460_000_000_000u64, "misaligned_partitions": 1 },
                "volumes": [{ "letter": "C", "filesystem": "NTFS" }]
            }
        ]
    });
    let mut disks = vec![hdd, nvme];
    platform::attach_volumes(&mut disks, &inventory);
    assert_eq!(
        disks[0].bus_link["kind"], "SATA",
        "ATA/SATA HDD must keep SATA link and must not inherit PCIe link"
    );
    assert_eq!(disks[0].optimization["write_cache"]["enabled"], true);
    assert_eq!(disks[0].optimization["aligned_4k"], true);
    assert_eq!(disks[1].bus_link["kind"], "PCIe");
    assert_eq!(disks[1].bus_link["bottleneck"], true);
    assert_eq!(disks[1].optimization["aligned_4k"], false);
    assert!(disks[1]
        .alerts
        .iter()
        .any(|a| a.code == "pcie-link-bottleneck"));
    assert!(disks[1].alerts.iter().any(|a| a.code == "alignment-4k"));
}

#[test]
fn replay_rejects_every_optimize_action_and_validates_defrag_options() {
    assert!(jobs::validate_options("defrag", &json!({"mode":"analyze"})).is_ok());
    assert!(jobs::validate_options("defrag", &json!({"mode":"defrag"})).is_ok());
    assert!(jobs::validate_options("defrag", &json!({"mode":"consolidate"})).is_ok());
    assert!(jobs::validate_options("defrag", &json!({"mode":"wipe"})).is_err());

    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let nvme_id = core.state()["disks"][1]["id"].as_str().unwrap().to_owned();
    assert!(core
        .optimize_action(OptimizeRequest {
            action: "ntfs_trim".into(),
            target: "system".into(),
            value: "enable".into(),
            confirmation: "optimize:ntfs_trim:system".into(),
        })
        .is_err());
    assert!(core
        .start_job(JobRequest {
            kind: "defrag".into(),
            disk_id: nvme_id.clone(),
            letter: "C".into(),
            confirmation: format!("defrag:{nvme_id}:C:analyze"),
            options: json!({"mode":"analyze"}),
        })
        .is_err());
}

#[cfg(windows)]
#[test]
fn live_win32_inventory_and_performance_are_read_only_and_finite() {
    let stop = AtomicBool::new(false);
    let inv = platform::inventory(&stop).unwrap();
    let inv_disks = inv["disks"].as_array().unwrap();
    let mut seen_numbers = std::collections::HashSet::new();
    for d in inv_disks {
        assert!(d["unique_ids"].is_array());
        assert!(seen_numbers.insert(d["number"].as_u64().unwrap()));
    }
    assert!(inv["system_tuning"].is_object());
    let mut prev = Value::Null;
    let first = platform::performance(&mut prev, &stop).unwrap();
    assert!(first["disks"].is_array());
    std::thread::sleep(Duration::from_millis(250));
    let second = platform::performance(&mut prev, &stop).unwrap();
    for d in second["disks"].as_array().unwrap() {
        assert!(d["read_bps"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= 0.0));
        assert!(d["write_bps"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= 0.0));
    }
}

#[test]
fn surface_recovery_modes_validate_bounds_and_reject_replay_and_invalid_targets() {
    for mode in ["physical_scan", "magnetic_refresh", "bad_zone_isolate"] {
        assert!(
            jobs::validate_options("surface", &json!({"mode": mode, "size_mib": 1024})).is_ok(),
            "Expected {mode} at 1024 MiB to pass validation"
        );
    }
    assert!(
        jobs::validate_options("surface", &json!({"mode": "format_raw", "size_mib": 64})).is_err()
    );

    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let hdd_id = core.state()["disks"][0]["id"].as_str().unwrap().to_owned();
    let nvme_id = core.state()["disks"][1]["id"].as_str().unwrap().to_owned();

    for mode in ["physical_scan", "magnetic_refresh", "bad_zone_isolate"] {
        assert!(core
            .start_job(JobRequest {
                kind: "surface".into(),
                disk_id: hdd_id.clone(),
                letter: "D".into(),
                confirmation: format!("surface:{hdd_id}:D:{mode}"),
                options: json!({"mode": mode, "size_mib": 256}),
            })
            .is_err());
    }
    for action in ["guard_tail", "expand_tail", "release_quarantine"] {
        assert!(core
            .optimize_action(OptimizeRequest {
                action: action.into(),
                target: format!("{nvme_id}:C"),
                value: if action == "guard_tail" {
                    "5"
                } else {
                    "release"
                }
                .into(),
                confirmation: format!("optimize:{action}:{nvme_id}:C"),
            })
            .is_err());
    }
}

#[test]
fn autopilot_and_recovery_validate_options_and_reject_replay() {
    assert!(jobs::validate_options("autopilot", &json!({"profile":"auto"})).is_ok());
    assert!(jobs::validate_options("autopilot", &json!({"profile":"bad"})).is_err());
    for mode in ["quick", "deep", "apps_only"] {
        for size in [256, 16384, 0] {
            assert!(
                jobs::validate_options("recovery", &json!({"mode": mode, "size_mib": size}))
                    .is_ok()
            );
        }
    }
    assert!(jobs::validate_options("recovery", &json!({"mode":"wipe","size_mib":256})).is_err());

    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let hdd_id = core.state()["disks"][0]["id"].as_str().unwrap().to_owned();
    assert!(core
        .start_job(JobRequest {
            kind: "autopilot".into(),
            disk_id: hdd_id.clone(),
            letter: "D".into(),
            confirmation: format!("autopilot:{hdd_id}:D"),
            options: json!({"profile":"auto"}),
        })
        .is_err());
    assert!(core
        .start_job(JobRequest {
            kind: "recovery".into(),
            disk_id: hdd_id.clone(),
            letter: "D".into(),
            confirmation: format!("recovery:{hdd_id}:D:quick"),
            options: json!({"mode":"quick","size_mib":256}),
        })
        .is_err());
}

#[test]
fn surface_triage_diagnoses_plans_and_validates_pinpoint_auto_budgets() {
    for mode in ["physical_scan", "magnetic_refresh", "bad_zone_isolate"] {
        assert!(
            jobs::validate_options("surface", &json!({"mode": mode, "size_mib": "auto"})).is_ok(),
            "Expected auto budget for {mode} to pass validation"
        );
    }

    let mut ssd_base = LatencyBaseline::new();
    for _ in 0..300 {
        ssd_base.push(0.5);
    }
    let ssd_thr = slow_threshold_ms(&ssd_base, false);
    assert!((8.0..=12.0).contains(&ssd_thr), "ssd_thr={ssd_thr}");

    let mut hdd_base = LatencyBaseline::new();
    for _ in 0..300 {
        hdd_base.push(8.0);
    }
    let hdd_thr = slow_threshold_ms(&hdd_base, true);
    assert!((50.0..=150.0).contains(&hdd_thr), "hdd_thr={hdd_thr}");

    let mib = 1024 * 1024u64;
    let hdd_targets = vec![
        Target {
            offset: 200 * mib,
            len: 4096,
            kind: TargetKind::WeakSector,
            latency_ms: Some(115.0),
        },
        Target {
            offset: 900 * mib,
            len: 4096,
            kind: TargetKind::Unreadable,
            latency_ms: None,
        },
    ];
    let (regions, outside) = plan_regions(&hdd_targets, 100 * mib, 1000 * mib, 4096);
    assert_eq!(outside, 0);
    assert_eq!(regions.len(), 2);
    assert_eq!(decide(&regions[0], true, false), Action::Heal);
    assert_eq!(decide(&regions[0], true, true), Action::Isolate);
    assert_eq!(decide(&regions[0], false, false), Action::Relocate);

    let hdd_diag = diagnose(&hdd_targets, true, 1, false);
    assert_eq!(hdd_diag["verdict"], "repairable");
    assert_eq!(hdd_diag["recommended_mode"], "magnetic_refresh");

    let ssd_targets = vec![Target {
        offset: 120 * mib,
        len: mib,
        kind: TargetKind::SlowArea,
        latency_ms: Some(45.0),
    }];
    let ssd_diag = diagnose(&ssd_targets, false, 0, false);
    assert_eq!(ssd_diag["verdict"], "repairable");
    assert_eq!(ssd_diag["recommended_mode"], "magnetic_refresh");
}

#[test]
fn v17_surface_checkpoint_validation_summary_and_clear_roundtrip() {
    assert!(jobs::validate_options(
        "surface",
        &json!({"mode": "physical_scan", "size_mib": "auto", "resume": true})
    )
    .is_ok());
    assert!(jobs::validate_options(
        "surface",
        &json!({"mode": "physical_scan", "size_mib": "auto", "resume": false})
    )
    .is_ok());
    assert!(jobs::validate_options(
        "surface",
        &json!({"mode": "physical_scan", "size_mib": "auto", "resume": "yes"})
    )
    .is_err());
    assert!(jobs::validate_options(
        "surface",
        &json!({"mode": "physical_scan", "size_mib": "auto", "unknown": 1})
    )
    .is_err());

    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let disk_id = core.state()["disks"][0]["id"].as_str().unwrap().to_owned();
    assert!(core.surface_checkpoint(&disk_id).unwrap().is_none());

    {
        let mut store = core.store.lock().unwrap();
        store
            .save_surface_checkpoint(
                &disk_id,
                &json!({
                    "disk_id": disk_id,
                    "disk_bytes": 1_000_000_000_000u64,
                    "letter": "D",
                    "next_zone": 60,
                    "total_zones": 200,
                    "done_bytes": 300_000_000_000u64,
                    "elapsed_seconds": 900.0,
                    "targets": [
                        {"offset": 4096, "len": 4096, "kind": "weak_sector", "latency_ms": 120.0}
                    ],
                    "slow_chunks_total": 3,
                    "error_chunks": 1
                }),
            )
            .unwrap();
    }

    let summary = core.surface_checkpoint(&disk_id).unwrap().unwrap();
    assert_eq!(summary["disk_id"], disk_id);
    assert_eq!(summary["next_zone"], 60);
    assert_eq!(summary["total_zones"], 200);
    assert_eq!(summary["progress_percent"], 30);
    assert_eq!(summary["targets_count"], 1);
    assert_eq!(summary["slow_chunks"], 3);
    assert_eq!(summary["error_chunks"], 1);

    core.clear_surface_checkpoint(&disk_id).unwrap();
    assert!(core.surface_checkpoint(&disk_id).unwrap().is_none());
}

#[test]
fn v17_health_forecast_and_nvme_interface_normalization() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let disks = core.disks().unwrap();
    assert_eq!(disks[1].interface.as_deref(), Some("NVMe 1.3"));
    assert_eq!(disks[2].interface.as_deref(), Some("NVMe 1.4"));

    // Single replay snapshot must never fabricate a forecast.
    let single = core.health_forecast(&disks[1].id).unwrap();
    assert_eq!(single["ready"], false);
    assert!(single["daily_written_gb"].is_null());
    assert!(single["estimated_remaining_years"].is_null());
    assert!(core.health_forecast("non-existent-disk").is_err());

    // Two distinct timestamps 24h apart produce a deterministic ready forecast.
    let history = vec![
        json!({
            "time": "2026-11-17T10:00:00Z",
            "health_percent": 74,
            "temperature_c": 38.0,
            "written_bytes": "50000000000000",
            "counters": {}
        }),
        json!({
            "time": "2026-11-18T10:00:00Z",
            "health_percent": 73,
            "temperature_c": 40.0,
            "written_bytes": "50025000000000",
            "counters": {}
        }),
    ];
    let ready = model::compute_health_forecast(&disks[1], &history);
    assert_eq!(ready["ready"], true);
    assert!(ready["daily_written_gb"]
        .as_f64()
        .is_some_and(|v| v > 20.0 && v < 30.0));
    assert!(ready["estimated_remaining_years"]
        .as_f64()
        .is_some_and(|v| v.is_finite() && v > 0.0));
    assert_eq!(ready["temp_avg_7d"], 39.0);
    assert!(ready["summary_tr"].as_str().is_some_and(|s| !s.is_empty()));
}

#[test]
fn v17_recovery_preview_media_repair_and_batch_restore_guards() {
    let sample_dir = tempfile::tempdir().unwrap();
    let txt_path = sample_dir.path().join("$R001.txt");
    std::fs::write(&txt_path, "Merhaba SSD Sentinel v1.7.0").unwrap();

    // Build a truncated JPEG (missing FF D9 EOI) that media_repair fixes.
    let mut trunc_jpg = vec![0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00];
    trunc_jpg.extend_from_slice(&[16u8; 64]);
    trunc_jpg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x20, 0x00, 0x20]);
    trunc_jpg.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
    trunc_jpg.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, 0x00, 0x01]);
    trunc_jpg.extend_from_slice(&[0u8; 16]);
    trunc_jpg.extend_from_slice(&[
        0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x45, 0x67, 0x89, 0xAB, 0xCD,
        0x00, 0x00,
    ]);
    let jpg_path = sample_dir.path().join("$R002.jpg");
    std::fs::write(&jpg_path, &trunc_jpg).unwrap();

    let bad_jpg_path = sample_dir.path().join("$R003.jpg");
    std::fs::write(&bad_jpg_path, vec![0x55u8; 256]).unwrap();

    let dest_dir = tempfile::tempdir().unwrap();
    let dest_drive = dest_dir
        .path()
        .to_string_lossy()
        .chars()
        .next()
        .unwrap_or('C')
        .to_ascii_uppercase()
        .to_string();

    let job_val = json!({
        "id": "rec-job-v17",
        "kind": "recovery",
        "disk_id": "disk-test",
        "letter": dest_drive,
        "status": "completed",
        "recovery_items": [
            {
                "id": "txt-1",
                "name": "notlar.txt",
                "extension": "txt",
                "category": "document",
                "size_bytes": 27,
                "source": "recycle_bin",
                "source_letter": dest_drive,
                "trim_zeroed": false,
                "locator": { "r_path": txt_path.to_string_lossy() }
            },
            {
                "id": "jpg-trunc",
                "name": "tatil.jpg",
                "extension": "jpg",
                "category": "photo",
                "size_bytes": trunc_jpg.len(),
                "source": "recycle_bin",
                "source_letter": dest_drive,
                "trim_zeroed": false,
                "locator": { "r_path": jpg_path.to_string_lossy() }
            },
            {
                "id": "jpg-bad",
                "name": "bozuk.jpg",
                "extension": "jpg",
                "category": "photo",
                "size_bytes": 256,
                "source": "recycle_bin",
                "source_letter": dest_drive,
                "trim_zeroed": false,
                "locator": { "r_path": bad_jpg_path.to_string_lossy() }
            },
            {
                "id": "mft-same-drive",
                "name": "gizli.txt",
                "extension": "txt",
                "category": "document",
                "size_bytes": 4,
                "source": "mft_resident",
                "source_letter": dest_drive,
                "trim_zeroed": false,
                "locator": { "resident_hex": "74657374" }
            }
        ]
    });

    // 1. Replay mode allows read-only preview (including auto-repair) but refuses batch restore.
    let replay_data = tempfile::tempdir().unwrap();
    let replay_core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: replay_data.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    replay_core.refresh_now().unwrap();
    replay_core
        .store
        .lock()
        .unwrap()
        .save_job("recovery", &job_val)
        .unwrap();

    let txt_prev = replay_core
        .preview_recovered_item("rec-job-v17", "txt-1")
        .unwrap();
    assert_eq!(txt_prev["kind"], "text");
    assert_eq!(txt_prev["text"], "Merhaba SSD Sentinel v1.7.0");

    let img_prev = replay_core
        .preview_recovered_item("rec-job-v17", "jpg-trunc")
        .unwrap();
    assert_eq!(img_prev["kind"], "image");
    assert_eq!(img_prev["repaired"], true);
    assert_eq!(img_prev["media_status"], "repaired");

    let bad_prev = replay_core
        .preview_recovered_item("rec-job-v17", "jpg-bad")
        .unwrap();
    assert_eq!(bad_prev["kind"], "unavailable");

    assert!(replay_core
        .restore_recovered_batch_to("rec-job-v17", &["txt-1".into()], dest_dir.path())
        .is_err());

    // 2. Live Core enforces non-empty selection, anti-overwrite on MFT items, and repairs media on batch restore.
    let live_data = tempfile::tempdir().unwrap();
    let live_core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: live_data.path().into(),
        snapshot_dir: None,
    })
    .unwrap();
    live_core
        .store
        .lock()
        .unwrap()
        .save_job("recovery", &job_val)
        .unwrap();

    assert!(live_core
        .restore_recovered_batch_to("rec-job-v17", &[], dest_dir.path())
        .is_err());
    assert!(live_core
        .restore_recovered_batch_to("rec-job-v17", &["mft-same-drive".into()], dest_dir.path())
        .is_err());

    let batch = live_core
        .restore_recovered_batch_to(
            "rec-job-v17",
            &["txt-1".into(), "jpg-trunc".into()],
            dest_dir.path(),
        )
        .unwrap();
    assert_eq!(batch["saved"], true);
    assert_eq!(batch["restored"], 2);
    assert_eq!(batch["failed"], 0);
    assert_eq!(batch["repaired"], true);
    assert_eq!(batch["repaired_count"], 1);
    let repaired_jpg = std::fs::read(dest_dir.path().join("tatil.jpg")).unwrap();
    assert!(repaired_jpg.ends_with(&[0xFF, 0xD9]));
}

#[test]
fn v18_smart_tier_validation_replay_guards_and_scoring_contract() {
    // 1. smart_tier option validation accepts only ssd_extent_optimize and hdd_outer_optimize.
    assert!(jobs::validate_options("smart_tier", &json!({"mode": "ssd_extent_optimize"})).is_ok());
    assert!(jobs::validate_options("smart_tier", &json!({"mode": "hdd_outer_optimize"})).is_ok());
    assert!(jobs::validate_options("smart_tier", &json!({"mode": "full_defrag"})).is_err());
    assert!(jobs::validate_options("smart_tier", &json!({})).is_err());
    assert!(jobs::validate_options(
        "smart_tier",
        &json!({"mode": "ssd_extent_optimize", "extra": true})
    )
    .is_err());

    // 2. Pure AI tier scoring: rejects OS/system paths and scores cold SSD vs hot HDD items accurately.
    let mib = 1024 * 1024u64;
    for forbidden in [
        "C:\\Windows\\System32\\ntoskrnl.exe",
        "C:\\Program Files\\App\\data.bin",
        "C:\\ProgramData\\Cache\\big.iso",
        "C:\\Users\\Demo\\AppData\\Local\\Temp\\x.zip",
        "C:\\pagefile.sys",
        "C:\\Users\\Demo\\..\\Windows\\foo.mp4",
        "D:\\SSD-Sentinel-Arsiv\\C\\Downloads\\old.mkv",
    ] {
        assert!(
            tiering::score_tier_candidate(Path::new(forbidden), false, 500 * mib, 120, true)
                .is_none(),
            "Forbidden path must return None: {forbidden}"
        );
    }

    let cold_video = tiering::score_tier_candidate(
        Path::new("C:\\Users\\Demo\\Videos\\tatil_4k.mkv"),
        false,
        4 * 1024 * mib,
        120,
        true,
    )
    .expect("large old video on SSD must score as cold candidate");
    assert!(
        cold_video.cold_score >= 70,
        "cold_score={}",
        cold_video.cold_score
    );
    assert_eq!(cold_video.recommended_direction, "to_hdd");

    let hot_game = tiering::score_tier_candidate(
        Path::new("D:\\Games\\Cyberpunk2077"),
        true,
        8 * 1024 * mib,
        2,
        false,
    )
    .expect("recently touched game folder on HDD must score as hot candidate");
    assert!(hot_game.hot_score >= 70, "hot_score={}", hot_game.hot_score);
    assert_eq!(hot_game.recommended_direction, "to_ssd");

    // 3. Replay mode refuses smart_tier, relocate_tier_item, and rollback_tier_move,
    //    while allowing read-only analyze_storage_tiering (replay: true) and list_tier_moves.
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let state = core.state();
    let hdd_id = state["disks"][0]["id"].as_str().unwrap().to_owned();
    let nvme_id = state["disks"][1]["id"].as_str().unwrap().to_owned();

    // Seed a stored move record to verify list_tier_moves & analyze_storage_tiering return it cleanly.
    {
        let mut store = core.store.lock().unwrap();
        store
            .record_tier_move(&json!({
                "id": "move-replay-1",
                "source_path": "C:\\Users\\Demo\\Videos\\Arsiv",
                "target_path": "D:\\SSD-Sentinel-Arsiv\\C\\Users\\Demo\\Videos\\Arsiv",
                "source_letter": "C",
                "target_letter": "D",
                "is_dir": true,
                "junction_created": true,
                "size_bytes": 2_147_483_648u64,
                "file_count": 12,
                "direction": "to_hdd",
                "moved_at": "2026-11-21T10:00:00Z"
            }))
            .unwrap();
    }

    let moves = core.list_tier_moves().unwrap();
    assert_eq!(moves.as_array().unwrap().len(), 1);
    assert_eq!(moves[0]["id"], "move-replay-1");
    assert_eq!(core.state()["tier_moves"].as_array().unwrap().len(), 1);

    let analysis = core.analyze_storage_tiering(&nvme_id, "C").unwrap();
    assert_eq!(analysis["replay"], true);
    assert_eq!(analysis["disk_id"], nvme_id);
    assert_eq!(analysis["is_ssd"], true);
    assert!(analysis["candidates"].as_array().unwrap().is_empty());
    assert_eq!(analysis["active_moves"].as_array().unwrap().len(), 1);

    // smart_tier rejected in replay (both SSD and HDD modes)
    assert!(core
        .start_job(JobRequest {
            kind: "smart_tier".into(),
            disk_id: nvme_id.clone(),
            letter: "C".into(),
            confirmation: format!("smart_tier:{nvme_id}:C:ssd_extent_optimize"),
            options: json!({"mode": "ssd_extent_optimize"}),
        })
        .is_err());
    assert!(core
        .start_job(JobRequest {
            kind: "smart_tier".into(),
            disk_id: hdd_id.clone(),
            letter: "D".into(),
            confirmation: format!("smart_tier:{hdd_id}:D:hdd_outer_optimize"),
            options: json!({"mode": "hdd_outer_optimize"}),
        })
        .is_err());

    // relocate_tier_item rejected with bad confirmation AND rejected in replay with valid confirmation
    assert!(core
        .relocate_tier_item(
            &nvme_id,
            "C:\\Users\\Demo\\Videos\\clip.mkv",
            "D",
            "to_hdd",
            "wrong-confirmation",
        )
        .is_err());
    assert!(core
        .relocate_tier_item(
            &nvme_id,
            "C:\\Users\\Demo\\Videos\\clip.mkv",
            "D",
            "to_hdd",
            &format!("tier_relocate:{nvme_id}:D"),
        )
        .is_err());

    // rollback_tier_move rejected with bad confirmation AND rejected in replay with valid confirmation
    assert!(core
        .rollback_tier_move("move-replay-1", "wrong-confirmation")
        .is_err());
    assert!(core
        .rollback_tier_move("move-replay-1", "tier_rollback:move-replay-1")
        .is_err());

    // 4. Mode/disk-kind mismatch guard rejects ssd_extent_optimize on HDD and hdd_outer_optimize on SSD.
    let live_dir = tempfile::tempdir().unwrap();
    let live_core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: live_dir.path().into(),
        snapshot_dir: None,
    })
    .unwrap();
    let mut live_disks = core.disks().unwrap();
    live_disks[0].os_disk_number = Some(0);
    live_disks[0].os_serial = live_disks[0].serial.clone();
    live_disks[0].os_model = Some(live_disks[0].model.clone());
    live_disks[0].volumes = vec![json!({"letter": "D", "filesystem": "NTFS"})];
    live_disks[1].os_disk_number = Some(1);
    live_disks[1].os_serial = live_disks[1].serial.clone();
    live_disks[1].os_model = Some(live_disks[1].model.clone());
    live_disks[1].volumes = vec![json!({"letter": "C", "filesystem": "NTFS"})];
    {
        let mut data = live_core.data.lock().unwrap();
        data["stale"] = json!(false);
        data["disks"] = json!(live_disks);
    }
    assert_eq!(live_core.state()["read_only_guard"], true);
    live_core
        .set_preferences(false, None, None, Some(false))
        .unwrap();
    let err_ssd_on_hdd = live_core
        .start_job(JobRequest {
            kind: "smart_tier".into(),
            disk_id: hdd_id.clone(),
            letter: "D".into(),
            confirmation: format!("smart_tier:{hdd_id}:D:ssd_extent_optimize"),
            options: json!({"mode": "ssd_extent_optimize"}),
        })
        .unwrap_err();
    assert!(
        err_ssd_on_hdd.contains("SSD"),
        "Expected SSD-only guard message, got: {err_ssd_on_hdd}"
    );
    let err_hdd_on_ssd = live_core
        .start_job(JobRequest {
            kind: "smart_tier".into(),
            disk_id: nvme_id.clone(),
            letter: "C".into(),
            confirmation: format!("smart_tier:{nvme_id}:C:hdd_outer_optimize"),
            options: json!({"mode": "hdd_outer_optimize"}),
        })
        .unwrap_err();
    assert!(
        err_hdd_on_ssd.contains("HDD"),
        "Expected HDD-only guard message, got: {err_hdd_on_ssd}"
    );
}

#[test]
fn partial_disk_smart_failure_isolates_failing_disk_and_keeps_healthy_disks_live() {
    let dir = tempfile::tempdir().unwrap();
    let snaps = tempfile::tempdir().unwrap();

    let mut d0: Value = serde_json::from_str(include_str!("fixtures/smart-0.json")).unwrap();
    d0["smartctl"]["exit_status"] = json!(4);
    let d1: Value = serde_json::from_str(include_str!("fixtures/smart-1.json")).unwrap();
    let d2: Value = serde_json::from_str(include_str!("fixtures/smart-2.json")).unwrap();
    std::fs::write(snaps.path().join("smart-0.json"), d0.to_string()).unwrap();
    std::fs::write(snaps.path().join("smart-1.json"), d1.to_string()).unwrap();
    std::fs::write(snaps.path().join("smart-2.json"), d2.to_string()).unwrap();

    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: Some(snaps.path().into()),
    })
    .unwrap();
    core.refresh_now().unwrap();

    let state = core.state();
    assert_eq!(state["stale"], false);
    assert_eq!(state["partial_warnings"].as_array().unwrap().len(), 1);
    assert!(state["error"].is_null());

    let disks = core.disks().unwrap();
    assert_eq!(disks.len(), 3);
    assert!(disks[0].read_incomplete);
    assert!(disks[0].read_warning.is_some());
    assert!(!disks[1].read_incomplete);
    assert!(disks[1].read_warning.is_none());
    assert!(!disks[2].read_incomplete);

    assert_eq!(
        core.history(&disks[0].id)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        core.history(&disks[1].id)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        core.history(&disks[2].id)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let live_dir = tempfile::tempdir().unwrap();
    let live_core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: live_dir.path().into(),
        snapshot_dir: None,
    })
    .unwrap();
    {
        let mut data = live_core.data.lock().unwrap();
        data["stale"] = json!(false);
        data["disks"] = json!(disks);
    }
    assert!(live_core.live_disk(&disks[0].id).is_err());
    assert!(live_core.live_disk(&disks[1].id).is_ok());
}

#[test]
fn v19_preferences_persist_language_and_update_check_handles_unconfigured() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: None,
    })
    .unwrap();
    assert_eq!(
        core.state()["language"],
        ssd_sentinel_core::platform::detect_system_language()
    );

    core.set_preferences(true, Some("strict"), Some("en"), None)
        .unwrap();
    let state = core.state();
    assert_eq!(state["language"], "en");
    assert_eq!(state["minimise_to_tray"], true);
    assert_eq!(state["health_mode"], "strict");
    assert_eq!(state["read_only_guard"], true);
    assert_eq!(state["auto_check_updates"], true);
    core.set_auto_check_updates(false).unwrap();
    assert_eq!(core.state()["auto_check_updates"], false);
    drop(core);

    let reopened = Core::new(Config {
        smartctl: "missing.exe".into(),
        data_dir: dir.path().into(),
        snapshot_dir: None,
    })
    .unwrap();
    let reopened_state = reopened.state();
    assert_eq!(reopened_state["language"], "en");
    assert_eq!(reopened_state["minimise_to_tray"], true);
    assert_eq!(reopened_state["health_mode"], "strict");
    assert_eq!(reopened_state["read_only_guard"], true);
    assert_eq!(reopened_state["auto_check_updates"], false);

    reopened.set_auto_check_updates(true).unwrap();
    assert_eq!(reopened.state()["auto_check_updates"], true);

    reopened
        .set_preferences(false, Some("default"), Some("tr"), Some(false))
        .unwrap();
    assert_eq!(reopened.state()["language"], "tr");
    assert_eq!(reopened.state()["read_only_guard"], false);
}

#[test]
fn nvme_nguid_and_identify_controller_serial_map_reliably() {
    let mut nguid_only = disk();
    nguid_only.nvme_eui64.clear();
    let inv = json!({
        "disks": [{
            "number": 1,
            "serial": "e823_8fa6_bf53_0001_001b_448b_4f81_c123.",
            "nvme_serial": nguid_only.serial,
            "model": nguid_only.model,
            "unique_ids": ["e8238fa6bf530001001b448b4f81c123"],
            "volumes": [{"letter": "C", "filesystem": "NTFS"}]
        }]
    });
    let mut disks = vec![nguid_only];
    platform::attach_volumes(&mut disks, &inv);
    assert_eq!(disks[0].os_disk_number, Some(1));
    assert_eq!(
        disks[0].os_serial.as_deref(),
        Some("e823_8fa6_bf53_0001_001b_448b_4f81_c123.")
    );
    assert_eq!(disks[0].volumes.len(), 1);

    let mut raw: Value = serde_json::from_str(include_str!("fixtures/smart-1.json")).unwrap();
    raw["nvme_namespaces"] = json!([{
        "id": 1,
        "nguid": "e8238fa6bf530001001b448b4f81c123"
    }]);
    let d_nguid = model::normalise(&raw);
    assert_eq!(
        d_nguid.nvme_nguid,
        vec!["e8238fa6bf530001001b448b4f81c123".to_owned()]
    );
    let inv_vpd = json!({
        "disks": [{
            "number": 2,
            "serial": "",
            "model": d_nguid.model,
            "unique_ids": ["e8238fa6bf530001001b448b4f81c123"],
            "volumes": [{"letter": "E", "filesystem": "NTFS"}]
        }]
    });
    let mut disks = vec![d_nguid.clone()];
    platform::attach_volumes(&mut disks, &inv_vpd);
    assert_eq!(disks[0].os_disk_number, Some(2));
    assert_eq!(
        disks[0].os_serial.as_deref(),
        Some("e8238fa6bf530001001b448b4f81c123")
    );

    let mut dup = d_nguid.clone();
    dup.id = "dup-id".into();
    dup.serial = Some("OTHER-SERIAL".into());
    let mut colliding = vec![d_nguid, dup];
    platform::attach_volumes(&mut colliding, &inv_vpd);
    assert!(colliding.iter().all(|d| d.os_disk_number.is_none()));
}

#[test]
fn dual_identical_budget_nvmes_with_colliding_dummy_eui_map_via_nvme_serial() {
    let raw: Value = serde_json::from_str(include_str!("fixtures/smart-2.json")).unwrap();
    let mut d1 = model::normalise(&raw);
    d1.id = "budget-1".into();
    d1.serial = Some("BUDGET-SN-001".into());
    d1.nvme_eui64 = vec!["0000000000000001".into()];

    let mut d2 = model::normalise(&raw);
    d2.id = "budget-2".into();
    d2.serial = Some("BUDGET-SN-002".into());
    d2.nvme_eui64 = vec!["0000000000000001".into()];

    let inv_with_identify = json!({
        "disks": [
            {
                "number": 2,
                "serial": "0000_0000_0000_0001.",
                "nvme_serial": "BUDGET-SN-001",
                "model": d1.model,
                "disk_bytes": 1024209543168u64,
                "unique_ids": ["0000000000000001"],
                "volumes": [{"letter": "D", "filesystem": "NTFS"}]
            },
            {
                "number": 3,
                "serial": "0000_0000_0000_0001.",
                "nvme_serial": "BUDGET-SN-002",
                "model": d2.model,
                "disk_bytes": 1024209543168u64,
                "unique_ids": ["0000000000000001"],
                "volumes": [{"letter": "E", "filesystem": "NTFS"}]
            }
        ]
    });
    let mut pair = vec![d1.clone(), d2.clone()];
    platform::attach_volumes(&mut pair, &inv_with_identify);
    assert_eq!(pair[0].os_disk_number, Some(2));
    assert_eq!(pair[0].volumes[0]["letter"], "D");
    assert_eq!(pair[1].os_disk_number, Some(3));
    assert_eq!(pair[1].volumes[0]["letter"], "E");

    let inv_without_identify = json!({
        "disks": [
            {
                "number": 2,
                "serial": "0000_0000_0000_0001.",
                "model": d1.model,
                "disk_bytes": 1024209543168u64,
                "unique_ids": ["0000000000000001"],
                "volumes": [{"letter": "D", "filesystem": "NTFS"}]
            },
            {
                "number": 3,
                "serial": "0000_0000_0000_0001.",
                "model": d2.model,
                "disk_bytes": 1024209543168u64,
                "unique_ids": ["0000000000000001"],
                "volumes": [{"letter": "E", "filesystem": "NTFS"}]
            }
        ]
    });
    let mut ambiguous = vec![d1, d2];
    platform::attach_volumes(&mut ambiguous, &inv_without_identify);
    assert!(ambiguous.iter().all(|d| d.os_disk_number.is_none()));
}

#[test]
fn model_prefix_suffix_and_scsi_truncation_map_without_allowing_unrelated_models() {
    let mut d = disk();
    d.model = "Samsung SSD 980 PRO 1TB".into();
    d.serial = Some("S6B0NG0T123456A".into());
    d.nvme_eui64.clear();

    let inv_nvme_prefix = json!({
        "disks": [{
            "number": 0,
            "serial": "S6B0NG0T123456A",
            "model": "NVMe Samsung SSD 980 PRO 1TB",
            "volumes": [{"letter": "C", "filesystem": "NTFS"}]
        }]
    });
    let mut disks = vec![d.clone()];
    platform::attach_volumes(&mut disks, &inv_nvme_prefix);
    assert_eq!(disks[0].os_disk_number, Some(0));

    let mut hdd = model::normalise(
        &serde_json::from_str::<Value>(include_str!("fixtures/smart-0.json")).unwrap(),
    );
    let inv_ata_suffix = json!({
        "disks": [{
            "number": 1,
            "serial": hdd.serial,
            "model": "ATA  WDC WD10EZEX-21M2NA0 SCSI Disk Device",
            "volumes": [{"letter": "D", "filesystem": "NTFS"}]
        }]
    });
    let mut hdds = vec![hdd.clone()];
    platform::attach_volumes(&mut hdds, &inv_ata_suffix);
    assert_eq!(hdds[0].os_disk_number, Some(1));

    d.model = "SAMSUNG MZVL4512HBLU-00BH1".into();
    let inv_truncated = json!({
        "disks": [{
            "number": 4,
            "serial": "S6B0NG0T123456A",
            "model": "NVMe SAMSUNG MZVL4512",
            "volumes": [{"letter": "F", "filesystem": "NTFS"}]
        }]
    });
    let mut trunc_disks = vec![d.clone()];
    platform::attach_volumes(&mut trunc_disks, &inv_truncated);
    assert_eq!(trunc_disks[0].os_disk_number, Some(4));

    let inv_unrelated = json!({
        "disks": [{
            "number": 5,
            "serial": "S6B0NG0T123456A",
            "model": "Other Brand 512G",
            "volumes": [{"letter": "G", "filesystem": "NTFS"}]
        }]
    });
    let mut unrelated = vec![d];
    platform::attach_volumes(&mut unrelated, &inv_unrelated);
    assert_eq!(unrelated[0].os_disk_number, None);

    hdd.model = "WDC WD10EZEX-21M2NA0".into();
    let inv_wrong_model = json!({
        "disks": [{
            "number": 6,
            "serial": hdd.serial,
            "model": "Seagate Barracuda 1TB",
            "volumes": []
        }]
    });
    let mut bad_hdds = vec![hdd];
    platform::attach_volumes(&mut bad_hdds, &inv_wrong_model);
    assert_eq!(bad_hdds[0].os_disk_number, None);
}

#[test]
fn sata_hex_encoded_byte_swapped_serial_and_wwn_map_reliably() {
    let hdd = model::normalise(
        &serde_json::from_str::<Value>(include_str!("fixtures/smart-0.json")).unwrap(),
    );
    assert_eq!(hdd.wwn.as_deref(), Some("50014ee2540be401"));

    for encoded in [
        "2020202057442d44454d4f2d484444303031",
        "202020204457442d4d452d4f444830443130",
        "DW-DMEO-DH0D10",
        "DEMO-HDD001",
    ] {
        let inv = json!({
            "disks": [{
                "number": 0,
                "serial": encoded,
                "model": hdd.model,
                "volumes": [{"letter": "D", "filesystem": "NTFS"}]
            }]
        });
        let mut disks = vec![hdd.clone()];
        platform::attach_volumes(&mut disks, &inv);
        assert_eq!(
            disks[0].os_disk_number,
            Some(0),
            "expected serial variant {encoded} to map"
        );
    }

    let inv_wwn = json!({
        "disks": [{
            "number": 2,
            "serial": "",
            "model": hdd.model,
            "unique_ids": ["50014ee2540be401"],
            "volumes": [{"letter": "H", "filesystem": "NTFS"}]
        }]
    });
    let mut disks = vec![hdd];
    platform::attach_volumes(&mut disks, &inv_wwn);
    assert_eq!(disks[0].os_disk_number, Some(2));
    assert_eq!(disks[0].os_serial.as_deref(), Some("50014ee2540be401"));
}

#[test]
fn tier3_unique_capacity_fallback_handles_unexposed_nguid_and_blocks_duplicates_and_contradictions()
{
    let mut nvme = disk();
    nvme.nvme_eui64.clear();

    let inv_ok = json!({
        "disks": [{
            "number": 1,
            "serial": "e823_8fa6_bf53_0001_001b_448b_4f81_c123.",
            "model": nvme.model,
            "bus_type": 17,
            "disk_bytes": 512110190592u64,
            "volumes": [{"letter": "C", "filesystem": "NTFS"}]
        }]
    });
    let mut single = vec![nvme.clone()];
    platform::attach_volumes(&mut single, &inv_ok);
    assert_eq!(single[0].os_disk_number, Some(1));

    let mut nvme2 = nvme.clone();
    nvme2.id = "second-nvme".into();
    nvme2.serial = Some("GB-DEMO-NVME02".into());
    let inv_two = json!({
        "disks": [
            {
                "number": 1,
                "serial": "e823_8fa6_bf53_0001_001b_448b_4f81_c123.",
                "model": nvme.model,
                "bus_type": 17,
                "disk_bytes": 512110190592u64,
                "volumes": [{"letter": "C", "filesystem": "NTFS"}]
            },
            {
                "number": 2,
                "serial": "e823_8fa6_bf53_0001_001b_448b_4f81_c456.",
                "model": nvme.model,
                "bus_type": 17,
                "disk_bytes": 512110190592u64,
                "volumes": [{"letter": "D", "filesystem": "NTFS"}]
            }
        ]
    });
    let mut pair = vec![nvme.clone(), nvme2];
    platform::attach_volumes(&mut pair, &inv_two);
    assert!(pair.iter().all(|d| d.os_disk_number.is_none()));

    let inv_contradiction = json!({
        "disks": [{
            "number": 1,
            "serial": "TOTALLY-DIFFERENT-SN",
            "model": nvme.model,
            "bus_type": 17,
            "disk_bytes": 512110190592u64,
            "volumes": [{"letter": "C", "filesystem": "NTFS"}]
        }]
    });
    let mut bad_ser = vec![nvme.clone()];
    platform::attach_volumes(&mut bad_ser, &inv_contradiction);
    assert_eq!(bad_ser[0].os_disk_number, None);

    let inv_wrong_bus = json!({
        "disks": [{
            "number": 1,
            "serial": "e823_8fa6_bf53_0001_001b_448b_4f81_c123.",
            "model": nvme.model,
            "bus_type": 11,
            "disk_bytes": 512110190592u64,
            "volumes": [{"letter": "C", "filesystem": "NTFS"}]
        }]
    });
    let mut bad_bus = vec![nvme];
    platform::attach_volumes(&mut bad_bus, &inv_wrong_bus);
    assert_eq!(bad_bus[0].os_disk_number, None);
}

#[test]
fn admin_scan_open_errors_and_missing_types_do_not_drop_healthy_drives() {
    let scan = json!({
        "smartctl": { "exit_status": 2 },
        "devices": [
            { "name": "/dev/csmi0,1", "open_error": "Input/output error" },
            { "name": "invalid_relative_path", "type": "ata" },
            { "name": "/dev/sda", "info_name": "/dev/sda", "type": "nvme", "protocol": "NVMe" },
            { "name": "/dev/sdb", "info_name": "/dev/sdb", "type": "nvme", "protocol": "NVMe" },
            { "name": "/dev/pd2" },
            { "name": "/dev/sda", "type": "nvme" }
        ]
    });
    let (targets, notes) = ssd_sentinel_core::core::parse_scan_devices(&scan);
    assert_eq!(
        targets,
        vec![
            ("/dev/sda".to_owned(), Some("nvme".to_owned())),
            ("/dev/sdb".to_owned(), Some("nvme".to_owned())),
            ("/dev/pd2".to_owned(), None),
        ]
    );
    assert_eq!(notes, vec!["/dev/csmi0,1: Input/output error".to_owned()]);
}

#[test]
fn admin_live_duplicate_csmi_and_nvme_aliases_deduplicate_safely_while_rejecting_real_conflicts() {
    let raw_corsair = json!({
        "model_name": "Corsair Force MP600",
        "serial_number": "CORS-987654",
        "firmware_version": "EGFM11.3",
        "device": { "name": "/dev/sda", "type": "nvme", "protocol": "NVMe" },
        "smartctl": { "exit_status": 0 },
        "smart_status": { "passed": true },
        "user_capacity": { "bytes": 1_000_204_886_016u64 },
        "nvme_smart_health_information_log": {
            "critical_warning": 0,
            "temperature": 40,
            "available_spare": 100,
            "available_spare_threshold": 10,
            "percentage_used": 2,
            "data_units_written": 1000,
            "data_units_read": 2000,
            "media_errors": 0,
            "num_err_log_entries": 0,
            "power_on_hours": 120,
            "power_cycles": 30
        }
    });
    let mut raw_corsair_csmi = raw_corsair.clone();
    raw_corsair_csmi["device"] =
        json!({ "name": "/dev/csmi0,0", "type": "ata", "protocol": "ATA" });
    raw_corsair_csmi["smartctl"] = json!({ "exit_status": 4 });
    raw_corsair_csmi
        .as_object_mut()
        .unwrap()
        .remove("nvme_smart_health_information_log");

    let raw_samsung = json!({
        "model_name": "Samsung SSD 970 EVO Plus 1TB",
        "serial_number": "S4EWNF0N123456",
        "firmware_version": "2B2QEXM7",
        "device": { "name": "/dev/sdb", "type": "nvme", "protocol": "NVMe" },
        "smartctl": { "exit_status": 0 },
        "smart_status": { "passed": true },
        "user_capacity": { "bytes": 1_000_204_886_016u64 },
        "nvme_smart_health_information_log": {
            "critical_warning": 0,
            "temperature": 44,
            "available_spare": 100,
            "available_spare_threshold": 10,
            "percentage_used": 5,
            "data_units_written": 5000,
            "data_units_read": 6000,
            "media_errors": 0,
            "num_err_log_entries": 0,
            "power_on_hours": 450,
            "power_cycles": 90
        }
    });
    let mut raw_samsung_nvme0 = raw_samsung.clone();
    raw_samsung_nvme0["device"] =
        json!({ "name": "/dev/nvme0", "type": "nvme", "protocol": "NVMe" });

    let deduped = ssd_sentinel_core::core::deduplicate_live_aliases(vec![
        (
            model::normalise(&raw_corsair_csmi),
            raw_corsair_csmi.clone(),
        ),
        (model::normalise(&raw_corsair), raw_corsair.clone()),
        (model::normalise(&raw_samsung_nvme0), raw_samsung_nvme0),
        (model::normalise(&raw_samsung), raw_samsung.clone()),
    ])
    .unwrap();
    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].0.model, "Corsair Force MP600");
    assert_eq!(deduped[0].0.device, "/dev/sda");
    assert!(!deduped[0].0.read_incomplete);
    assert_eq!(deduped[1].0.model, "Samsung SSD 970 EVO Plus 1TB");
    assert_eq!(deduped[1].0.device, "/dev/nvme0");

    assert!(ssd_sentinel_core::core::deduplicate_live_aliases(vec![
        (model::normalise(&raw_corsair), raw_corsair.clone()),
        (model::normalise(&raw_corsair), raw_corsair.clone()),
    ])
    .is_err());

    let mut conflict_fw = raw_corsair.clone();
    conflict_fw["device"]["name"] = json!("/dev/pd0");
    conflict_fw["firmware_version"] = json!("DIFFERENT-FW");
    assert!(ssd_sentinel_core::core::deduplicate_live_aliases(vec![
        (model::normalise(&raw_corsair), raw_corsair),
        (model::normalise(&conflict_fw), conflict_fw),
    ])
    .is_err());
}
