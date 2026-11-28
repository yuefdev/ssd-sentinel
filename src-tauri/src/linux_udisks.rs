//! Passwordless SMART telemetry fallback via Freedesktop UDisks2 D-Bus (`org.freedesktop.UDisks2`).
//!
//! When running unprivileged on GNOME, KDE, Ubuntu, Fedora, Arch, etc., `udisks2` exposes
//! cached SMART attributes and NVMe health logs over D-Bus without requiring `pkexec`.
//! This module translates UDisks2 D-Bus objects into `smartctl -j`-compatible JSON snapshots
//! so `model::normalise_with_mode()` produces identical health scores, alerts, and history.

use serde_json::{json, Value};
use std::collections::HashMap;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as ZValue};

type AtaAttrRow = (
    u8,
    String,
    u16,
    i32,
    i32,
    i32,
    i64,
    i32,
    HashMap<String, OwnedValue>,
);

fn prop_str(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let val = props.get(key)?;
    match &**val {
        ZValue::Str(s) => Some(s.as_str().trim().to_owned()).filter(|x| !x.is_empty()),
        ZValue::Array(arr) => {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|v| match v {
                    ZValue::U8(b) if *b != 0 => Some(*b),
                    _ => None,
                })
                .collect();
            String::from_utf8(bytes)
                .ok()
                .map(|s| s.trim().to_owned())
                .filter(|x| !x.is_empty())
        }
        _ => None,
    }
}

fn prop_u64(props: &HashMap<String, OwnedValue>, key: &str) -> Option<u64> {
    let val = props.get(key)?;
    zval_u64(val)
}

fn prop_i64(props: &HashMap<String, OwnedValue>, key: &str) -> Option<i64> {
    let val = props.get(key)?;
    match &**val {
        ZValue::I64(n) => Some(*n),
        ZValue::I32(n) => Some(i64::from(*n)),
        ZValue::I16(n) => Some(i64::from(*n)),
        ZValue::U64(n) => i64::try_from(*n).ok(),
        ZValue::U32(n) => Some(i64::from(*n)),
        ZValue::U16(n) => Some(i64::from(*n)),
        ZValue::U8(n) => Some(i64::from(*n)),
        _ => None,
    }
}

fn prop_f64(props: &HashMap<String, OwnedValue>, key: &str) -> Option<f64> {
    let val = props.get(key)?;
    match &**val {
        ZValue::F64(n) if n.is_finite() => Some(*n),
        _ => prop_u64(props, key).map(|n| n as f64),
    }
}

fn prop_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    let val = props.get(key)?;
    match &**val {
        ZValue::Bool(b) => Some(*b),
        _ => None,
    }
}

fn prop_obj_path(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    let val = props.get(key)?;
    match &**val {
        ZValue::ObjectPath(p) => Some(p.as_str().to_owned()),
        _ => None,
    }
}

