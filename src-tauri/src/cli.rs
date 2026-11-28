use crate::{
    core::{Config, Core, JobRequest},
    dev_workspace, platform, report, tiering,
};
use serde_json::{json, Value};
use std::{fs, path::PathBuf};

fn default_cli_data_dir(snapshot_dir: Option<&PathBuf>) -> PathBuf {
    if snapshot_dir.is_some() {
        return std::env::temp_dir()
            .join(format!("ssd-sentinel-cli-replay-{}", std::process::id()));
    }
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join("ssd-sentinel")
}

fn build_core(
    snapshot_dir: Option<PathBuf>,
) -> Result<(std::sync::Arc<Core>, Option<PathBuf>), String> {
    let cleanup = snapshot_dir
        .as_ref()
        .map(|_| default_cli_data_dir(snapshot_dir.as_ref()));
    let data_dir = cleanup
        .clone()
        .unwrap_or_else(|| default_cli_data_dir(None));
    fs::create_dir_all(&data_dir).map_err(|e| format!("data_dir: {e}"))?;
    let core = Core::new(Config {
        smartctl: platform::discover_smartctl(),
        data_dir,
        snapshot_dir,
    })?;
    let _ = core.refresh_now();
    Ok((core, cleanup))
}

fn find_flag_value(args: &[String], flag: &str) -> Result<Option<String>, String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            let val = args
                .get(i + 1)
                .ok_or_else(|| format!("{flag} requires an argument"))?;
            return Ok(Some(val.clone()));
        }
        i += 1;
    }
    Ok(None)
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn positional_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if matches!(args[i].as_str(), "--snapshot-dir" | "--size-mib") {
            i += 2;
            continue;
        }
        if args[i].starts_with("--") {
            i += 1;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

pub fn print_help() {
    println!(
        "SSD Sentinel {} — Storage Diagnostics, Benchmark, Surface Repair & Data Recovery

USAGE:
    ssd-sentinel [GUI_OPTIONS]
    ssd-sentinel --cli <SUBCOMMAND> [OPTIONS]

GLOBAL FLAGS:
    -h, --help                Print help information
    -V, --version             Print version
    --cli <SUBCOMMAND>        Run headless CLI without opening GUI
    --privileged-helper       Internal Polkit stdio JSON-RPC helper

GUI OPTIONS:
    --snapshot-dir <DIR>      Open in read-only replay mode using fixture directory
    --data-dir <DIR>          Override SQLite/data directory
    --smartctl <PATH>         Override smartctl binary path
    --portable                Store settings & DB in ./data beside executable
    --minimized               Start hidden if a system tray watcher is available

CLI SUBCOMMANDS:
    status [--json] [--snapshot-dir <DIR>]
    perf   [--json]
    smart  <DISK_ID> [--json] [--snapshot-dir <DIR>]
    bench  <MOUNT_OR_LETTER> [--size-mib <N>] [--confirm] [--json]
    dev    <PATH> [--json]
    tier   <DISK_ID> <MOUNT_OR_LETTER> [--json]
    report <OUT_HTML_OR_JSON> [--redact] [--snapshot-dir <DIR>]",
        env!("CARGO_PKG_VERSION")
    );
}

pub fn run(args: &[String]) -> i32 {
    if args.is_empty() || has_flag(args, "--help") || has_flag(args, "-h") {
        print_help();
        return 0;
    }

    let json_mode = has_flag(args, "--json");
    let redact = has_flag(args, "--redact");
    let confirm = has_flag(args, "--confirm");
    let snap = match find_flag_value(args, "--snapshot-dir") {
        Ok(s) => s.map(PathBuf::from),
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let size_mib: u64 = match find_flag_value(args, "--size-mib") {
        Ok(Some(v)) => match v.parse() {
            Ok(n) if matches!(n, 256 | 1024 | 4096) => n,
            Ok(_) => {
                eprintln!("--size-mib must be 256, 1024, or 4096");
                return 2;
            }
            Err(_) => {
                eprintln!("Invalid --size-mib value: {v}");
                return 2;
            }
        },
        Ok(None) => 256,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    let pos = positional_args(args);
    let Some(sub) = pos.first().map(String::as_str) else {
        print_help();
        return 2;
    };

    let mut code = 0;
    let mut temp_cleanup: Option<PathBuf> = None;

    match sub {
        "perf" => {
            let mut prev = Value::Null;
            match platform::performance(&mut prev, &std::sync::atomic::AtomicBool::new(false)) {
                Ok(v) => {
                    if json_mode {
                        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    } else {
                        for d in v["disks"].as_array().into_iter().flatten() {
                            println!(
                                "{}: read {:.1} MB/s · write {:.1} MB/s · IOPS {:.0} · queue {}",
                                d["device"].as_str().unwrap_or("—"),
                                d["read_mb_s"].as_f64().unwrap_or(0.0),
                                d["write_mb_s"].as_f64().unwrap_or(0.0),
                                d["iops"].as_f64().unwrap_or(0.0),
                                d["queue_depth"].as_u64().unwrap_or(0),
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("perf failed: {e}");
                    code = 1;
                }
            }
        }
        "dev" => {
            let target = pos.get(1).map(String::as_str).unwrap_or(".");
            let root = PathBuf::from(target);
            if !root.exists() {
                eprintln!("Path does not exist: {target}");
                code = 1;
            } else {
                let projs = dev_workspace::scan_directory_projects(&root);
                let total_bytes: u64 = projs.iter().map(|p| p.artifact_bytes).sum();
                let out = json!({
                    "root": target,
                    "projects": projs.len(),
                    "reclaimable_bytes": total_bytes,
                    "items": projs,
                });
                if json_mode {
                    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
                } else {
                    println!(
                        "{}: {} project(s), {} reclaimable bytes",
                        target, out["projects"], out["reclaimable_bytes"]
                    );
                }
            }
        }
        "status" | "smart" | "report" | "tier" | "bench" => {
            let (core, cleanup) = match build_core(snap) {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            temp_cleanup = cleanup;
            let state = core.state();

            match sub {
                "status" => {
                    if json_mode {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&state).unwrap_or_default()
                        );
                    } else {
                        for d in state["disks"].as_array().into_iter().flatten() {
                            let pct = d["health"]["percent"]
                                .as_f64()
                                .map_or_else(|| "—".into(), |p| format!("{p:.0}%"));
                            let temp = d["metrics"]["temperature_c"]
                                .as_f64()
                                .map_or_else(|| "—".into(), |t| format!("{t:.0}°C"));
                            println!(
                                "[{}] {} ({}) · health {} · temp {} · dev {}",
                                d["id"].as_str().unwrap_or(""),
                                d["model"].as_str().unwrap_or(""),
                                d["type"].as_str().unwrap_or(""),
                                pct,
                                temp,
                                d["device"].as_str().unwrap_or(""),
                            );
                        }
                    }
                }
                "smart" => {
                    let Some(id) = pos.get(1) else {
                        eprintln!("Usage: --cli smart <DISK_ID>");
                        return 2;
                    };
                    let found = state["disks"].as_array().and_then(|arr| {
                        arr.iter().find(|d| {
                            d["id"].as_str() == Some(id.as_str())
                                || d["device"].as_str() == Some(id.as_str())
                        })
                    });
                    match found {
                        Some(d) => {
                            if json_mode {
                                println!("{}", serde_json::to_string_pretty(d).unwrap_or_default());
                            } else {
                                println!(
                                    "{} ({})",
                                    d["model"].as_str().unwrap_or(""),
                                    d["device"].as_str().unwrap_or("")
                                );
                                for r in d["smart"].as_array().into_iter().flatten() {
                                    println!(
                                        "  {:>4} {:<32} raw={:<16} val={}",
                                        r["id"].as_str().unwrap_or(""),
                                        r["original"]
                                            .as_str()
                                            .or_else(|| r["name"].as_str())
                                            .unwrap_or(""),
                                        r["raw"].as_str().unwrap_or(""),
                                        r["value"]
                                    );
                                }
                            }
                        }
                        None => {
                            eprintln!("Disk not found: {id}");
                            code = 1;
                        }
                    }
                }
                "report" => {
                    let Some(out_path) = pos.get(1) else {
                        eprintln!("Usage: --cli report <OUT_PATH.html|json> [--redact]");
                        return 2;
                    };
                    let content = if out_path.ends_with(".json") {
                        let mut s = state.clone();
                        if redact {
                            report::redact_value(&mut s);
                        }
                        serde_json::to_string_pretty(&s).unwrap_or_default()
                    } else {
                        report::render_html_report("snapshot", &state, None, redact)
                    };
                    if let Err(e) = fs::write(out_path, content) {
                        eprintln!("Failed to write {out_path}: {e}");
                        code = 1;
                    } else {
                        println!("Report written to {out_path}");
                    }
                }
                "tier" => {
                    let (Some(disk_id), Some(mount)) = (pos.get(1), pos.get(2)) else {
                        eprintln!("Usage: --cli tier <DISK_ID> <MOUNT>");
                        return 2;
                    };
                    match core.analyze_storage_tiering(disk_id, mount) {
                        Ok(v) => {
                            if json_mode {
                                println!(
                                    "{}",
                                    serde_json::to_string_pretty(&v).unwrap_or_default()
                                );
                            } else {
                                println!(
                                    "{}",
                                    v["summary_tr"]
                                        .as_str()
                                        .or_else(|| v["summary"].as_str())
                                        .unwrap_or("OK")
                                );
                                println!("Reclaimable: {} bytes", v["total_reclaimable_bytes"]);
                            }
                        }
                        Err(e) => {
                            let _ = tiering::is_forbidden_path(std::path::Path::new(mount));
                            eprintln!("tier analysis failed: {e}");
                            code = 1;
                        }
                    }
                }
                "bench" => {
                    let Some(mount) = pos.get(1) else {
                        eprintln!("Usage: --cli bench <MOUNT> [--size-mib <N>] [--confirm]");
                        return 2;
                    };
                    if !confirm {
                        eprintln!("Safety lock S1 active: pass --confirm to write temporary benchmark file on {mount}");
                        return 2;
                    }
                    let disk_id = state["disks"]
                        .as_array()
                        .and_then(|ds| {
                            ds.iter().find(|d| {
                                d["volumes"].as_array().is_some_and(|vs| {
                                    vs.iter()
                                        .any(|v| v["letter"].as_str() == Some(mount.as_str()))
                                })
                            })
                        })
                        .and_then(|d| d["id"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    if disk_id.is_empty() {
                        eprintln!("No disk owns mount {mount}");
                        return 1;
                    }
                    let _ = core.set_preferences(false, None, None, Some(false));
                    let conf = format!("benchmark:{disk_id}:{mount}");
                    match core.start_job(JobRequest {
                        kind: "benchmark".into(),
                        disk_id,
                        letter: mount.clone(),
                        options: json!({"profile": "standard", "mode": "read", "size_mib": size_mib}),
                        confirmation: conf,
                    }) {
                        Ok(_) => {
                            loop {
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                let j = core.state()["job"].clone();
                                if !matches!(j["status"].as_str(), Some("running" | "cancelling")) {
                                    println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("bench failed: {e}");
                            code = 1;
                        }
                    }
                }
                _ => {}
            }
            core.shutdown();
        }
        other => {
            eprintln!("Unknown CLI subcommand: {other}");
            print_help();
            code = 2;
        }
    }

    if let Some(dir) = temp_cleanup {
        let _ = fs::remove_dir_all(dir);
    }
    code
}
