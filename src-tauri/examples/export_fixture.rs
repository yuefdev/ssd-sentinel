use serde_json::json;
use ssd_sentinel_core::core::{Config, Core};
use std::path::PathBuf;

fn main() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let data = tempfile::tempdir().unwrap();
    let core = Core::new(Config {
        smartctl: "unused.exe".into(),
        data_dir: data.path().into(),
        snapshot_dir: Some(project.join("src-tauri/tests/fixtures")),
    })
    .unwrap();
    core.refresh_now().unwrap();
    let mut state = core.state();
    state["admin"] = json!(false);
    state["language"] = json!("tr");
    let mut histories = json!({});
    for disk in state["disks"].as_array().unwrap() {
        let id = disk["id"].as_str().unwrap();
        histories[id] = core.history(id).unwrap();
    }
    let target = std::env::args().nth(1).expect("output path");
    let output = PathBuf::from(target);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&json!({"state":state,"histories":histories,"events":[]}))
            .unwrap(),
    )
    .unwrap();
}
