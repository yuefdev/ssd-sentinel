use crate::{
    jobs,
    model::{self, normalise, Disk},
    optimize::OptimizeRequest,
    platform,
    store::Store,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, MutexGuard,
    },
    thread::JoinHandle,
    time::Duration,
};

pub struct Config {
    pub smartctl: PathBuf,
    pub data_dir: PathBuf,
    pub snapshot_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobRequest {
    pub kind: String,
    pub disk_id: String,
    pub letter: String,
    pub confirmation: String,
    pub options: Value,
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Permit(Arc<(Mutex<bool>, Condvar)>);
impl Drop for Permit {
    fn drop(&mut self) {
        *lock(&self.0 .0) = false;
        self.0 .1.notify_all();
    }
}

pub struct Core {
    pub config: Config,
    pub store: Mutex<Store>,
    pub data: Mutex<Value>,
    pub snapshots: Mutex<HashMap<String, Value>>,
    pub job: Mutex<Value>,
    pub stop: AtomicBool,
    pub job_cancel: AtomicBool,
    pub minimise_to_tray: AtomicBool,
    pub strict_health: AtomicBool,
    pub read_only_guard: AtomicBool,
    pub language: Mutex<String>,
    busy: Arc<(Mutex<bool>, Condvar)>,
    source_key: String,
    wake: (Mutex<bool>, Condvar),
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl Core {
    pub fn new(config: Config) -> Result<Arc<Self>, String> {
        let store = Store::open(&config.data_dir.join("history.sqlite3"))?;
        let source = if config.snapshot_dir.is_some() {
            "replay"
        } else {
            "live"
        };
        let source_key = if let Some(path) = &config.snapshot_dir {
            let path = path.canonicalize().map_err(|e| e.to_string())?;
            let path = path
                .to_string_lossy()
                .trim_start_matches("\\\\?\\")
                .to_owned();
            format!(
                "replay:{:.12}",
                format!("{:x}", Sha256::digest(path.as_bytes()))
            )
        } else {
            "live".into()
        };
        let prefs: Value = std::fs::read(config.data_dir.join("preferences.json"))
            .ok()
            .and_then(|s| serde_json::from_slice(&s).ok())
            .unwrap_or(Value::Null);
        let minimise = prefs["minimise_to_tray"].as_bool().unwrap_or(false);
        let strict = prefs["health_mode"].as_str() == Some("strict");
        let read_only_guard = prefs["read_only_guard"].as_bool().unwrap_or(true);
        let lang = match prefs["language"].as_str() {
            Some("en") => "en".to_owned(),
            _ => "tr".to_owned(),
        };
        let data = json!({"source":source,"admin":platform::is_admin(),"updating":false,"stale":true,"updated_at":null,"error":null,"partial_warnings":[],"disks":[],"windows":{},"performance":{"disks":[],"error":null},"refresh_seconds":60});
        Ok(Arc::new(Self {
            config,
            store: Mutex::new(store),
            data: Mutex::new(data),
            snapshots: Mutex::new(HashMap::new()),
            job: Mutex::new(Value::Null),
            stop: AtomicBool::new(false),
            job_cancel: AtomicBool::new(false),
            minimise_to_tray: AtomicBool::new(minimise),
            strict_health: AtomicBool::new(strict),
            read_only_guard: AtomicBool::new(read_only_guard),
            language: Mutex::new(lang),
            busy: Arc::new((Mutex::new(false), Condvar::new())),
            source_key,
            wake: (Mutex::new(false), Condvar::new()),
            workers: Mutex::new(Vec::new()),
        }))
    }

    pub fn reserve(&self) -> Result<Permit, String> {
        let mut busy = lock(&self.busy.0);
        if self.stop.load(Ordering::SeqCst) {
            return Err("Uygulama kapanıyor.".into());
        }
        if *busy {
            return Err("Başka bir disk işlemi / okuması sürüyor.".into());
        }
        *busy = true;
        Ok(Permit(self.busy.clone()))
    }

    pub fn state(&self) -> Value {
        let mut data = lock(&self.data).clone();
        data["version"] = json!(env!("CARGO_PKG_VERSION"));
        data["backend"] = json!("Rust · Tauri 2");
        data["job"] = lock(&self.job).clone();
        data["engine"] = jobs::engine_info();
        data["profiles"] = json!(jobs::profiles());
        data["minimise_to_tray"] = json!(self.minimise_to_tray.load(Ordering::SeqCst));
        data["health_mode"] = json!(if self.strict_health.load(Ordering::SeqCst) {
            "strict"
        } else {
            "default"
        });
        data["read_only_guard"] = json!(self.read_only_guard.load(Ordering::SeqCst));
        data["language"] = json!(lock(&self.language).clone());
        data["optimizations"] = lock(&self.store)
            .list_optimizations()
            .unwrap_or_else(|_| json!([]));
        data["tier_moves"] = lock(&self.store)
            .list_tier_moves()
            .unwrap_or_else(|_| json!([]));
        data["busy"] = json!(*lock(&self.busy.0));
        data
    }

    pub fn disks(&self) -> Result<Vec<Disk>, String> {
        serde_json::from_value(lock(&self.data)["disks"].clone()).map_err(|e| e.to_string())
    }

    pub fn refresh_now(&self) -> Result<(), String> {
        let _permit = self.reserve()?;
        lock(&self.data)["updating"] = json!(true);
        let result = self.collect();
        let mut data = lock(&self.data);
        data["updating"] = json!(false);
        if let Err(error) = &result {
            data["stale"] = json!(true);
            data["error"] = json!(error);
        }
        result
    }

    fn collect(&self) -> Result<(), String> {
        let mut snapshots = Vec::new();
        let mut skipped_warnings: Vec<String> = Vec::new();
        let inventory;
        if let Some(path) = &self.config.snapshot_dir {
            let mut paths = std::fs::read_dir(path)
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|s| s.to_string_lossy().starts_with("smart-"))
                        && p.extension().is_some_and(|s| s == "json")
                })
                .collect::<Vec<_>>();
            paths.sort();
            if paths.is_empty() || paths.len() > 256 {
                return Err("Klasörde 1–256 smart-*.json dosyası olmalı.".into());
            }
            for path in paths {
                let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
                if size > 8 * 1024 * 1024 {
                    return Err("SMART kayıt dosyası 8 MiB sınırını aşıyor.".into());
                }
                snapshots.push(
                    serde_json::from_slice::<Value>(
                        &std::fs::read(path).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?,
                );
            }
            inventory = json!({"disks":[],"trim_enabled":null,"system_tuning":{},"error":"Kaydedilmiş ölçüm. Canlı Windows ve disk işlemleri kapalı."});
        } else {
            let scan = platform::smart(&self.config.smartctl, &["--scan-open".into()], &self.stop)?;
            if scan["smartctl"]["exit_status"].as_u64().unwrap_or(7) & 7 != 0 {
                return Err("Disk keşfi başarısız. Yönetici erişimini kontrol edin.".into());
            }
            let devices = scan["devices"]
                .as_array()
                .filter(|v| !v.is_empty() && v.len() <= 256)
                .ok_or("Okunabilir SMART aygıtı bulunamadı.")?;
            for device in devices {
                if self.stop.load(Ordering::SeqCst) {
                    return Err(crate::process::CANCELLED.into());
                }
                let name = device["name"].as_str().ok_or("Aygıt yolu yok.")?;
                let kind = device["type"].as_str().ok_or("Aygıt türü yok.")?;
                if !name.starts_with("/dev/")
                    || name.len() > 200
                    || kind.is_empty()
                    || kind.starts_with('-')
                    || kind.len() > 80
                {
                    return Err("Geçersiz aygıt keşif bilgisi.".into());
                }
                match platform::smart(
                    &self.config.smartctl,
                    &[
                        "-i".into(),
                        "-H".into(),
                        "-A".into(),
                        "-d".into(),
                        kind.into(),
                        name.into(),
                    ],
                    &self.stop,
                ) {
                    Ok(mut snapshot) => {
                        if !snapshot["device"].is_object() {
                            snapshot["device"] = json!({});
                        }
                        snapshot["device"]["name"] = json!(name);
                        snapshot["device"]["type"] = json!(kind);
                        snapshots.push(snapshot);
                    }
                    Err(e) => {
                        if self.stop.load(Ordering::SeqCst) || e == crate::process::CANCELLED {
                            return Err(crate::process::CANCELLED.into());
                        }
                        skipped_warnings.push(format!("{name}: {e}"));
                    }
                }
            }
            if snapshots.is_empty() {
                return Err(if skipped_warnings.is_empty() {
                    "Okunabilir SMART aygıtı bulunamadı.".into()
                } else {
                    skipped_warnings.join(" ")
                });
            }
            inventory = platform::inventory(&self.stop).unwrap_or_else(
                |e| json!({"disks":[],"error":e,"trim_enabled":null,"system_tuning":{}}),
            );
        }
        if self.stop.load(Ordering::SeqCst) {
            return Err(crate::process::CANCELLED.into());
        }
        let strict = self.strict_health.load(Ordering::SeqCst);
        let mut disks: Vec<Disk> = snapshots
            .iter()
            .map(|s| model::normalise_with_mode(s, strict))
            .collect();
        let mut identities = HashSet::new();
        if disks.iter().any(|disk| !identities.insert(&disk.id)) {
            return Err(
                "Tekrarlanan SMART disk kimliği bulundu; belirsiz ölçümler geçmişe yazılmadı."
                    .into(),
            );
        }
        platform::attach_volumes(&mut disks, &inventory);
        let healthy_reads: Vec<Disk> = disks
            .iter()
            .filter(|d| !d.read_incomplete)
            .cloned()
            .collect();
        let mut all_warnings = skipped_warnings;
        for d in &disks {
            if let Some(w) = &d.read_warning {
                all_warnings.push(w.clone());
            }
        }
        if !healthy_reads.is_empty() {
            lock(&self.store).record(&healthy_reads, &self.source_key)?;
        }
        let all_incomplete = healthy_reads.is_empty();
        *lock(&self.snapshots) = disks
            .iter()
            .zip(&snapshots)
            .map(|(d, s)| (d.id.clone(), s.clone()))
            .collect();
        let mut data = lock(&self.data);
        data["disks"] = json!(disks);
        data["windows"] = inventory;
        data["stale"] = json!(all_incomplete);
        data["partial_warnings"] = json!(all_warnings);
        data["error"] = if all_incomplete && !all_warnings.is_empty() {
            json!(all_warnings.join(" "))
        } else {
            Value::Null
        };
        data["updated_at"] = json!(Utc::now().to_rfc3339());
        Ok(())
    }

    pub fn refresh(&self) {
        *lock(&self.wake.0) = true;
        self.wake.1.notify_all();
    }

    pub fn start_background(self: &Arc<Self>) {
        let core = self.clone();
        let monitor = std::thread::spawn(move || {
            while !core.stop.load(Ordering::SeqCst) {
                let _ = core.refresh_now();
                let pending = lock(&core.wake.0);
                let (mut pending, _) = core
                    .wake
                    .1
                    .wait_timeout_while(pending, Duration::from_secs(60), |pending| {
                        !*pending && !core.stop.load(Ordering::SeqCst)
                    })
                    .unwrap_or_else(|e| e.into_inner());
                *pending = false;
            }
        });
        lock(&self.workers).push(monitor);
        if self.config.snapshot_dir.is_none() {
            let core = self.clone();
            let performance = std::thread::spawn(move || {
                let mut previous = Value::Null;
                while !core.stop.load(Ordering::SeqCst) {
                    let result =
                        platform::performance(&mut previous, &core.stop).unwrap_or_else(|e| {
                            previous = Value::Null;
                            json!({"disks":[],"error":e})
                        });
                    lock(&core.data)["performance"] = result;
                    for _ in 0..30 {
                        if core.stop.load(Ordering::SeqCst) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            });
            lock(&self.workers).push(performance);
        }
    }

    pub fn history(&self, id: &str) -> Result<Value, String> {
        lock(&self.store).history(id, &self.source_key)
    }
    pub fn events(&self, id: Option<&str>) -> Result<Value, String> {
        lock(&self.store).events(id, &self.source_key)
    }
    pub fn saved_jobs(&self, kind: &str, id: Option<&str>) -> Result<Value, String> {
        lock(&self.store).jobs(kind, id)
    }
    pub fn health_forecast(&self, id: &str) -> Result<Value, String> {
        let disk = self
            .disks()?
            .into_iter()
            .find(|d| d.id == id)
            .ok_or("Disk bulunamadı.")?;
        let history = self.history(id)?;
        let rows = history.as_array().map(Vec::as_slice).unwrap_or(&[]);
        Ok(model::compute_health_forecast(&disk, rows))
    }
    pub fn surface_checkpoint(&self, id: &str) -> Result<Option<Value>, String> {
        Ok(lock(&self.store)
            .get_surface_checkpoint(id)?
            .map(|v| crate::surface_heal::checkpoint_summary(&v)))
    }
    pub fn clear_surface_checkpoint(&self, id: &str) -> Result<(), String> {
        lock(&self.store).clear_surface_checkpoint(id)
    }

    pub fn live_disk(&self, id: &str) -> Result<Disk, String> {
        let data = lock(&self.data);
        if self.config.snapshot_dir.is_some() || data["stale"] != false {
            return Err(
                "İşlem için güncel canlı disk kimliği gerekli; kayıt modunda kapalıdır.".into(),
            );
        }
        drop(data);
        let matches: Vec<_> = self
            .disks()?
            .into_iter()
            .filter(|d| d.id == id && d.stable_identity)
            .collect();
        if matches.len() != 1 {
            return Err("Disk kimliği benzersiz değil; yenileyin.".into());
        }
        if matches[0].read_incomplete {
            return Err(
                "Bu diskin SMART okuması eksik; işlem için Yönetici olarak açıp yenileyin.".into(),
            );
        }
        Ok(matches[0].clone())
    }

    pub fn is_mutating_job(kind: &str, options: &Value) -> bool {
        match kind {
            "recovery" => false,
            "surface" => !matches!(
                options["mode"].as_str(),
                Some("read_scan" | "physical_scan")
            ),
            "defrag" => options["mode"].as_str() != Some("analyze"),
            "benchmark" | "autopilot" | "smart_tier" => true,
            _ => true,
        }
    }

    pub fn require_write_unlocked(&self, action: &str) -> Result<(), String> {
        if self.read_only_guard.load(Ordering::SeqCst) {
            return Err(format!(
                "Salt-okunur koruma etkin ({action} engellendi). Yazma kilidini üst çubuktan veya Ayarlar'dan açın."
            ));
        }
        Ok(())
    }

    pub fn start_job(self: &Arc<Self>, mut request: JobRequest) -> Result<Value, String> {
        jobs::validate_options(&request.kind, &request.options)?;
        if request.letter.len() != 1 || !request.letter.as_bytes()[0].is_ascii_uppercase() {
            return Err("Geçersiz bölüm harfi.".into());
        }
        let expected = match request.kind.as_str() {
            "benchmark" => format!("benchmark:{}:{}", request.disk_id, request.letter),
            "defrag" => format!(
                "defrag:{}:{}:{}",
                request.disk_id,
                request.letter,
                request.options["mode"].as_str().unwrap_or("")
            ),
            "autopilot" => format!("autopilot:{}:{}", request.disk_id, request.letter),
            "recovery" => format!(
                "recovery:{}:{}:{}",
                request.disk_id,
                request.letter,
                request.options["mode"].as_str().unwrap_or("")
            ),
            "smart_tier" => format!(
                "smart_tier:{}:{}:{}",
                request.disk_id,
                request.letter,
                request.options["mode"].as_str().unwrap_or("")
            ),
            _ => format!(
                "surface:{}:{}:{}",
                request.disk_id,
                request.letter,
                request.options["mode"].as_str().unwrap_or("")
            ),
        };
        if request.confirmation != expected {
            return Err("Disk işlemi için açık onay gerekli.".into());
        }
        let disk = self.live_disk(&request.disk_id)?;
        if Self::is_mutating_job(&request.kind, &request.options) {
            self.require_write_unlocked(&request.kind)?;
        }
        if request.kind == "defrag" && disk.kind != "HDD" {
            return Err(
                "SSD'lerde disk birleştirme (Defrag) NAND ömrünü kısaltır ve engellenmiştir; ReTrim kullanın."
                    .into(),
            );
        }
        if request.kind == "smart_tier" {
            let mode = request.options["mode"].as_str().unwrap_or("");
            if mode == "ssd_extent_optimize" && !crate::tiering::is_ssd_disk(&disk) {
                return Err(
                    "SSD Akıllı Parça Temizliği + ReTrim yalnız SSD'lerde desteklenir; HDD için HDD Dış-İz Hızlı Yerleşim kullanın."
                        .into(),
                );
            }
            if mode == "hdd_outer_optimize" && disk.kind != "HDD" {
                return Err(
                    "HDD Dış-İz Hızlı Yerleşim yalnız mekanik HDD'lerde desteklenir; SSD için Akıllı Parça Temizliği + ReTrim kullanın."
                        .into(),
                );
            }
        }
        if (request.kind == "autopilot"
            || request.kind == "smart_tier"
            || (request.kind == "recovery" && request.options["mode"] == "deep"))
            && !platform::is_admin()
        {
            return Err("Bu işlem için Yönetici yetkisi gereklidir.".into());
        }
        if request.kind == "surface" {
            let mode = request.options["mode"].as_str().unwrap_or("read_scan");
            if mode == "bad_zone_isolate" && disk.kind != "HDD" {
                return Err(
                    "Ölü bölge kapatma yalnız mekanik HDD'lerde desteklenir; SSD'ler bozuk hücreleri kendisi değiştirir."
                        .into(),
                );
            }
            if matches!(
                mode,
                "physical_scan" | "magnetic_refresh" | "bad_zone_isolate"
            ) && !platform::is_admin()
            {
                return Err(
                    "Fiziksel LBA tarama ve HDD kurtarma modları için Yönetici yetkisi gereklidir."
                        .into(),
                );
            }
        }
        if disk.os_serial.is_none()
            || disk.os_model.is_none()
            || disk.os_disk_number.is_none()
            || !disk.volumes.iter().any(|v| v["letter"] == request.letter)
        {
            return Err("Windows bölüm/disk kimliği doğrulanamadı.".into());
        }
        if request.options.get("size_mib") == Some(&json!("auto")) {
            let free = disk
                .volumes
                .iter()
                .find(|v| v["letter"] == request.letter)
                .and_then(|v| v["free_bytes"].as_u64());
            let mode = request.options["mode"].as_str().unwrap_or("").to_string();
            request.options["size_mib"] = json!(jobs::auto_budget_mib(
                &request.kind,
                &mode,
                disk.kind == "HDD",
                free
            ));
        }
        if request.kind == "benchmark" {
            jobs::check_engine()?;
        }
        let permit = self.reserve()?;
        self.job_cancel.store(false, Ordering::SeqCst);
        let job = json!({"id":uuid::Uuid::new_v4().simple().to_string(),"kind":request.kind,"disk_id":disk.id,"model":disk.model,"letter":request.letter,"options":request.options,"status":"preparing","phase":"Disk kimliği doğrulanıyor","progress":0,"completed":0,"total":200,"results":[],"blocks":[],"summary":{},"damaged_files":[],"read_bytes":0,"write_bytes":0,"prepared_bytes":0,"started_at":Utc::now().to_rfc3339(),"finished_at":null,"error":null,"cleanup_error":null});
        *lock(&self.job) = job.clone();
        let core = self.clone();
        let worker = std::thread::Builder::new()
            .name("disk-job".into())
            .spawn(move || {
                let _permit = permit;
                jobs::run(&core, &request, &disk);
            })
            .map_err(|e| {
                let mut job = lock(&self.job);
                job["status"] = json!("failed");
                job["error"] = json!(e.to_string());
                e.to_string()
            })?;
        let mut workers = lock(&self.workers);
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.remove(index).join();
            } else {
                index += 1;
            }
        }
        workers.push(worker);
        Ok(job)
    }

    pub fn cancel_job(&self, id: &str) -> Result<Value, String> {
        let mut job = lock(&self.job);
        if job["id"] != id {
            return Err("Durdurulacak iş kimliği değişti.".into());
        }
        if matches!(
            job["status"].as_str(),
            Some("preparing" | "running" | "cancelling")
        ) && job["finished_at"].is_null()
        {
            self.job_cancel.store(true, Ordering::SeqCst);
            job["status"] = json!("cancelling");
            job["phase"] = json!("Durduruluyor ve geçici dosya temizleniyor");
        }
        Ok(job.clone())
    }

    pub fn disk_action(
        &self,
        id: &str,
        action: &str,
        confirmation: Option<&str>,
        letter: Option<&str>,
    ) -> Result<Value, String> {
        if !matches!(
            action,
            "logs" | "short_test" | "long_test" | "abort_test" | "retrim"
        ) {
            return Err("Desteklenmeyen işlem.".into());
        }
        if action == "logs" && self.config.snapshot_dir.is_some() {
            let snapshots = lock(&self.snapshots);
            let snapshot = snapshots.get(id).ok_or("Disk bulunamadı.")?;
            return Ok(Self::logs(snapshot));
        }
        let disk = self.live_disk(id)?;
        if action != "logs" {
            if confirmation != Some(&format!("{action}:{id}")) {
                return Err("İşlem onayı eksik.".into());
            }
            self.require_write_unlocked(action)?;
            if !platform::is_admin() {
                return Err("Bu işlem için yönetici olarak başlatın.".into());
            }
        }
        let _permit = self.reserve()?;
        self.job_cancel.store(false, Ordering::SeqCst);
        let target = vec!["-d".into(), disk.device_type.clone(), disk.device.clone()];
        let mut args = vec!["-i".into()];
        args.extend(target.clone());
        let current = platform::smart(&self.config.smartctl, &args, &self.stop)?;
        if normalise(&current).id != disk.id
            || current["smartctl"]["exit_status"].as_u64().unwrap_or(7) & 7 != 0
        {
            return Err("Aygıt değişti / kimliği okunamıyor. İşlem iptal edildi.".into());
        }
        if action == "logs" {
            let mut args = vec!["-a".into()];
            args.extend(target);
            return platform::smart(&self.config.smartctl, &args, &self.stop)
                .map(|s| Self::logs(&s));
        }
        let inventory = platform::inventory(&self.stop)?;
        platform::verify_disk_mapping(&self.disks()?, &disk, &inventory)?;
        if action == "retrim" {
            if disk.kind == "HDD" {
                return Err("ReTrim yalnız SSD için sunulur.".into());
            }
            let letter = letter.ok_or("Bölüm seçin.")?;
            jobs::validate_target(self, &disk, letter, 0, false, false)?;
            return platform::powershell(
                "ReTrim",
                &[
                    "-DriveLetter".into(),
                    letter.into(),
                    "-ExpectedSerial".into(),
                    disk.os_serial.unwrap_or_default(),
                    "-ExpectedModel".into(),
                    disk.model,
                ],
                &self.stop,
                180,
            );
        }
        if disk.capabilities["self_test"] != true {
            return Err("Aygıt dahili test desteği bildirmiyor.".into());
        }
        let mut args = if action == "abort_test" {
            vec!["-X".into()]
        } else {
            vec![
                "-t".into(),
                if action == "short_test" {
                    "short".into()
                } else {
                    "long".into()
                },
            ]
        };
        args.extend(target);
        let result = platform::smart(&self.config.smartctl, &args, &self.stop)?;
        if result["smartctl"]["exit_status"].as_u64().unwrap_or(7) & 7 != 0 {
            return Err("Aygıt komutu kabul etmedi.".into());
        }
        Ok(
            json!({"status":"requested","message":"Komut aygıta gönderildi. Tamamlanma durumunu günlükten izleyin.","details":result["smartctl"]["messages"]}),
        )
    }

    fn logs(snapshot: &Value) -> Value {
        let mut logs = json!({});
        for key in [
            "nvme_error_information_log",
            "nvme_self_test_log",
            "ata_smart_error_log",
            "ata_smart_self_test_log",
            "ata_smart_selective_self_test_log",
        ] {
            if !snapshot[key].is_null() {
                logs[key] = snapshot[key].clone();
            }
        }
        json!({"status":"read","logs":logs,"note":"Günlük okuması test başlatmaz. Desteklenmeyen komutlar hata günlüğü sayısını artırabilir."})
    }

    pub fn set_minimise(&self, enabled: bool) -> Result<(), String> {
        self.set_preferences(enabled, None, None, None)
    }

    fn write_preferences(&self) -> Result<(), String> {
        let mode = if self.strict_health.load(Ordering::SeqCst) {
            "strict"
        } else {
            "default"
        };
        let minimise = self.minimise_to_tray.load(Ordering::SeqCst);
        let read_only_guard = self.read_only_guard.load(Ordering::SeqCst);
        let lang = lock(&self.language).clone();
        std::fs::create_dir_all(&self.config.data_dir).map_err(|e| e.to_string())?;
        let temp = self.config.data_dir.join("preferences.json.tmp");
        std::fs::write(
            &temp,
            json!({
                "minimise_to_tray": minimise,
                "health_mode": mode,
                "read_only_guard": read_only_guard,
                "language": lang
            })
            .to_string(),
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&temp, self.config.data_dir.join("preferences.json"))
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn set_preferences(
        &self,
        minimise: bool,
        health_mode: Option<&str>,
        language: Option<&str>,
        read_only_guard: Option<bool>,
    ) -> Result<(), String> {
        let mode = match health_mode {
            Some("strict") => "strict",
            Some("default") => "default",
            _ => {
                if self.strict_health.load(Ordering::SeqCst) {
                    "strict"
                } else {
                    "default"
                }
            }
        };
        if let Some(lang) = language {
            let normalized = if lang == "en" { "en" } else { "tr" };
            *lock(&self.language) = normalized.to_owned();
        }
        let new_strict = mode == "strict";
        let changed_strict = self.strict_health.swap(new_strict, Ordering::SeqCst) != new_strict;
        self.minimise_to_tray.store(minimise, Ordering::SeqCst);
        if let Some(ro) = read_only_guard {
            self.read_only_guard.store(ro, Ordering::SeqCst);
        }
        self.write_preferences()?;
        if changed_strict {
            self.refresh();
        }
        Ok(())
    }

    pub fn find_recovery_item(
        &self,
        job_id: &str,
        item_id: &str,
    ) -> Result<(String, Value), String> {
        let current = lock(&self.job).clone();
        if current["id"] == job_id {
            let letter = current["letter"].as_str().unwrap_or("").to_owned();
            if let Some(item) = current["recovery_items"]
                .as_array()
                .and_then(|arr| arr.iter().find(|i| i["id"] == item_id))
            {
                return Ok((letter, item.clone()));
            }
        }
        let saved = lock(&self.store).jobs("recovery", None)?;
        for job in saved.as_array().into_iter().flatten() {
            if job["id"] == job_id {
                let letter = job["letter"].as_str().unwrap_or("").to_owned();
                if let Some(item) = job["recovery_items"]
                    .as_array()
                    .and_then(|arr| arr.iter().find(|i| i["id"] == item_id))
                {
                    return Ok((letter, item.clone()));
                }
            }
        }
        Err("Kurtarılacak dosya kaydı bulunamadı; taramayı yenileyin.".into())
    }

    pub fn find_recovery_job(&self, job_id: &str) -> Result<Value, String> {
        let current = lock(&self.job).clone();
        if current["id"] == job_id && current["kind"] == "recovery" {
            return Ok(current);
        }
        let saved = lock(&self.store).jobs("recovery", None)?;
        for job in saved.as_array().into_iter().flatten() {
            if job["id"] == job_id {
                return Ok(job.clone());
            }
        }
        Err("Kurtarma taraması kaydı bulunamadı; taramayı yenileyin.".into())
    }

    pub fn preview_recovered_item(&self, job_id: &str, item_id: &str) -> Result<Value, String> {
        let job = self.find_recovery_job(job_id)?;
        crate::recovery::preview_item(&job, item_id)
    }

    pub fn restore_recovered_batch_to(
        &self,
        job_id: &str,
        item_ids: &[String],
        dest_dir: &std::path::Path,
    ) -> Result<Value, String> {
        if self.config.snapshot_dir.is_some() || lock(&self.data)["source"] == "replay" {
            return Err("Kayıt modunda toplu dosya kurtarma kapalıdır.".into());
        }
        if item_ids.is_empty() {
            return Err("Toplu kurtarma için en az bir dosya seçin.".into());
        }
        let job = self.find_recovery_job(job_id)?;
        crate::recovery::restore_batch(&job, item_ids, dest_dir)
    }

    pub fn optimize_action(&self, req: OptimizeRequest) -> Result<Value, String> {
        let result = crate::optimize::execute(self, &req)?;
        if req.action != "query_op_bounds" && self.refresh_now().is_err() {
            self.refresh();
        }
        Ok(result)
    }

    pub fn optimizations(&self) -> Result<Value, String> {
        lock(&self.store).list_optimizations()
    }

    pub fn list_tier_moves(&self) -> Result<Value, String> {
        lock(&self.store).list_tier_moves()
    }

    pub fn analyze_storage_tiering(&self, disk_id: &str, letter: &str) -> Result<Value, String> {
        let clean_letter = letter.trim().trim_end_matches(':').to_ascii_uppercase();
        let disks = self.disks()?;
        let selected = disks
            .iter()
            .find(|d| d.id == disk_id)
            .ok_or("Disk bulunamadı.")?;
        let active_moves = lock(&self.store)
            .list_tier_moves()?
            .as_array()
            .cloned()
            .unwrap_or_default();
        if self.config.snapshot_dir.is_some() || lock(&self.data)["source"] == "replay" {
            let replay_letter =
                if clean_letter.len() == 1 && clean_letter.as_bytes()[0].is_ascii_uppercase() {
                    clean_letter.as_str()
                } else {
                    ""
                };
            return Ok(crate::tiering::analyze_volume_replay(
                &disks,
                selected,
                replay_letter,
                &active_moves,
            ));
        }
        if clean_letter.len() != 1 || !clean_letter.as_bytes()[0].is_ascii_uppercase() {
            return Err("Geçersiz bölüm harfi.".into());
        }
        if !selected.volumes.iter().any(|v| {
            v["letter"]
                .as_str()
                .is_some_and(|l| l.eq_ignore_ascii_case(&clean_letter))
        }) {
            return Err("Seçilen bölüm bu diske ait değil.".into());
        }
        crate::tiering::analyze_volume(&disks, selected, &clean_letter, &active_moves)
    }

    pub fn relocate_tier_item(
        &self,
        disk_id: &str,
        source_path: &str,
        target_letter: &str,
        direction: &str,
        confirmation: &str,
    ) -> Result<Value, String> {
        let clean_target = target_letter
            .trim()
            .trim_end_matches(':')
            .to_ascii_uppercase();
        if clean_target.len() != 1 || !clean_target.as_bytes()[0].is_ascii_uppercase() {
            return Err("Geçersiz hedef bölüm harfi.".into());
        }
        if !matches!(direction, "to_hdd" | "to_ssd") {
            return Err("Geçersiz katmanlama yönü (to_hdd veya to_ssd olmalı).".into());
        }
        let expected = format!("tier_relocate:{disk_id}:{clean_target}");
        if confirmation != expected {
            return Err("Katmanlama taşıması için açık onay gerekli.".into());
        }
        let disk = self.live_disk(disk_id)?;
        self.require_write_unlocked("tier_relocate")?;
        if !platform::is_admin() {
            return Err(
                "Katmanlama taşıması ve NTFS Junction oluşturma için Yönetici yetkisi gereklidir."
                    .into(),
            );
        }
        let src_bytes = source_path.as_bytes();
        let src_letter = if src_bytes.len() >= 3
            && src_bytes[0].is_ascii_alphabetic()
            && src_bytes[1] == b':'
            && (src_bytes[2] == b'\\' || src_bytes[2] == b'/')
        {
            (src_bytes[0] as char).to_ascii_uppercase().to_string()
        } else {
            return Err("Geçersiz kaynak dosya/klasör yolu.".into());
        };
        if !disk.volumes.iter().any(|v| {
            v["letter"]
                .as_str()
                .is_some_and(|l| l.eq_ignore_ascii_case(&src_letter))
        }) {
            return Err("Kaynak yol seçilen diske ait değil.".into());
        }
        let all_disks = self.disks()?;
        let target_disk = all_disks
            .iter()
            .find(|d| {
                d.id != disk.id
                    && d.volumes.iter().any(|v| {
                        v["letter"]
                            .as_str()
                            .is_some_and(|l| l.eq_ignore_ascii_case(&clean_target))
                    })
            })
            .ok_or("Hedef bölüm başka bir canlı diskte doğrulanamadı.")?;
        if direction == "to_hdd" && target_disk.kind != "HDD" {
            return Err("Soğuk veri taşıma hedefi bir HDD bölümü olmalıdır.".into());
        }
        if direction == "to_ssd" && !crate::tiering::is_ssd_disk(target_disk) {
            return Err("Sıcak veri terfi hedefi bir SSD bölümü olmalıdır.".into());
        }
        let job_id = uuid::Uuid::new_v4().simple().to_string();
        let rec = {
            let _permit = self.reserve()?;
            self.job_cancel.store(false, Ordering::SeqCst);
            *lock(&self.job) = json!({
                "id": job_id,
                "kind": "tier_relocate",
                "disk_id": disk.id,
                "model": disk.model,
                "letter": src_letter,
                "options": { "direction": direction, "target_letter": clean_target },
                "status": "running",
                "phase": "Dosyalar kopyalanıyor (SHA-256 doğrulama)",
                "progress": 0,
                "completed": 0,
                "total": 100,
                "results": [],
                "blocks": [],
                "summary": {},
                "damaged_files": [],
                "read_bytes": 0,
                "write_bytes": 0,
                "prepared_bytes": 0,
                "started_at": Utc::now().to_rfc3339(),
                "finished_at": null,
                "error": null,
                "cleanup_error": null
            });
            let outcome = (|| -> Result<Value, String> {
                let mut on_progress =
                    |done: u64, total: u64, read_b: u64, write_b: u64, phase: &str| {
                        let pct = ((done.saturating_mul(99)) / total.max(1)).min(99);
                        let mut job = lock(&self.job);
                        job["progress"] = json!(pct);
                        job["completed"] = json!(pct);
                        job["phase"] = json!(phase);
                        job["read_bytes"] = json!(read_b);
                        job["write_bytes"] = json!(write_b);
                    };
                let rec = crate::tiering::relocate_item_with_progress(
                    std::path::Path::new(source_path),
                    &clean_target,
                    direction,
                    &self.job_cancel,
                    &mut on_progress,
                )?;
                lock(&self.store).record_tier_move(&rec)?;
                Ok(rec)
            })();
            match &outcome {
                Ok(rec) => {
                    let mut job = lock(&self.job);
                    job["status"] = json!("completed");
                    job["phase"] = json!("Tamamlandı");
                    job["progress"] = json!(100);
                    job["completed"] = json!(100);
                    job["cleanup_error"] = rec["cleanup_warning"].clone();
                    job["finished_at"] = json!(Utc::now().to_rfc3339());
                }
                Err(e) => {
                    let cancelled = e == crate::process::CANCELLED;
                    let mut job = lock(&self.job);
                    job["status"] = json!(if cancelled { "cancelled" } else { "failed" });
                    job["phase"] = json!(if cancelled {
                        "İptal edildi"
                    } else {
                        "Tamamlanamadı"
                    });
                    job["error"] = json!(e);
                    job["finished_at"] = json!(Utc::now().to_rfc3339());
                }
            }
            outcome?
        };
        if self.refresh_now().is_err() {
            self.refresh();
        }
        Ok(rec)
    }

    pub fn rollback_tier_move(&self, move_id: &str, confirmation: &str) -> Result<Value, String> {
        if move_id.trim().is_empty() {
            return Err("Taşıma kaydı kimliği eksik.".into());
        }
        let expected = format!("tier_rollback:{move_id}");
        if confirmation != expected {
            return Err("Geri alma işlemi için açık onay gerekli.".into());
        }
        {
            let data = lock(&self.data);
            if self.config.snapshot_dir.is_some()
                || data["source"] == "replay"
                || data["stale"] != false
            {
                return Err(
                    "İşlem için güncel canlı disk kimliği gerekli; kayıt modunda kapalıdır.".into(),
                );
            }
        }
        self.require_write_unlocked("tier_rollback")?;
        if !platform::is_admin() {
            return Err("Taşımayı geri almak için Yönetici yetkisi gereklidir.".into());
        }
        let rec = lock(&self.store)
            .get_tier_move(move_id)?
            .ok_or("Geri alınacak taşıma kaydı bulunamadı.")?;
        let job_id = uuid::Uuid::new_v4().simple().to_string();
        let src_letter = rec["source_letter"].as_str().unwrap_or("").to_owned();
        let item_label = std::path::Path::new(rec["source_path"].as_str().unwrap_or("Katmanlama"))
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Katmanlama".into());
        let res = {
            let _permit = self.reserve()?;
            self.job_cancel.store(false, Ordering::SeqCst);
            *lock(&self.job) = json!({
                "id": job_id,
                "kind": "tier_rollback",
                "disk_id": "",
                "model": item_label,
                "letter": src_letter,
                "options": { "move_id": move_id },
                "status": "running",
                "phase": "Özgün konuma geri kopyalanıyor (SHA-256 doğrulama)",
                "progress": 0,
                "completed": 0,
                "total": 100,
                "results": [],
                "blocks": [],
                "summary": {},
                "damaged_files": [],
                "read_bytes": 0,
                "write_bytes": 0,
                "prepared_bytes": 0,
                "started_at": Utc::now().to_rfc3339(),
                "finished_at": null,
                "error": null,
                "cleanup_error": null
            });
            let outcome = (|| -> Result<Value, String> {
                let mut on_progress =
                    |done: u64, total: u64, read_b: u64, write_b: u64, phase: &str| {
                        let pct = ((done.saturating_mul(99)) / total.max(1)).min(99);
                        let mut job = lock(&self.job);
                        job["progress"] = json!(pct);
                        job["completed"] = json!(pct);
                        job["phase"] = json!(phase);
                        job["read_bytes"] = json!(read_b);
                        job["write_bytes"] = json!(write_b);
                    };
                let res = crate::tiering::rollback_move_with_progress(
                    &rec,
                    &self.job_cancel,
                    &mut on_progress,
                )?;
                lock(&self.store).delete_tier_move(move_id)?;
                Ok(res)
            })();
            match &outcome {
                Ok(res) => {
                    let mut job = lock(&self.job);
                    job["status"] = json!("completed");
                    job["phase"] = json!("Tamamlandı");
                    job["progress"] = json!(100);
                    job["completed"] = json!(100);
                    job["cleanup_error"] = res["cleanup_warning"].clone();
                    job["finished_at"] = json!(Utc::now().to_rfc3339());
                }
                Err(e) => {
                    let cancelled = e == crate::process::CANCELLED;
                    let mut job = lock(&self.job);
                    job["status"] = json!(if cancelled { "cancelled" } else { "failed" });
                    job["phase"] = json!(if cancelled {
                        "İptal edildi"
                    } else {
                        "Tamamlanamadı"
                    });
                    job["error"] = json!(e);
                    job["finished_at"] = json!(Utc::now().to_rfc3339());
                }
            }
            outcome?
        };
        if self.refresh_now().is_err() {
            self.refresh();
        }
        Ok(res)
    }

    pub fn shutdown(&self) {
        let busy = lock(&self.busy.0);
        self.stop.store(true, Ordering::SeqCst);
        self.job_cancel.store(true, Ordering::SeqCst);
        *lock(&self.wake.0) = true;
        self.wake.1.notify_all();
        let busy = self
            .busy
            .1
            .wait_while(busy, |busy| *busy)
            .unwrap_or_else(|e| e.into_inner());
        drop(busy);
        let workers = std::mem::take(&mut *lock(&self.workers));
        for worker in workers {
            let _ = worker.join();
        }
    }
}
