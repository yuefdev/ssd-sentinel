use crate::model::Disk;
use chrono::{Duration, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{path::Path, time::Duration as StdDuration};

pub struct Store {
    db: Connection,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn decimal_greater(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() || !a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit()) {
        return false;
    }
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len() > b.len() || (a.len() == b.len() && a > b)
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(err)?;
        }
        let db = Connection::open(path).map_err(err)?;
        db.busy_timeout(StdDuration::from_secs(3)).map_err(err)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS samples (id INTEGER PRIMARY KEY,disk_id TEXT NOT NULL,source TEXT NOT NULL,time TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS sample_identity ON samples(disk_id,source,id);
            CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY,disk_id TEXT NOT NULL,source TEXT NOT NULL,time TEXT NOT NULL,code TEXT NOT NULL,level TEXT NOT NULL,message TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS benchmarks (sequence INTEGER PRIMARY KEY,job_id TEXT UNIQUE NOT NULL,disk_id TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS surface_scans (sequence INTEGER PRIMARY KEY,job_id TEXT UNIQUE NOT NULL,disk_id TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS defrag_jobs (sequence INTEGER PRIMARY KEY,job_id TEXT UNIQUE NOT NULL,disk_id TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS autopilot_jobs (sequence INTEGER PRIMARY KEY,job_id TEXT UNIQUE NOT NULL,disk_id TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS recovery_jobs (sequence INTEGER PRIMARY KEY,job_id TEXT UNIQUE NOT NULL,disk_id TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS tier_jobs (sequence INTEGER PRIMARY KEY,job_id TEXT UNIQUE NOT NULL,disk_id TEXT NOT NULL,payload TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS optimizations (id INTEGER PRIMARY KEY,key TEXT UNIQUE NOT NULL,target TEXT NOT NULL,title TEXT NOT NULL,previous_value TEXT NOT NULL,applied_value TEXT NOT NULL,updated_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS surface_checkpoints (disk_id TEXT PRIMARY KEY,payload TEXT NOT NULL,updated_at TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS tier_moves (id TEXT PRIMARY KEY,source_path TEXT NOT NULL,target_path TEXT NOT NULL,source_letter TEXT NOT NULL,target_letter TEXT NOT NULL,is_dir INTEGER NOT NULL,junction_created INTEGER NOT NULL,size_bytes INTEGER NOT NULL,file_count INTEGER NOT NULL,direction TEXT NOT NULL,moved_at TEXT NOT NULL);").map_err(err)?;
        Ok(Self { db })
    }

    pub fn record(&mut self, disks: &[Disk], source: &str) -> Result<(), String> {
        let tx = self.db.transaction().map_err(err)?;
        for disk in disks {
            let time = if source.starts_with("replay:") {
                let Some(time) = disk
                    .source_time
                    .and_then(|s| Utc.timestamp_opt(s, 0).single())
                else {
                    continue;
                };
                time.to_rfc3339()
            } else {
                Utc::now().to_rfc3339()
            };
            let counters: serde_json::Map<String, Value> = disk
                .smart
                .iter()
                .filter(|r| {
                    matches!(
                        r.id.as_str(),
                        "5" | "10"
                            | "184"
                            | "187"
                            | "196"
                            | "197"
                            | "198"
                            | "199"
                            | "media_errors"
                            | "num_err_log_entries"
                            | "unsafe_shutdowns"
                    )
                })
                .map(|r| (r.id.clone(), json!(r.raw)))
                .collect();
            let mut payload = disk.metrics.clone();
            payload["health_percent"] = json!(disk.health.percent);
            payload["counters"] = json!(counters);
            let previous = tx.query_row("SELECT time,payload FROM samples WHERE disk_id=? AND source=? ORDER BY id DESC LIMIT 1", params![disk.id, source], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
            match previous {
                Ok((old_time, old)) => {
                    if source.starts_with("replay:") && old_time == time {
                        continue;
                    }
                    let old: Value = serde_json::from_str(&old).map_err(err)?;
                    let mut changes = Vec::new();
                    if let (Some(a), Some(b)) =
                        (disk.health.percent, old["health_percent"].as_u64())
                    {
                        if u64::from(a) < b {
                            changes.push((
                                "health-drop".to_owned(),
                                "warning",
                                format!("Sağlık / dayanım %{b} → %{a} düştü."),
                            ));
                        }
                    }
                    for (key, v) in &counters {
                        if let (Some(a), Some(b)) = (v.as_str(), old["counters"][key].as_str()) {
                            if decimal_greater(a, b) {
                                let level = if matches!(
                                    key.as_str(),
                                    "199" | "num_err_log_entries" | "unsafe_shutdowns"
                                ) {
                                    "info"
                                } else {
                                    "warning"
                                };
                                changes.push((
                                    format!("counter:{key}"),
                                    level,
                                    format!("{key} sayacı {b} → {a} arttı."),
                                ));
                            }
                        }
                    }
                    for (code, level, message) in changes {
                        tx.execute("INSERT INTO events(disk_id,source,time,code,level,message) VALUES(?,?,?,?,?,?)", params![disk.id, source, time, code, level, message]).map_err(err)?;
                    }
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => {}
                Err(e) => return Err(err(e)),
            }
            tx.execute(
                "INSERT INTO samples(disk_id,source,time,payload) VALUES(?,?,?,?)",
                params![disk.id, source, time, payload.to_string()],
            )
            .map_err(err)?;
        }
        let cutoff = (Utc::now() - Duration::days(90)).to_rfc3339();
        tx.execute("DELETE FROM samples WHERE time < ?", [&cutoff])
            .map_err(err)?;
        tx.execute("DELETE FROM events WHERE time < ?", [&cutoff])
            .map_err(err)?;
        tx.commit().map_err(err)
    }

    pub fn history(&self, id: &str, source: &str) -> Result<Value, String> {
        let mut stmt = self.db.prepare("SELECT time,payload FROM samples WHERE disk_id=? AND source=? ORDER BY id DESC LIMIT 720").map_err(err)?;
        let rows = stmt
            .query_map(params![id, source], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(err)?;
        let mut result = Vec::new();
        for row in rows {
            let (time, text) = row.map_err(err)?;
            let mut value: Value = serde_json::from_str(&text).map_err(err)?;
            value["time"] = json!(time);
            result.push(value);
        }
        result.reverse();
        Ok(json!(result))
    }

    pub fn events(&self, id: Option<&str>, source: &str) -> Result<Value, String> {
        let mut stmt = self.db.prepare("SELECT id,disk_id,time,code,level,message FROM events WHERE source=? AND (? IS NULL OR disk_id=?) ORDER BY id DESC LIMIT 200").map_err(err)?;
        let rows = stmt.query_map(params![source, id, id], |r| Ok(json!({"id":r.get::<_, i64>(0)?, "disk_id":r.get::<_, String>(1)?, "source":source, "time":r.get::<_, String>(2)?, "code":r.get::<_, String>(3)?, "level":r.get::<_, String>(4)?, "message":r.get::<_, String>(5)?}))).map_err(err)?;
        Ok(json!(rows.collect::<Result<Vec<_>, _>>().map_err(err)?))
    }

    fn table(kind: &str) -> Result<&'static str, String> {
        match kind {
            "benchmark" => Ok("benchmarks"),
            "surface" => Ok("surface_scans"),
            "defrag" => Ok("defrag_jobs"),
            "autopilot" => Ok("autopilot_jobs"),
            "recovery" => Ok("recovery_jobs"),
            "smart_tier" => Ok("tier_jobs"),
            _ => Err("Geçersiz iş türü.".into()),
        }
    }

    pub fn save_job(&mut self, kind: &str, job: &Value) -> Result<(), String> {
        let table = Self::table(kind)?;
        let id = job["id"].as_str().ok_or("İş kimliği eksik.")?;
        let disk = job["disk_id"].as_str().ok_or("Disk kimliği eksik.")?;
        let tx = self.db.transaction().map_err(err)?;
        tx.execute(
            &format!("INSERT OR REPLACE INTO {table}(job_id,disk_id,payload) VALUES(?,?,?)"),
            params![id, disk, job.to_string()],
        )
        .map_err(err)?;
        tx.execute(&format!("DELETE FROM {table} WHERE sequence NOT IN(SELECT sequence FROM {table} ORDER BY sequence DESC LIMIT 20)"), []).map_err(err)?;
        tx.commit().map_err(err)
    }

    pub fn jobs(&self, kind: &str, id: Option<&str>) -> Result<Value, String> {
        let table = Self::table(kind)?;
        let mut stmt = self.db.prepare(&format!("SELECT payload FROM {table} WHERE (? IS NULL OR disk_id=?) ORDER BY sequence DESC LIMIT 20")).map_err(err)?;
        let rows = stmt
            .query_map(params![id, id], |r| r.get::<_, String>(0))
            .map_err(err)?;
        let mut result = Vec::new();
        for row in rows {
            let mut job = serde_json::from_str::<Value>(&row.map_err(err)?).map_err(err)?;
            job["kind"] = json!(kind);
            result.push(job);
        }
        Ok(json!(result))
    }

    pub fn record_optimization(
        &mut self,
        key: &str,
        target: &str,
        title: &str,
        previous: &str,
        applied: &str,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        self.db
            .execute(
                "INSERT INTO optimizations(key,target,title,previous_value,applied_value,updated_at) VALUES(?,?,?,?,?,?)
                 ON CONFLICT(key) DO UPDATE SET target=excluded.target, title=excluded.title, applied_value=excluded.applied_value, updated_at=excluded.updated_at",
                params![key, target, title, previous, applied, now],
            )
            .map_err(err)?;
        Ok(())
    }

    pub fn get_optimization(&self, key: &str) -> Result<Option<Value>, String> {
        let row = self.db.query_row(
            "SELECT key,target,title,previous_value,applied_value,updated_at FROM optimizations WHERE key=?",
            params![key],
            |r| {
                Ok(json!({
                    "key": r.get::<_, String>(0)?,
                    "target": r.get::<_, String>(1)?,
                    "title": r.get::<_, String>(2)?,
                    "previous_value": r.get::<_, String>(3)?,
                    "applied_value": r.get::<_, String>(4)?,
                    "updated_at": r.get::<_, String>(5)?
                }))
            },
        );
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    pub fn remove_optimization(&mut self, key: &str) -> Result<(), String> {
        self.db
            .execute("DELETE FROM optimizations WHERE key=?", params![key])
            .map_err(err)?;
        Ok(())
    }

    pub fn list_optimizations(&self) -> Result<Value, String> {
        let mut stmt = self.db.prepare("SELECT key,target,title,previous_value,applied_value,updated_at FROM optimizations ORDER BY id DESC").map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({
                    "key": r.get::<_, String>(0)?,
                    "target": r.get::<_, String>(1)?,
                    "title": r.get::<_, String>(2)?,
                    "previous_value": r.get::<_, String>(3)?,
                    "applied_value": r.get::<_, String>(4)?,
                    "updated_at": r.get::<_, String>(5)?
                }))
            })
            .map_err(err)?;
        Ok(json!(rows.collect::<Result<Vec<_>, _>>().map_err(err)?))
    }

    pub fn save_surface_checkpoint(
        &mut self,
        disk_id: &str,
        payload: &Value,
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        self.db
            .execute(
                "INSERT INTO surface_checkpoints(disk_id,payload,updated_at) VALUES(?,?,?)
                 ON CONFLICT(disk_id) DO UPDATE SET payload=excluded.payload, updated_at=excluded.updated_at",
                params![disk_id, payload.to_string(), now],
            )
            .map_err(err)?;
        Ok(())
    }

    pub fn get_surface_checkpoint(&self, disk_id: &str) -> Result<Option<Value>, String> {
        let row = self.db.query_row(
            "SELECT payload,updated_at FROM surface_checkpoints WHERE disk_id=?",
            params![disk_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        );
        match row {
            Ok((text, updated_at)) => {
                let mut value: Value = serde_json::from_str(&text).map_err(err)?;
                if value.is_object() {
                    value["disk_id"] = json!(disk_id);
                    value["updated_at"] = json!(updated_at);
                }
                Ok(Some(value))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    pub fn clear_surface_checkpoint(&mut self, disk_id: &str) -> Result<(), String> {
        self.db
            .execute(
                "DELETE FROM surface_checkpoints WHERE disk_id=?",
                params![disk_id],
            )
            .map_err(err)?;
        Ok(())
    }

    pub fn record_tier_move(&mut self, rec: &Value) -> Result<(), String> {
        let id = rec["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Taşıma kaydı kimliği eksik.")?;
        let source_path = rec["source_path"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Kaynak yolu eksik.")?;
        let target_path = rec["target_path"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Hedef yolu eksik.")?;
        let source_letter = rec["source_letter"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Kaynak bölüm harfi eksik.")?;
        let target_letter = rec["target_letter"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("Hedef bölüm harfi eksik.")?;
        let is_dir = i64::from(rec["is_dir"].as_bool().unwrap_or(false));
        let junction_created = i64::from(rec["junction_created"].as_bool().unwrap_or(false));
        let size_bytes = rec["size_bytes"].as_u64().unwrap_or(0).min(i64::MAX as u64) as i64;
        let file_count = rec["file_count"].as_u64().unwrap_or(0).min(i64::MAX as u64) as i64;
        let direction = rec["direction"]
            .as_str()
            .filter(|s| matches!(*s, "to_hdd" | "to_ssd" | "to_archive"))
            .ok_or("Geçersiz katmanlama yönü.")?;
        let moved_at = rec["moved_at"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        self.db
            .execute(
                "INSERT OR REPLACE INTO tier_moves(id,source_path,target_path,source_letter,target_letter,is_dir,junction_created,size_bytes,file_count,direction,moved_at)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    id,
                    source_path,
                    target_path,
                    source_letter,
                    target_letter,
                    is_dir,
                    junction_created,
                    size_bytes,
                    file_count,
                    direction,
                    moved_at
                ],
            )
            .map_err(err)?;
        Ok(())
    }

    fn tier_move_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
        Ok(json!({
            "id": r.get::<_, String>(0)?,
            "source_path": r.get::<_, String>(1)?,
            "target_path": r.get::<_, String>(2)?,
            "source_letter": r.get::<_, String>(3)?,
            "target_letter": r.get::<_, String>(4)?,
            "is_dir": r.get::<_, i64>(5)? != 0,
            "junction_created": r.get::<_, i64>(6)? != 0,
            "size_bytes": r.get::<_, i64>(7)?.max(0) as u64,
            "file_count": r.get::<_, i64>(8)?.max(0) as u64,
            "direction": r.get::<_, String>(9)?,
            "moved_at": r.get::<_, String>(10)?
        }))
    }

    pub fn list_tier_moves(&self) -> Result<Value, String> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT id,source_path,target_path,source_letter,target_letter,is_dir,junction_created,size_bytes,file_count,direction,moved_at
                 FROM tier_moves ORDER BY moved_at DESC, rowid DESC",
            )
            .map_err(err)?;
        let rows = stmt.query_map([], Self::tier_move_from_row).map_err(err)?;
        Ok(json!(rows.collect::<Result<Vec<_>, _>>().map_err(err)?))
    }

    pub fn get_tier_move(&self, id: &str) -> Result<Option<Value>, String> {
        let row = self.db.query_row(
            "SELECT id,source_path,target_path,source_letter,target_letter,is_dir,junction_created,size_bytes,file_count,direction,moved_at
             FROM tier_moves WHERE id=?",
            params![id],
            Self::tier_move_from_row,
        );
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    pub fn delete_tier_move(&mut self, id: &str) -> Result<(), String> {
        self.db
            .execute("DELETE FROM tier_moves WHERE id=?", params![id])
            .map_err(err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_checkpoint_save_load_overwrite_and_clear_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cp.sqlite3");
        {
            let mut store = Store::open(&db_path).unwrap();
            assert!(store.get_surface_checkpoint("disk-a").unwrap().is_none());
            store
                .save_surface_checkpoint(
                    "disk-a",
                    &json!({
                        "disk_id": "disk-a",
                        "disk_bytes": 500_000_000_000u64,
                        "next_zone": 15,
                        "total_zones": 200
                    }),
                )
                .unwrap();
            let got = store.get_surface_checkpoint("disk-a").unwrap().unwrap();
            assert_eq!(got["next_zone"], 15);
            assert!(got["updated_at"].as_str().is_some_and(|s| !s.is_empty()));

            store
                .save_surface_checkpoint(
                    "disk-a",
                    &json!({
                        "disk_id": "disk-a",
                        "disk_bytes": 500_000_000_000u64,
                        "next_zone": 35,
                        "total_zones": 200
                    }),
                )
                .unwrap();
        }
        // Reopen database and verify persistence and clearing.
        let mut reopened = Store::open(&db_path).unwrap();
        let got = reopened.get_surface_checkpoint("disk-a").unwrap().unwrap();
        assert_eq!(got["next_zone"], 35);
        reopened.clear_surface_checkpoint("disk-a").unwrap();
        assert!(reopened.get_surface_checkpoint("disk-a").unwrap().is_none());
    }

    #[test]
    fn tier_moves_and_tier_jobs_sqlite_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("tier.sqlite3");
        {
            let mut store = Store::open(&db_path).unwrap();
            assert_eq!(store.list_tier_moves().unwrap(), json!([]));
            assert!(store.get_tier_move("move-1").unwrap().is_none());

            let rec = json!({
                "id": "move-1",
                "source_path": "C:\\Users\\demo\\Videos\\Tatil",
                "target_path": "D:\\SSD-Sentinel-Arsiv\\C\\Users\\demo\\Videos\\Tatil",
                "source_letter": "C",
                "target_letter": "D",
                "is_dir": true,
                "junction_created": true,
                "size_bytes": 1_500_000_000u64,
                "file_count": 12u64,
                "direction": "to_hdd",
                "moved_at": "2026-11-21T10:00:00Z"
            });
            store.record_tier_move(&rec).unwrap();
            let got = store.get_tier_move("move-1").unwrap().unwrap();
            assert_eq!(got["source_path"], rec["source_path"]);
            assert_eq!(got["is_dir"], true);
            assert_eq!(got["junction_created"], true);
            assert_eq!(got["size_bytes"], 1_500_000_000u64);
            assert_eq!(got["file_count"], 12u64);
            assert_eq!(got["direction"], "to_hdd");

            store
                .save_job(
                    "smart_tier",
                    &json!({
                        "id": "tier-job-1",
                        "disk_id": "nvme-1",
                        "status": "completed",
                        "repair_summary": {
                            "mode": "ssd_extent_optimize",
                            "files_consolidated": 3,
                            "extents_before": 210,
                            "extents_after": 6,
                            "bytes_written": 120_000_000u64,
                            "retrim_ok": true,
                            "scope": "test"
                        }
                    }),
                )
                .unwrap();
        }
        let mut reopened = Store::open(&db_path).unwrap();
        let list = reopened.list_tier_moves().unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], "move-1");

        let jobs = reopened.jobs("smart_tier", Some("nvme-1")).unwrap();
        assert_eq!(jobs.as_array().unwrap().len(), 1);
        assert_eq!(jobs[0]["kind"], "smart_tier");
        assert_eq!(jobs[0]["repair_summary"]["mode"], "ssd_extent_optimize");

        reopened.delete_tier_move("move-1").unwrap();
        assert!(reopened.get_tier_move("move-1").unwrap().is_none());
        assert_eq!(reopened.list_tier_moves().unwrap(), json!([]));
    }
}