fn prop_str_list(props: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    let Some(val) = props.get(key) else {
        return Vec::new();
    };
    match &**val {
        ZValue::Array(arr) => arr
            .iter()
            .filter_map(|v| match v {
                ZValue::Str(s) => Some(s.as_str().to_owned()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn zval_u64(val: &ZValue<'_>) -> Option<u64> {
    match val {
        ZValue::U64(n) => Some(*n),
        ZValue::U32(n) => Some(u64::from(*n)),
        ZValue::U16(n) => Some(u64::from(*n)),
        ZValue::U8(n) => Some(u64::from(*n)),
        ZValue::I64(n) if *n >= 0 => Some(*n as u64),
        ZValue::I32(n) if *n >= 0 => Some(*n as u64),
        ZValue::I16(n) if *n >= 0 => Some(*n as u64),
        _ => None,
    }
}

fn kelvin_to_celsius(k: f64) -> Option<i64> {
    if k > 150.0 && k < 500.0 {
        Some((k - 273.15).round() as i64)
    } else {
        None
    }
}

fn ata_pretty_raw(pretty: i64, pretty_unit: i32) -> u64 {
    if pretty < 0 {
        return 0;
    }
    match pretty_unit {
        // UDisks2 pretty_unit 2 = milliseconds -> hours for attr 9
        2 => (pretty as u64) / 3_600_000,
        // 3 = millikelvin -> Celsius
        3 => ((pretty as f64 / 1000.0) - 273.15).round().max(0.0) as u64,
        _ => pretty as u64,
    }
}

/// Queries `org.freedesktop.UDisks2` over the system D-Bus and converts readable
/// drives into `smartctl -j` compatible snapshots. Returns empty on D-Bus absence.
pub fn collect_smart_snapshots() -> Vec<Value> {
    let Ok(conn) = zbus::blocking::Connection::system() else {
        return Vec::new();
    };
    let Ok(om) = zbus::blocking::fdo::ObjectManagerProxy::builder(&conn)
        .destination("org.freedesktop.UDisks2")
        .and_then(|b| b.path("/org/freedesktop/UDisks2"))
        .and_then(|b| b.build())
    else {
        return Vec::new();
    };
    let Ok(objects) = om.get_managed_objects() else {
        return Vec::new();
    };

    // Map drive object path -> whole-disk block device path (e.g. /dev/nvme0n1, /dev/sda)
    let mut drive_dev_map: HashMap<String, String> = HashMap::new();
    for ifaces in objects.values() {
        let block_iface: Option<&HashMap<String, OwnedValue>> = ifaces
            .iter()
            .find(|(k, _)| k.as_str() == "org.freedesktop.UDisks2.Block")
            .map(|(_, v)| v);
        let is_partition = ifaces
            .keys()
            .any(|k| k.as_str() == "org.freedesktop.UDisks2.Partition");
        if is_partition {
            continue;
        }
        if let Some(block) = block_iface {
            if let (Some(drive_path), Some(dev_path)) =
                (prop_obj_path(block, "Drive"), prop_str(block, "Device"))
            {
                if drive_path != "/" && dev_path.starts_with("/dev/") {
                    drive_dev_map.entry(drive_path).or_insert(dev_path);
                }
            }
        }
    }

    let mut out = Vec::new();
    for (obj_path, ifaces) in &objects {
        let Some(drive) = ifaces
            .iter()
            .find(|(k, _)| k.as_str() == "org.freedesktop.UDisks2.Drive")
            .map(|(_, v)| v)
        else {
            continue;
        };
        let drive_key = obj_path.as_str().to_owned();
        let Some(dev_path) = drive_dev_map.get(&drive_key).cloned() else {
            continue;
        };

        let model = prop_str(drive, "Model").unwrap_or_else(|| "Unknown Drive".into());
        let serial = prop_str(drive, "Serial");
        let firmware = prop_str(drive, "Revision");
        let size_bytes = prop_u64(drive, "Size").unwrap_or(0);
        let rotation = prop_i64(drive, "RotationRate");
        let bus = prop_str(drive, "ConnectionBus").unwrap_or_default();
        let wwn = prop_str(drive, "WWN");

        let ata_iface = ifaces
            .iter()
            .find(|(k, _)| k.as_str() == "org.freedesktop.UDisks2.Drive.Ata")
            .map(|(_, v)| v);
        let nvme_iface = ifaces
            .iter()
            .find(|(k, _)| k.as_str() == "org.freedesktop.UDisks2.NVMe.Controller")
            .map(|(_, v)| v);

        if let Some(nvme) = nvme_iface {
            if let Some(snap) = build_nvme_snapshot(
                &conn,
                obj_path,
                &dev_path,
                &model,
                serial.as_deref(),
                firmware.as_deref(),
                size_bytes,
                nvme,
            ) {
                out.push(snap);
                continue;
            }
        }

        if let Some(ata) = ata_iface {
            if prop_bool(ata, "SmartSupported") == Some(true) {
                if let Some(snap) = build_ata_snapshot(
                    &conn,
                    obj_path,
                    &dev_path,
                    &model,
                    serial.as_deref(),
                    firmware.as_deref(),
                    size_bytes,
                    rotation,
                    &bus,
                    wwn.as_deref(),
                    ata,
                ) {
                    out.push(snap);
                }
            }
        }
    }

    out
}

#[allow(clippy::too_many_arguments)]
fn build_nvme_snapshot(
    conn: &zbus::blocking::Connection,
    obj_path: &OwnedObjectPath,
    dev_path: &str,
    model: &str,
    serial: Option<&str>,
    firmware: Option<&str>,
    size_bytes: u64,
    nvme: &HashMap<String, OwnedValue>,
) -> Option<Value> {
    let empty_opts: HashMap<&str, ZValue<'_>> = HashMap::new();
    let attrs: HashMap<String, OwnedValue> = conn
        .call_method(
            Some("org.freedesktop.UDisks2"),
            obj_path.as_ref(),
            Some("org.freedesktop.UDisks2.NVMe.Controller"),
            "SmartGetAttributes",
            &(&empty_opts,),
        )
        .ok()
        .and_then(|msg| msg.body().deserialize().ok())
        .unwrap_or_default();

    let warnings = prop_str_list(nvme, "SmartCriticalWarning");
    let critical_warning = if warnings.is_empty() { 0u64 } else { 1u64 };

    let temp_c = prop_u64(nvme, "SmartTemperature")
        .and_then(|k| kelvin_to_celsius(k as f64))
        .or_else(|| {
            attrs
                .get("temp_sensors")
                .and_then(|v| match &**v {
                    ZValue::Array(a) => a.iter().next().and_then(zval_u64),
                    _ => None,
                })
                .and_then(|k| kelvin_to_celsius(k as f64))
        });

    let avail_spare = prop_u64(&attrs, "avail_spare");
    let spare_thresh = prop_u64(&attrs, "spare_thresh");
    let percent_used = prop_u64(&attrs, "percent_used");
    let media_errors = prop_u64(&attrs, "media_errors").unwrap_or(0);
    let num_err_log = prop_u64(&attrs, "num_err_log_entries").unwrap_or(0);
    let power_cycles = prop_u64(&attrs, "power_cycles");
    let unsafe_shutdowns = prop_u64(&attrs, "unsafe_shutdowns");
    let ctrl_busy_time = prop_u64(&attrs, "ctrl_busy_time");
    let power_on_hours =
        prop_u64(nvme, "SmartPowerOnHours").or_else(|| prop_u64(&attrs, "power_on_hours"));
    let data_units_read = prop_u64(&attrs, "total_data_read").map(|b| b / 512_000);
    let data_units_written = prop_u64(&attrs, "total_data_written").map(|b| b / 512_000);

    let has_smart_data = percent_used.is_some()
        || avail_spare.is_some()
        || temp_c.is_some()
        || power_on_hours.is_some();
    if !has_smart_data {
        return None;
    }

    let mut log = serde_json::Map::new();
    log.insert("critical_warning".into(), json!(critical_warning));
    if let Some(t) = temp_c {
        log.insert("temperature".into(), json!(t));
    }
    if let Some(s) = avail_spare {
        log.insert("available_spare".into(), json!(s));
    }
    if let Some(st) = spare_thresh {
        log.insert("available_spare_threshold".into(), json!(st));
    }
    if let Some(pu) = percent_used {
        log.insert("percentage_used".into(), json!(pu));
    }
    if let Some(r) = data_units_read {
        log.insert("data_units_read".into(), json!(r));
    }
    if let Some(w) = data_units_written {
        log.insert("data_units_written".into(), json!(w));
    }
    if let Some(pc) = power_cycles {
        log.insert("power_cycles".into(), json!(pc));
    }
    if let Some(poh) = power_on_hours {
        log.insert("power_on_hours".into(), json!(poh));
    }
    if let Some(us) = unsafe_shutdowns {
        log.insert("unsafe_shutdowns".into(), json!(us));
    }
    if let Some(cbt) = ctrl_busy_time {
        log.insert("controller_busy_time".into(), json!(cbt));
    }
    log.insert("media_errors".into(), json!(media_errors));
    log.insert("num_err_log_entries".into(), json!(num_err_log));

    let passed = critical_warning == 0 && prop_str(nvme, "State").is_none_or(|s| s == "live");
    let selftest_supported = prop_str(nvme, "SmartSelftestStatus").is_some();

    Some(json!({
        "model_name": model,
        "serial_number": serial,
        "firmware_version": firmware,
        "rotation_rate": 0,
        "user_capacity": { "bytes": size_bytes },
        "device": { "name": dev_path, "type": "nvme", "protocol": "NVMe" },
        "smartctl": { "exit_status": 0, "source": "udisks2" },
        "smart_support": { "available": true, "enabled": true },
        "smart_status": { "passed": passed },
        "temperature": { "current": temp_c },
        "power_on_time": { "hours": power_on_hours },
        "power_cycle_count": power_cycles,
        "nvme_optional_admin_commands": { "self_test": selftest_supported },
        "nvme_smart_health_information_log": Value::Object(log),
        "trim": { "supported": true }
    }))
}

#[allow(clippy::too_many_arguments)]
fn build_ata_snapshot(
    conn: &zbus::blocking::Connection,
    obj_path: &OwnedObjectPath,
    dev_path: &str,
    model: &str,
    serial: Option<&str>,
    firmware: Option<&str>,
    size_bytes: u64,
    rotation: Option<i64>,
    bus: &str,
    wwn: Option<&str>,
    ata: &HashMap<String, OwnedValue>,
) -> Option<Value> {
    let empty_opts: HashMap<&str, ZValue<'_>> = HashMap::new();
    let attrs: Vec<AtaAttrRow> = conn
        .call_method(
            Some("org.freedesktop.UDisks2"),
            obj_path.as_ref(),
            Some("org.freedesktop.UDisks2.Drive.Ata"),
            "SmartGetAttributes",
            &(&empty_opts,),
        )
        .ok()
        .and_then(|msg| msg.body().deserialize().ok())
        .unwrap_or_default();

    let mut table = Vec::with_capacity(attrs.len());
    let mut poh_from_attr: Option<u64> = None;
    let mut cycles_from_attr: Option<u64> = None;
    for (id, name, _flags, value, worst, thresh, pretty, pretty_unit, _extra) in attrs {
        let raw_val = ata_pretty_raw(pretty, pretty_unit);
        if id == 9 && raw_val > 0 {
            poh_from_attr = Some(raw_val);
        }
        if id == 12 {
            cycles_from_attr = Some(raw_val);
        }
        let when_failed = if thresh > 0 && value <= thresh {
            "now"
        } else {
            ""
        };
        table.push(json!({
            "id": id,
            "name": name,
            "value": value,
            "worst": worst,
            "thresh": thresh,
            "when_failed": when_failed,
            "raw": { "value": raw_val, "string": raw_val.to_string() }
        }));
    }

    let temp_c = prop_f64(ata, "SmartTemperature").and_then(kelvin_to_celsius);
    let poh = prop_u64(ata, "SmartPowerOnSeconds")
        .map(|s| s / 3600)
        .or(poh_from_attr);
    let failing = prop_bool(ata, "SmartFailing").unwrap_or(false);
    let rot_json = match rotation {
        Some(r) if r >= 0 => json!(r),
        _ => Value::Null,
    };
    let proto = if bus.eq_ignore_ascii_case("usb") {
        "USB"
    } else {
        "ATA"
    };

    Some(json!({
        "model_name": model,
        "serial_number": serial,
        "firmware_version": firmware,
        "wwn": wwn,
        "rotation_rate": rot_json,
        "user_capacity": { "bytes": size_bytes },
        "device": { "name": dev_path, "type": "sat", "protocol": proto },
        "smartctl": { "exit_status": 0, "source": "udisks2" },
        "smart_support": { "available": true, "enabled": prop_bool(ata, "SmartEnabled").unwrap_or(true) },
        "smart_status": { "passed": !failing },
        "temperature": { "current": temp_c },
        "power_on_time": { "hours": poh },
        "power_cycle_count": cycles_from_attr,
        "ata_smart_data": {
            "capabilities": {
                "self_tests_supported": prop_str(ata, "SmartSelftestStatus").is_some()
            }
        },
        "ata_smart_attributes": { "table": table }
    }))
}
