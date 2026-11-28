#![cfg(target_os = "linux")]

use serde_json::json;
use sha2::{Digest, Sha256};
use ssd_sentinel_core::{
    core::{Config, Core},
    dev_workspace, io_linux,
    jobs::Row,
    linux_priv::{self, HelperOp},
    model,
    optimize::OptimizeRequest,
    platform_linux,
    recovery::{self, linux_rec},
    tiering,
};
use std::{
    fs,
    path::Path,
    sync::{atomic::AtomicBool, Mutex},
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn linux_mountinfo_diskstats_and_sysfs_fallback_parsers_preserve_invariants() {
    let mount_sample = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
35 22 259:3 / /mnt/My\\040Archive rw,nosuid - btrfs /dev/sda1 rw,compress=zstd
40 22 0:25 / /proc rw,nosuid,nodev,noexec - proc proc rw";
    let mounts = platform_linux::parse_mountinfo_str(mount_sample);
    assert_eq!(mounts.len(), 3);
    assert_eq!(mounts[0].mount_point, "/");
    assert_eq!(mounts[0].fstype, "ext4");
    assert_eq!(mounts[0].source, "/dev/nvme0n1p2");
    assert_eq!(mounts[1].mount_point, "/mnt/My Archive");
    assert_eq!(mounts[1].fstype, "btrfs");

    let stat_sample = "\
 259       0 nvme0n1 1234 50 987654 310 567 10 432100 180 2 490 0
   8       0 sda 400 0 16000 120 200 0 8000 90 0 200 0";
    let stats = platform_linux::parse_diskstats_str(stat_sample);
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].name, "nvme0n1");
    assert_eq!(stats[0].sectors_read, 987654);
    assert_eq!(stats[0].ios_in_progress, 2);
    assert_eq!(stats[1].name, "sda");
    assert_eq!(stats[1].sectors_written, 8000);

    assert_eq!(platform_linux::parse_pcie_gen("16.0 GT/s PCIe"), Some(4));
    assert_eq!(platform_linux::parse_pcie_gen("32.0 GT/s PCIe"), Some(5));
    let (cur_sched, avail) = platform_linux::parse_scheduler_str("[none] mq-deadline kyber bfq");
    assert_eq!(cur_sched.as_deref(), Some("none"));
    assert_eq!(avail, vec!["none", "mq-deadline", "kyber", "bfq"]);

    let fake_inv = json!({
        "disks": [{
            "device_path": "/dev/nvme0n1",
            "model": "Samsung 990 PRO 2TB",
            "serial": "S700NJ0X000123",
            "disk_bytes": 2_000_398_934_016u64,
            "bus_type": 17u64,
            "rotational": 0u64
        }]
    });
    let snaps = platform_linux::synthesize_sysfs_fallback_snapshots(&fake_inv);
    assert_eq!(snaps.len(), 1);
    let disk = model::normalise(&snaps[0]);
    assert!(disk.read_incomplete);
    assert!(disk.health.percent.is_none());
}

#[test]
fn io_linux_runs_sequential_and_random_bench_in_tempdir() {
    let dir = tempfile::tempdir().unwrap();
    let bench_file = dir.path().join("bench-test.dat");
    fs::write(&bench_file, vec![0u8; 4 * 1024 * 1024]).unwrap();
    let cancel = AtomicBool::new(false);
    let seq_row = Row {
        id: "seq".into(),
        name: "Seq 1M".into(),
        block_kib: 128,
        queue: 4,
        threads: 1,
        random: false,
    };
    let w_res = io_linux::run(
        &bench_file,
        4 * 1024 * 1024,
        &seq_row,
        "write",
        1,
        false,
        &cancel,
    )
    .expect("io_linux sequential write");
    assert!(w_res["mb_s"].as_f64().unwrap_or(0.0) > 0.0);

    let rand_row = Row {
        id: "rnd".into(),
        name: "Rand 4K".into(),
        block_kib: 4,
        queue: 8,
        threads: 1,
        random: true,
    };
    let r_res = io_linux::run(
        &bench_file,
        4 * 1024 * 1024,
        &rand_row,
        "read",
        1,
        false,
        &cancel,
    )
    .expect("io_linux random read");
    assert!(r_res["iops"].as_f64().unwrap_or(0.0) > 0.0);
}

