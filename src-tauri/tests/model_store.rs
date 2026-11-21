use serde_json::{json, Value};
use ssd_sentinel_core::{
    model::{health_with_mode, normalise, normalise_with_mode},
    store::Store,
};

fn fixture(index: usize) -> Value {
    let path = format!(
        "{}/tests/fixtures/smart-{index}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn captured_drives_keep_health_identity_and_exact_raw_values() {
    let ids = [
        "bd3b66d08395846ff762c191",
        "5d2acd56541b332f41cb92ce",
        "866b3c39d831c7ae07d38d1a",
    ];
    for (i, percent) in [100, 73, 98].iter().enumerate() {
        let value = fixture(i);
        let disk = normalise(&value);
        assert_eq!(disk.health.percent, Some(*percent));
        assert!(disk.stable_identity);
        assert_eq!(disk.id.len(), 24);
        assert_eq!(
            disk.id, ids[i],
            "Existing Python history must use the same identity"
        );
        assert!(!disk.smart.is_empty());
    }
    let mut v = fixture(1);
    v["nvme_smart_health_information_log"]["host_reads"] =
        serde_json::from_str("18446744073709551617001").unwrap();
    let d = normalise(&v);
    assert_eq!(
        d.smart.iter().find(|r| r.id == "host_reads").unwrap().raw,
        "18446744073709551617001"
    );
    assert_eq!(normalise(&fixture(2)).health.status, "warning");
}

#[test]
fn critical_missing_and_unsupported_fields_never_become_healthy() {
    let mut v = fixture(1);
    v["nvme_smart_health_information_log"]["critical_warning"] = json!(1);
    let disk = normalise(&v);
    assert_eq!(disk.health.percent, None);
    assert_eq!(disk.health.status, "critical");
    v["nvme_smart_health_information_log"]["critical_warning"] = json!(0);
    v["nvme_smart_health_information_log"]
        .as_object_mut()
        .unwrap()
        .remove("available_spare");
    assert_eq!(normalise(&v).health.percent, None);
    assert_eq!(normalise(&json!({})).health.percent, None);
    let mut hdd = fixture(0);
    hdd["rotation_rate"] = json!(0);
    assert_eq!(normalise(&hdd).health.percent, None);
}

#[test]
fn malformed_smart_metadata_cannot_hide_an_incomplete_read() {
    let mut v = fixture(1);
    v["smartctl"] = json!([]);
    assert_eq!(normalise(&v).health.percent, None);
    v["smartctl"] = json!({"exit_status":true});
    assert_eq!(normalise(&v).health.percent, None);
}

#[test]
fn hdd_rejects_duplicate_required_attributes_and_uses_unsigned_low_word() {
    let mut v = fixture(0);
    let table = v["ata_smart_attributes"]["table"].as_array_mut().unwrap();
    let row = table.iter().find(|r| r["id"] == 5).unwrap().clone();
    table.push(row);
    assert_eq!(normalise(&v).health.percent, None);
    let mut v = fixture(0);
    let row = v["ata_smart_attributes"]["table"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["id"] == 5)
        .unwrap();
    row["raw"]["value"] = json!(65537);
    assert_eq!(normalise(&v).health.percent, Some(99));
}

#[test]
fn history_reopens_preserves_replay_time_and_deduplicates_capture() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.sqlite3");
    let d = normalise(&fixture(1));
    {
        let mut store = Store::open(&file).unwrap();
        store
            .record(std::slice::from_ref(&d), "replay:test")
            .unwrap();
        store
            .record(std::slice::from_ref(&d), "replay:test")
            .unwrap();
        assert_eq!(
            store
                .history(&d.id, "replay:test")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .history(&d.id, "live")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        assert!(store
            .events(None, "replay:test")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
        store
            .save_job(
                "benchmark",
                &json!({"id":"a","disk_id":d.id,"status":"completed"}),
            )
            .unwrap();
    }
    let store = Store::open(&file).unwrap();
    assert_eq!(store.jobs("benchmark", Some(&d.id)).unwrap()[0]["id"], "a");
    assert_eq!(
        store.jobs("benchmark", Some(&d.id)).unwrap()[0]["kind"],
        "benchmark"
    );
}

#[test]
fn initial_counters_are_not_new_alerts_but_increases_are() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("h.db")).unwrap();
    let mut d = normalise(&fixture(2));
    store.record(&[d.clone()], "live").unwrap();
    assert_eq!(
        store
            .events(None, "live")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
    d.smart
        .iter_mut()
        .find(|r| r.id == "media_errors")
        .unwrap()
        .raw = "2".into();
    store.record(&[d.clone()], "live").unwrap();
    assert_eq!(
        store.events(Some(&d.id), "live").unwrap()[0]["code"],
        "counter:media_errors"
    );
}

#[test]
fn sata_ssd_wear_and_tbw_metrics_support_vendor_attributes() {
    let sata_177 = json!({
        "model_name": "Samsung SSD 870 EVO 500GB",
        "serial_number": "S5P2NX0R123456",
        "rotation_rate": 0,
        "device": { "name": "/dev/sda", "protocol": "ATA" },
        "smart_status": { "passed": true },
        "smartctl": { "exit_status": 0 },
        "user_capacity": { "bytes": 500_107_862_016u64 },
        "ata_smart_attributes": {
            "table": [
                { "id": 177, "name": "Wear_Leveling_Count", "value": 92, "raw": { "value": 85 } },
                { "id": 241, "name": "Total_LBAs_Written", "value": 99, "raw": { "value": 2_000_000 } },
                { "id": 242, "name": "Total_LBAs_Read", "value": 99, "raw": { "value": 1_000_000 } }
            ]
        }
    });
    let disk = normalise(&sata_177);
    assert_eq!(disk.kind, "SATA SSD");
    assert_eq!(disk.health.percent, Some(92));
    assert_eq!(
        disk.health.method.as_deref(),
        Some("sata-ssd-wear-indicator")
    );
    assert_eq!(disk.metrics["written_bytes"], "1024000000");
    assert_eq!(disk.metrics["read_bytes"], "512000000");

    let mut temp_only_231 = sata_177.clone();
    temp_only_231["ata_smart_attributes"]["table"] = json!([
        { "id": 231, "name": "Temperature_Celsius", "value": 35, "raw": { "value": 35 } }
    ]);
    assert_eq!(normalise(&temp_only_231).health.percent, None);

    temp_only_231["ata_smart_attributes"]["table"] = json!([
        { "id": 231, "name": "SSD_Life_Left", "value": 85, "raw": { "value": 15 } }
    ]);
    assert_eq!(normalise(&temp_only_231).health.percent, Some(85));

    let dev_stats = json!({
        "model_name": "Crucial MX500",
        "serial_number": "CT500MX500",
        "rotation_rate": 0,
        "device": { "name": "/dev/sdb", "protocol": "ATA" },
        "smart_status": { "passed": true },
        "smartctl": { "exit_status": 0 },
        "ata_smart_attributes": {
            "table": [
                { "id": 202, "name": "Percent_Lifetime_Remain", "value": 97, "raw": { "value": 3 } }
            ]
        },
        "ata_device_statistics": {
            "pages": [
                {
                    "table": [
                        { "name": "Logical Sectors Written", "value": 4_000_000 }
                    ]
                }
            ]
        }
    });
    let stat_disk = normalise(&dev_stats);
    assert_eq!(stat_disk.health.percent, Some(97));
    assert_eq!(stat_disk.metrics["written_bytes"], "2048000000");
}

#[test]
fn strict_health_mode_applies_hdd_tail_curve_and_preserves_default() {
    let base_hdd = fixture(0);
    assert_eq!(health_with_mode(&base_hdd, false).percent, Some(100));
    assert_eq!(health_with_mode(&base_hdd, true).percent, Some(100));

    let mut degraded = base_hdd.clone();
    for row in degraded["ata_smart_attributes"]["table"]
        .as_array_mut()
        .unwrap()
    {
        if row["id"] == 5 {
            row["raw"]["value"] = json!(15);
        }
    }
    let default_disk = normalise_with_mode(&degraded, false);
    let strict_disk = normalise_with_mode(&degraded, true);
    assert!(strict_disk.health.percent.unwrap() < default_disk.health.percent.unwrap());
}

#[test]
fn store_optimizations_preserve_initial_baseline_and_defrag_jobs_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(&dir.path().join("opt.sqlite3")).unwrap();
    store
        .record_optimization("write_cache:disk1", "disk1", "WC", "disable", "enable")
        .unwrap();
    store
        .record_optimization(
            "write_cache:disk1",
            "disk1",
            "WC Updated",
            "enable",
            "enable",
        )
        .unwrap();
    let rec = store
        .get_optimization("write_cache:disk1")
        .unwrap()
        .unwrap();
    assert_eq!(rec["previous_value"], "disable");
    assert_eq!(rec["title"], "WC Updated");
    assert_eq!(
        store
            .list_optimizations()
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    store.remove_optimization("write_cache:disk1").unwrap();
    assert!(store
        .get_optimization("write_cache:disk1")
        .unwrap()
        .is_none());

    store
        .save_job(
            "defrag",
            &json!({"id":"df1","disk_id":"disk1","status":"completed"}),
        )
        .unwrap();
    let jobs = store.jobs("defrag", Some("disk1")).unwrap();
    assert_eq!(jobs[0]["id"], "df1");
    assert_eq!(jobs[0]["kind"], "defrag");
}