#[test]
fn linux_forbidden_paths_are_strictly_rejected() {
    for p in [
        "/",
        "/etc",
        "/etc/passwd",
        "/usr",
        "/usr/bin/bash",
        "/boot/vmlinuz-linux",
        "/proc/1/status",
        "/sys/block/sda",
        "/dev/sda",
        "/var/lib/dpkg/status",
        "/var/lib/pacman/local",
        "/home",
        "/root/.ssh/id_ed25519",
    ] {
        assert!(
            tiering::is_forbidden_path(Path::new(p)),
            "Expected Linux protected path to be rejected: {p}"
        );
    }
}

#[test]
fn tiering_relocate_symlink_sha256_rollback_and_cancel_work_on_linux() {
    let src_root = tempfile::tempdir().unwrap();
    let dst_root = tempfile::tempdir().unwrap();
    let project_dir = src_root.path().join("MediaArchive");
    fs::create_dir_all(&project_dir).unwrap();
    let payload: Vec<u8> = (0..131_072u32).map(|i| (i % 251) as u8).collect();
    let expected_sha: String = Sha256::digest(&payload)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    fs::write(project_dir.join("clip.mkv"), &payload).unwrap();

    let rec = tiering::relocate_item_into_dir(&project_dir, dst_root.path(), "to_hdd", true, 0)
        .expect("tiering relocate");
    let target_dir = Path::new(rec["target_path"].as_str().unwrap());
    assert!(target_dir.join("clip.mkv").is_file());
    let link_meta = fs::symlink_metadata(&project_dir).unwrap();
    assert!(link_meta.file_type().is_symlink());
    assert_eq!(rec["junction_created"], true);
    assert_eq!(rec["size_bytes"], payload.len() as u64);
    let moved_bytes = fs::read(target_dir.join("clip.mkv")).unwrap();
    let moved_sha: String = Sha256::digest(&moved_bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(moved_sha, expected_sha);

    tiering::rollback_move(&rec).expect("tiering rollback");
    let restored_meta = fs::symlink_metadata(&project_dir).unwrap();
    assert!(restored_meta.is_dir());
    assert!(!target_dir.exists());
    assert_eq!(fs::read(project_dir.join("clip.mkv")).unwrap(), payload);

    // Pre-cancelled relocate leaves source completely untouched
    let cancel = AtomicBool::new(true);
    let err = tiering::relocate_item_into_dir_with_cancel(
        &project_dir,
        dst_root.path(),
        "to_hdd",
        true,
        0,
        &cancel,
        &mut |_, _, _, _, _| {},
    );
    assert!(err.is_err());
    assert_eq!(fs::read(project_dir.join("clip.mkv")).unwrap(), payload);
}

#[test]
fn xdg_trash_parsing_recovery_and_same_mount_anti_overwrite_work() {
    let parsed = linux_rec::parse_trashinfo(
        "[Trash Info]\nPath=/home/omer/Documents/Notlar%20%C4%9F%C3%BC%C5%9F%C3%B6%C3%A7.txt\nDeletionDate=2026-11-28T10:15:30\n",
    )
    .expect("parse Turkish UTF-8 trashinfo");
    assert_eq!(parsed.0, "/home/omer/Documents/Notlar ğüşöç.txt");
    assert_eq!(parsed.1.as_deref(), Some("2026-11-28T10:15:30Z"));

    let tmp = tempfile::tempdir().unwrap();
    let trash = tmp.path().join("Trash");
    fs::create_dir_all(trash.join("info")).unwrap();
    fs::create_dir_all(trash.join("files")).unwrap();
    fs::write(
        trash.join("info/rapor.txt.trashinfo"),
        "[Trash Info]\nPath=/mnt/other/rapor%20yeni.txt\nDeletionDate=2026-11-28T11:00:00\n",
    )
    .unwrap();
    fs::write(trash.join("files/rapor.txt"), b"onemli icerik").unwrap();

    let items = linux_rec::scan_xdg_trash_dir(&trash, "/mnt/other", true, &|| false);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "rapor yeni.txt");
    assert_eq!(items[0].source, "recycle_bin");

    // Restoring raw/MFT item onto the mount that owns `dst` must fail (S5 anti-overwrite).
    let dst = tmp.path().join("restore_out");
    fs::create_dir_all(&dst).unwrap();
    let dst_str = dst.to_string_lossy();
    let owning_mount = platform_linux::mounted_volumes()
        .into_iter()
        .filter(|v| {
            let mp = v.mount_point.as_str();
            mp == "/" || dst_str.as_ref() == mp || dst_str.starts_with(&format!("{mp}/"))
        })
        .max_by_key(|v| v.mount_point.len())
        .map(|v| v.mount_point)
        .unwrap_or_else(|| "/".into());

    let raw_mft = json!({
        "source": "mft_resident",
        "trim_zeroed": false,
        "locator": { "resident_hex": "68656c6c6f" }
    });
    assert!(recovery::restore_item(&owning_mount, &raw_mft, &dst.join("blocked.txt")).is_err());
    assert!(!dst.join("blocked.txt").exists());
}

#[test]
fn dev_workspace_cleans_cargo_target_and_protects_git_and_ai_models() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tmp.path().join("rust-app");
    fs::create_dir_all(proj.join("target")).unwrap();
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::write(proj.join("Cargo.toml"), b"[package]\nname=\"demo\"\n").unwrap();
    fs::write(proj.join("target/debug_artifact.bin"), vec![1u8; 8192]).unwrap();
    fs::write(proj.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    let model_dir = tmp.path().join("models");
    fs::create_dir_all(&model_dir).unwrap();
    fs::write(model_dir.join("llama.gguf"), vec![7u8; 4096]).unwrap();

    let found = dev_workspace::scan_directory_projects(tmp.path());
    assert_eq!(found.len(), 1);
    assert!(found[0].artifact_bytes >= 8192);

    let ok = dev_workspace::validate_cleanable_dev_path(
        &proj.join("target"),
        &tiering::is_forbidden_path,
    );
    assert!(ok.is_ok(), "paired target should be cleanable");
    assert!(dev_workspace::validate_cleanable_dev_path(
        &proj.join(".git"),
        &tiering::is_forbidden_path
    )
    .is_err());
    assert!(
        dev_workspace::validate_cleanable_dev_path(&model_dir, &tiering::is_forbidden_path)
            .is_err()
    );
}

#[test]
fn xdg_autostart_creates_and_rolls_back_desktop_entry_in_isolated_config_home() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let old_xdg = std::env::var_os("XDG_CONFIG_HOME");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
    }

    let data_dir = tmp.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    let core = Core::new(Config {
        smartctl: platform_linux::discover_smartctl(),
        data_dir,
        snapshot_dir: None,
    })
    .unwrap();
    let _ = core.refresh_now();
    // Ensure fresh non-stale state on containers with virtual disks if needed
    let res = core.optimize_action(OptimizeRequest {
        action: "autostart".into(),
        target: "system".into(),
        value: "enable".into(),
        confirmation: "optimize:autostart:system".into(),
    });
    if core.state()["stale"] == false {
        assert!(res.is_ok(), "autostart enable failed: {res:?}");
        let desktop_file = tmp.path().join("autostart/ssd-sentinel.desktop");
        assert!(desktop_file.is_file());
        core.optimize_action(OptimizeRequest {
            action: "autostart".into(),
            target: "system".into(),
            value: "disable".into(),
            confirmation: "optimize:autostart:system".into(),
        })
        .unwrap();
        assert!(!desktop_file.exists());
    }
    core.shutdown();

    match old_xdg {
        Some(v) => unsafe { std::env::set_var("XDG_CONFIG_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_CONFIG_HOME") },
    }
}

#[test]
fn polkit_helper_allowlist_rejects_traversal_passwd_and_invalid_inputs() {
    assert!(!linux_priv::is_valid_block_dev_path("/etc/passwd"));
    assert!(!linux_priv::is_valid_block_dev_path("/dev/../etc/shadow"));
    assert!(linux_priv::validate_writable_sysfs_path("/etc/shadow").is_err());
    assert!(linux_priv::validate_writable_sysfs_path("/sys/block/../kernel/panic").is_err());

    assert!(linux_priv::execute_op_local(HelperOp::SmartQuery {
        device: "/etc/passwd".into(),
        device_type: None,
        mode: "all".into(),
    })
    .is_err());
    assert!(linux_priv::execute_op_local(HelperOp::SmartQuery {
        device: "/dev/sda".into(),
        device_type: Some("bad;rm".into()),
        mode: "all".into(),
    })
    .is_err());
    assert!(linux_priv::execute_op_local(HelperOp::WriteSysfs {
        path: "/etc/hosts".into(),
        value: "oops".into(),
    })
    .is_err());
    assert!(linux_priv::execute_op_local(HelperOp::FsDefrag {
        path: "/does/not/../exist".into(),
        fstype: "ext4".into(),
    })
    .is_err());
}
