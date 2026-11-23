use crate::{
    core::{lock, Core},
    model::Disk,
    platform, process,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizeRequest {
    pub action: String,
    pub target: String,
    pub value: String,
    pub confirmation: String,
}

fn validate_letter(letter: &str) -> Result<(), String> {
    if letter.len() != 1 || !letter.as_bytes()[0].is_ascii_uppercase() {
        return Err("Geçersiz bölüm harfi.".into());
    }
    Ok(())
}

fn split_target(target: &str) -> Result<(&str, &str), String> {
    let (disk_id, letter) = target
        .split_once(':')
        .ok_or("Hedef biçimi geçersiz (<disk_id>:<harf> bekleniyor).")?;
    if disk_id.is_empty() {
        return Err("Disk kimliği boş olamaz.".into());
    }
    validate_letter(letter)?;
    Ok((disk_id, letter))
}

fn verify_disk(core: &Core, disk_id: &str) -> Result<Disk, String> {
    let disk = core.live_disk(disk_id)?;
    let inventory = platform::inventory(&core.stop)?;
    platform::verify_disk_mapping(&core.disks()?, &disk, &inventory)
}

#[cfg(windows)]
fn set_write_cache_ioctl(disk_number: u64, enable: bool) -> Result<bool, String> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        DISK_CACHE_INFORMATION, IOCTL_DISK_GET_CACHE_INFORMATION, IOCTL_DISK_SET_CACHE_INFORMATION,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    let path_w: Vec<u16> = format!("\\\\.\\PhysicalDrive{disk_number}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let h = unsafe {
        CreateFileW(
            path_w.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE || h.is_null() {
        return Err(format!(
            "Fiziksel sürücü açılamadı: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut info = DISK_CACHE_INFORMATION::default();
    let mut ret = 0u32;
    let get_ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_GET_CACHE_INFORMATION,
            null(),
            0,
            (&mut info as *mut DISK_CACHE_INFORMATION).cast(),
            std::mem::size_of::<DISK_CACHE_INFORMATION>() as u32,
            &mut ret,
            null_mut(),
        )
    };
    if get_ok == 0 {
        let err = std::io::Error::last_os_error();
        unsafe {
            CloseHandle(h);
        }
        return Err(format!("Yazma önbelleği bilgisi okunamadı: {err}"));
    }
    let previous = info.WriteCacheEnabled;
    info.WriteCacheEnabled = enable;
    let set_ok = unsafe {
        DeviceIoControl(
            h,
            IOCTL_DISK_SET_CACHE_INFORMATION,
            (&info as *const DISK_CACHE_INFORMATION).cast(),
            std::mem::size_of::<DISK_CACHE_INFORMATION>() as u32,
            null_mut(),
            0,
            &mut ret,
            null_mut(),
        )
    };
    let err = std::io::Error::last_os_error();
    unsafe {
        CloseHandle(h);
    }
    if set_ok == 0 {
        return Err(format!("Aygıt yazma önbelleği değişimini reddetti: {err}"));
    }
    Ok(previous)
}

#[cfg(windows)]
fn set_autostart_registry(enable: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
    };

    let sub_w: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Run"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let name_w: Vec<u16> = "SSDSentinel".encode_utf16().chain(Some(0)).collect();
    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let val = format!("\"{}\" --minimized", exe.display());
        let val_w: Vec<u16> = val.encode_utf16().chain(Some(0)).collect();
        let rc = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                sub_w.as_ptr(),
                name_w.as_ptr(),
                REG_SZ,
                val_w.as_ptr().cast(),
                (val_w.len() * 2) as u32,
            )
        };
        if rc != 0 {
            return Err(format!("Başlangıç kaydı yazılamadı (hata kodu {rc})."));
        }
    } else {
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, sub_w.as_ptr(), name_w.as_ptr()) };
        if rc != 0 && rc != 2 {
            return Err(format!("Başlangıç kaydı silinemedi (hata kodu {rc})."));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn set_long_paths_registry(enable: bool) -> Result<bool, String> {
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, RegSetKeyValueW, HKEY_LOCAL_MACHINE, REG_DWORD, RRF_RT_REG_DWORD,
    };

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let sub_w = wide("SYSTEM\\CurrentControlSet\\Control\\FileSystem");
    let name_w = wide("LongPathsEnabled");
    let mut prev_raw: u32 = 0;
    let mut size: u32 = std::mem::size_of::<u32>() as u32;
    let prev_rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_w.as_ptr(),
            name_w.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut prev_raw as *mut u32).cast(),
            &mut size,
        )
    };
    let prev_enabled = prev_rc == 0 && prev_raw != 0;
    let new_val: u32 = u32::from(enable);
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            sub_w.as_ptr(),
            name_w.as_ptr(),
            REG_DWORD,
            (&new_val as *const u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    if rc != 0 {
        return Err(format!(
            "LongPathsEnabled kayıt defteri yazılamadı (hata kodu {rc})."
        ));
    }
    Ok(prev_enabled)
}

fn run_ps(core: &Core, script: &str, timeout_secs: u64) -> Result<String, String> {
    let args = vec![
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-Command".into(),
        script.into(),
    ];
    let out = process::run(
        &platform::system_executable("WindowsPowerShell/v1.0/powershell.exe"),
        &args,
        Duration::from_secs(timeout_secs),
        &core.stop,
    )?;
    if out.code != 0 {
        return Err(out.stderr.chars().take(1200).collect());
    }
    Ok(out.stdout)
}

fn query_partition_bounds(core: &Core, letter: &str) -> Result<(u64, u64, u64), String> {
    validate_letter(letter)?;
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $s = Get-PartitionSupportedSize -DriveLetter '{letter}'; \
         $p = Get-Partition -DriveLetter '{letter}'; \
         @{{ size_min = [uint64]$s.SizeMin; size_max = [uint64]$s.SizeMax; size = [uint64]$p.Size }} | ConvertTo-Json -Compress"
    );
    let stdout = run_ps(core, &script, 30)?;
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Bölüm sınırları okunamadı: {e}"))?;
    let size_min = v["size_min"].as_u64().ok_or("SizeMin okunamadı.")?;
    let size_max = v["size_max"].as_u64().ok_or("SizeMax okunamadı.")?;
    let size = v["size"].as_u64().ok_or("Bölüm boyutu okunamadı.")?;
    Ok((size_min, size_max, size))
}

pub fn execute(core: &Core, req: &OptimizeRequest) -> Result<Value, String> {
    let _permit = core.reserve()?;
    execute_unlocked(core, req)
}

pub(crate) fn execute_unlocked(core: &Core, req: &OptimizeRequest) -> Result<Value, String> {
    if !matches!(
        req.action.as_str(),
        "write_cache"
            | "ntfs_trim"
            | "pcie_aspm"
            | "hibernation"
            | "hdd_apm"
            | "query_op_bounds"
            | "apply_op"
            | "reclaim_op"
            | "guard_tail"
            | "expand_tail"
            | "release_quarantine"
            | "autostart"
            | "long_paths"
            | "ntfs_8dot3"
            | "compact_wsl_vhdx"
    ) {
        return Err("Desteklenmeyen optimizasyon işlemi.".into());
    }
    if req.confirmation != format!("optimize:{}:{}", req.action, req.target) {
        return Err("Optimizasyon işlemi için açık onay anahtarı eksik veya hatalı.".into());
    }
    {
        let data = lock(&core.data);
        if core.config.snapshot_dir.is_some() || data["source"] == "replay" {
            return Err("Kayıt modunda sistem ve disk optimizasyonları kapalıdır.".into());
        }
        if data["stale"] != false {
            return Err("Disk verileri güncel değil; önce SMART verilerini yenileyin.".into());
        }
    }
    if !matches!(req.action.as_str(), "query_op_bounds" | "autostart") {
        core.require_write_unlocked(&req.action)?;
    }
    if req.action != "autostart" && !platform::is_admin() {
        return Err("Bu işlem için uygulamayı Yönetici olarak başlatın.".into());
    }

    match req.action.as_str() {
        "autostart" => {
            if !matches!(req.value.as_str(), "enable" | "disable") {
                return Err("Geçersiz başlangıç tercihi.".into());
            }
            #[cfg(windows)]
            {
                let enable = req.value == "enable";
                set_autostart_registry(enable)?;
                Ok(json!({
                    "status": "completed",
                    "action": "autostart",
                    "enabled": enable,
                    "message": if enable {
                        "SSD Sentinel Windows açılışında sistem tepsisinde başlatılacak."
                    } else {
                        "Windows açılışında otomatik başlatma kapatıldı."
                    }
                }))
            }
            #[cfg(not(windows))]
            {
                Err("Yalnız Windows desteklenmektedir.".into())
            }
        }
        "write_cache" => {
            if !matches!(req.value.as_str(), "enable" | "disable" | "rollback") {
                return Err("Geçersiz yazma önbelleği değeri.".into());
            }
            let mapped = verify_disk(core, &req.target)?;
            let num = mapped
                .os_disk_number
                .ok_or("Fiziksel disk numarası doğrulanamadı.")?;
            let key = format!("write_cache:{}", req.target);
            let target_enable = if req.value == "rollback" {
                let saved = lock(&core.store)
                    .get_optimization(&key)?
                    .ok_or("Geri alınacak yazma önbelleği kaydı bulunamadı.")?;
                saved["previous_value"].as_str() == Some("enable")
            } else {
                req.value == "enable"
            };
            #[cfg(windows)]
            let prev = {
                let was_enabled = set_write_cache_ioctl(num, target_enable)?;
                if was_enabled {
                    "enable"
                } else {
                    "disable"
                }
            };
            #[cfg(not(windows))]
            let prev = {
                let _ = (num, target_enable);
                return Err("Yalnız Windows desteklenmektedir.".into());
            };
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(&key)?;
            } else {
                lock(&core.store).record_optimization(
                    &key,
                    &req.target,
                    &format!("Yazma önbelleği · {}", mapped.model),
                    prev,
                    &req.value,
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "write_cache",
                "enabled": target_enable,
                "message": format!(
                    "{} için donanım yazma önbelleği {}.",
                    mapped.model,
                    if target_enable { "etkinleştirildi" } else { "kapatıldı" }
                )
            }))
        }
        "ntfs_trim" => {
            if !matches!(req.value.as_str(), "enable" | "rollback") {
                return Err("Geçersiz TRIM ayarı.".into());
            }
            let key = "ntfs_trim:system";
            let current = lock(&core.data)["windows"]["trim_enabled"].as_bool();
            let prev = match current {
                Some(true) => "enable",
                Some(false) => "disable",
                None => "enable",
            };
            let target_flag = if req.value == "rollback" {
                let saved = lock(&core.store)
                    .get_optimization(key)?
                    .ok_or("Geri alınacak TRIM kaydı bulunamadı.")?;
                if saved["previous_value"].as_str() == Some("disable") {
                    "1"
                } else {
                    "0"
                }
            } else {
                "0"
            };
            let out = process::run(
                &platform::system_executable("fsutil.exe"),
                &[
                    "behavior".into(),
                    "set".into(),
                    "DisableDeleteNotify".into(),
                    target_flag.into(),
                ],
                Duration::from_secs(20),
                &core.stop,
            )?;
            if out.code != 0 {
                return Err(format!(
                    "fsutil başarısız (kod {}): {}",
                    out.code, out.stderr
                ));
            }
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(key)?;
            } else {
                lock(&core.store).record_optimization(
                    key,
                    "system",
                    "Windows NTFS/ReFS TRIM (DisableDeleteNotify)",
                    prev,
                    "enable",
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "ntfs_trim",
                "message": if target_flag == "0" {
                    "Windows TRIM bildirimleri etkinleştirildi."
                } else {
                    "Windows TRIM ayarı önceki değerine döndürüldü."
                }
            }))
        }
        "pcie_aspm" => {
            if !matches!(req.value.as_str(), "off" | "moderate" | "rollback") {
                return Err("Geçersiz PCIe ASPM modu.".into());
            }
            let key = "pcie_aspm:system";
            let prev_idx = lock(&core.data)["windows"]["system_tuning"]["pcie_aspm"]
                .as_u64()
                .unwrap_or(1)
                .to_string();
            let target_idx = if req.value == "rollback" {
                let saved = lock(&core.store)
                    .get_optimization(key)?
                    .ok_or("Geri alınacak PCIe ASPM kaydı bulunamadı.")?;
                let val = saved["previous_value"].as_str().unwrap_or("1").to_owned();
                if !matches!(val.as_str(), "0" | "1" | "2") {
                    return Err("Kayıtlı ASPM değeri geçersiz.".into());
                }
                val
            } else if req.value == "off" {
                "0".into()
            } else {
                "1".into()
            };
            let powercfg = platform::system_executable("powercfg.exe");
            let out = process::run(
                &powercfg,
                &[
                    "/setacvalueindex".into(),
                    "SCHEME_CURRENT".into(),
                    "501a4d13-42af-4429-9fd1-a8218c268e20".into(),
                    "ee12f906-d277-404b-b6da-e5fa1a576df5".into(),
                    target_idx.clone(),
                ],
                Duration::from_secs(15),
                &core.stop,
            )?;
            if out.code != 0 {
                return Err(format!("powercfg başarısız: {}", out.stderr));
            }
            let act = process::run(
                &powercfg,
                &["/setactive".into(), "SCHEME_CURRENT".into()],
                Duration::from_secs(15),
                &core.stop,
            )?;
            if act.code != 0 {
                return Err(format!("Güç planı etkinleştirilemedi: {}", act.stderr));
            }
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(key)?;
            } else {
                lock(&core.store).record_optimization(
                    key,
                    "system",
                    "PCIe ASPM Güç Yönetimi (NVMe gecikme profili)",
                    &prev_idx,
                    &target_idx,
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "pcie_aspm",
                "value": target_idx,
                "message": "Aktif güç planında PCIe ASPM ayarı güncellendi."
            }))
        }
        "hibernation" => {
            if !matches!(req.value.as_str(), "off" | "on" | "rollback") {
                return Err("Geçersiz hazırda bekletme değeri.".into());
            }
            let key = "hibernation:system";
            let current = lock(&core.data)["windows"]["system_tuning"]["hibernate_enabled"]
                .as_bool()
                .unwrap_or(true);
            let prev = if current { "on" } else { "off" };
            let target = if req.value == "rollback" {
                let saved = lock(&core.store)
                    .get_optimization(key)?
                    .ok_or("Geri alınacak hazırda bekletme kaydı bulunamadı.")?;
                match saved["previous_value"].as_str() {
                    Some("on") => "on",
                    _ => "off",
                }
            } else {
                req.value.as_str()
            };
            let out = process::run(
                &platform::system_executable("powercfg.exe"),
                &["/h".into(), target.into()],
                Duration::from_secs(30),
                &core.stop,
            )?;
            if out.code != 0 {
                return Err(format!("powercfg /h başarısız: {}", out.stderr));
            }
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(key)?;
            } else {
                lock(&core.store).record_optimization(
                    key,
                    "system",
                    "Hazırda Bekletme (hiberfil.sys)",
                    prev,
                    target,
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "hibernation",
                "value": target,
                "message": if target == "off" {
                    "Hazırda bekletme kapatıldı ve hiberfil.sys alanı geri kazanıldı."
                } else {
                    "Hazırda bekletme etkinleştirildi."
                }
            }))
        }
        "hdd_apm" => {
            if !matches!(req.value.as_str(), "254" | "128" | "off" | "rollback") {
                return Err("Geçersiz HDD APM seviyesi.".into());
            }
            let mapped = verify_disk(core, &req.target)?;
            if mapped.kind != "HDD" {
                return Err("APM kafa park kontrolü yalnız mekanik HDD'lerde sunulur.".into());
            }
            let key = format!("hdd_apm:{}", req.target);
            let level = if req.value == "rollback" {
                let saved = lock(&core.store)
                    .get_optimization(&key)?
                    .ok_or("Geri alınacak HDD APM kaydı bulunamadı.")?;
                saved["previous_value"].as_str().unwrap_or("128").to_owned()
            } else {
                req.value.clone()
            };
            let res = platform::smart(
                &core.config.smartctl,
                &[
                    "-s".into(),
                    format!("apm,{level}"),
                    "-d".into(),
                    mapped.device_type.clone(),
                    mapped.device.clone(),
                ],
                &core.stop,
            )?;
            if res["smartctl"]["exit_status"].as_u64().unwrap_or(7) & 7 != 0 {
                return Err("Aygıt APM komutunu kabul etmedi.".into());
            }
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(&key)?;
            } else {
                lock(&core.store).record_optimization(
                    &key,
                    &req.target,
                    &format!("HDD APM Seviyesi · {}", mapped.model),
                    "128",
                    &level,
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "hdd_apm",
                "value": level,
                "message": format!("{} için APM seviyesi '{level}' olarak ayarlandı.", mapped.model)
            }))
        }
        "query_op_bounds" => {
            let (disk_id, letter) = split_target(&req.target)?;
            let mapped = verify_disk(core, disk_id)?;
            if mapped.kind == "HDD" {
                return Err("Over-Provisioning yalnız SSD'lerde desteklenir.".into());
            }
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or("Seçilen bölüm diskte bulunamadı.")?;
            platform::check_filesystem(vol, false)?;
            let (size_min, size_max, size) = query_partition_bounds(core, letter)?;
            Ok(json!({
                "status": "ok",
                "letter": letter,
                "size_min": size_min,
                "size_max": size_max,
                "current_size": size,
                "free_bytes": vol["free_bytes"]
            }))
        }
        "apply_op" | "guard_tail" => {
            let is_tail = req.action == "guard_tail";
            let pct: u64 = match (is_tail, req.value.as_str()) {
                (false, "5") | (true, "5") => 5,
                (false, "7") => 7,
                (false, "10") | (true, "10") => 10,
                (true, "15") => 15,
                _ => {
                    return Err(if is_tail {
                        "Ölü kuyruk koruma oranı %5, %10 veya %15 olmalıdır.".into()
                    } else {
                        "Over-Provisioning oranı %5, %7 veya %10 olmalıdır.".into()
                    })
                }
            };
            let (disk_id, letter) = split_target(&req.target)?;
            let mapped = verify_disk(core, disk_id)?;
            if !is_tail && mapped.kind == "HDD" {
                return Err("Over-Provisioning yalnız SSD'lerde desteklenir.".into());
            }
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or("Seçilen bölüm diskte bulunamadı.")?;
            platform::check_filesystem(vol, false)?;
            let disk_cap = mapped.metrics["capacity_bytes"]
                .as_u64()
                .or_else(|| {
                    mapped.metrics["capacity_bytes"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                })
                .ok_or("Disk toplam kapasitesi okunamadı.")?;
            let unallocated = mapped.optimization["unallocated_bytes"]
                .as_u64()
                .unwrap_or(0);
            let target_unalloc = disk_cap / 100 * pct;
            if unallocated >= target_unalloc {
                return Ok(json!({
                    "status": "already_satisfied",
                    "message": format!("Disk sonunda zaten %{pct} veya daha fazla ayrılmamış koruma alanı mevcut.")
                }));
            }
            let delta = target_unalloc - unallocated;
            let vol_free = vol["free_bytes"].as_u64().unwrap_or(0);
            let min_keep_free = 10 * 1024 * 1024 * 1024u64;
            if vol_free.saturating_sub(delta) < min_keep_free {
                return Err("Bölüm daraltıldıktan sonra en az 10 GiB boş alan kalmalıdır.".into());
            }
            let (size_min, _size_max, current_size) = query_partition_bounds(core, letter)?;
            let new_size = ((current_size.saturating_sub(delta)) / (1024 * 1024)) * (1024 * 1024);
            let safety_min = size_min.saturating_add(64 * 1024 * 1024);
            if new_size < safety_min || new_size >= current_size {
                return Err(
                    "Taşınamayan sistem dosyaları nedeniyle bölüm istenen boyuta daraltılamıyor."
                        .into(),
                );
            }
            let trim_step = if mapped.kind == "HDD" {
                String::new()
            } else {
                format!("Optimize-Volume -DriveLetter '{letter}' -ReTrim -ErrorAction Stop; ")
            };
            let script = format!(
                "$ErrorActionPreference='Stop'; {trim_step}Resize-Partition -DriveLetter '{letter}' -Size {new_size} -ErrorAction Stop"
            );
            run_ps(core, &script, 300)?;
            let key = if is_tail {
                format!("tail:{disk_id}:{letter}")
            } else {
                format!("op:{disk_id}:{letter}")
            };
            let title = if is_tail {
                format!("Ölü Kuyruk İzolasyonu (%{pct}) · {letter}:")
            } else {
                format!("SSD Over-Provisioning (%{pct}) · {letter}:")
            };
            lock(&core.store).record_optimization(
                &key,
                &req.target,
                &title,
                &current_size.to_string(),
                &new_size.to_string(),
            )?;
            Ok(json!({
                "status": "completed",
                "action": req.action,
                "letter": letter,
                "new_size": new_size,
                "message": if is_tail {
                    format!("{letter}: bölümü sondan %{pct} küçültülerek bozuk kuyruk izleri bölüm dışına çıkarıldı.")
                } else {
                    format!("{letter}: bölümü küçültülerek %{pct} Over-Provisioning ayrılmamış alanı ayrıldı ve TRIM ile temizlendi.")
                }
            }))
        }
        "reclaim_op" | "expand_tail" => {
            let is_tail = req.action == "expand_tail";
            let (disk_id, letter) = split_target(&req.target)?;
            let mapped = verify_disk(core, disk_id)?;
            if !is_tail && mapped.kind == "HDD" {
                return Err("Over-Provisioning yalnız SSD'lerde desteklenir.".into());
            }
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or("Seçilen bölüm diskte bulunamadı.")?;
            platform::check_filesystem(vol, false)?;
            let (_size_min, size_max, current_size) = query_partition_bounds(core, letter)?;
            if size_max <= current_size {
                return Ok(json!({
                    "status": "already_satisfied",
                    "message": "Bölüm arkasında geri katılabilecek ayrılmamış alan bulunmuyor."
                }));
            }
            let script = format!(
                "$ErrorActionPreference='Stop'; \
                 Resize-Partition -DriveLetter '{letter}' -Size {size_max} -ErrorAction Stop"
            );
            run_ps(core, &script, 180)?;
            let key = if is_tail {
                format!("tail:{disk_id}:{letter}")
            } else {
                format!("op:{disk_id}:{letter}")
            };
            lock(&core.store).remove_optimization(&key)?;
            Ok(json!({
                "status": "completed",
                "action": req.action,
                "letter": letter,
                "new_size": size_max,
                "message": format!("{letter}: bölümü bitişik ayrılmamış alanla ({size_max} bayt) genişletildi.")
            }))
        }
        "release_quarantine" => {
            let (disk_id, letter) = split_target(&req.target)?;
            let mapped = verify_disk(core, disk_id)?;
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or("Seçilen bölüm diskte bulunamadı.")?;
            platform::check_filesystem(vol, false)?;
            let root = platform::volume_root(letter)?;
            let removed = crate::surface_heal::release_quarantine_vault(&root)?;
            let key = format!("quarantine:{disk_id}:{letter}");
            lock(&core.store).remove_optimization(&key)?;
            Ok(json!({
                "status": "completed",
                "action": "release_quarantine",
                "letter": letter,
                "removed_files": removed,
                "message": format!("{letter}: üzerindeki karantina kalkanı kaldırıldı ({removed} bölge dosyası temizlendi).")
            }))
        }
        "long_paths" => {
            if req.target != "system" {
                return Err("Geçersiz uzun yol hedefi.".into());
            }
            if !matches!(req.value.as_str(), "enable" | "disable" | "rollback") {
                return Err("Geçersiz uzun yol değeri.".into());
            }
            let key = "long_paths:system";
            let target_enable = if req.value == "rollback" {
                let saved = lock(&core.store)
                    .get_optimization(key)?
                    .ok_or("Geri alınacak uzun yol kaydı bulunamadı.")?;
                saved["previous_value"].as_str() == Some("enable")
            } else {
                req.value == "enable"
            };
            #[cfg(windows)]
            let prev = {
                if set_long_paths_registry(target_enable)? {
                    "enable"
                } else {
                    "disable"
                }
            };
            #[cfg(not(windows))]
            let prev = {
                let _ = target_enable;
                return Err("Yalnız Windows desteklenmektedir.".into());
            };
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(key)?;
            } else {
                lock(&core.store).record_optimization(
                    key,
                    "system",
                    "Windows Uzun Yol Desteği (LongPathsEnabled > 260 karakter)",
                    prev,
                    &req.value,
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "long_paths",
                "enabled": target_enable,
                "message": if target_enable {
                    "Windows uzun dosya yolu desteği (LongPathsEnabled) etkinleştirildi."
                } else {
                    "Windows uzun dosya yolu desteği kapatıldı."
                }
            }))
        }
        "ntfs_8dot3" => {
            if req.target != "system" {
                return Err("Geçersiz NTFS 8.3 hedefi.".into());
            }
            if !matches!(req.value.as_str(), "disable" | "default" | "rollback") {
                return Err("Geçersiz NTFS 8.3 ayarı.".into());
            }
            let key = "ntfs_8dot3:system";
            let prev_idx = lock(&core.data)["windows"]["system_tuning"]["ntfs_8dot3_creation"]
                .as_u64()
                .filter(|v| *v <= 3)
                .unwrap_or(2)
                .to_string();
            let target_idx = if req.value == "rollback" {
                let saved = lock(&core.store)
                    .get_optimization(key)?
                    .ok_or("Geri alınacak NTFS 8.3 kaydı bulunamadı.")?;
                let val = saved["previous_value"].as_str().unwrap_or("2").to_owned();
                if !matches!(val.as_str(), "0" | "1" | "2" | "3") {
                    return Err("Kayıtlı NTFS 8.3 değeri geçersiz.".into());
                }
                val
            } else if req.value == "disable" {
                "1".into()
            } else {
                "2".into()
            };
            let out = process::run(
                &platform::system_executable("fsutil.exe"),
                &[
                    "behavior".into(),
                    "set".into(),
                    "disable8dot3".into(),
                    target_idx.clone(),
                ],
                Duration::from_secs(20),
                &core.stop,
            )?;
            if out.code != 0 {
                return Err(format!(
                    "fsutil disable8dot3 başarısız (kod {}): {}",
                    out.code, out.stderr
                ));
            }
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(key)?;
            } else {
                lock(&core.store).record_optimization(
                    key,
                    "system",
                    "NTFS 8.3 Kısa Dosya Adı Oluşturma (disable8dot3)",
                    &prev_idx,
                    &target_idx,
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "ntfs_8dot3",
                "value": target_idx,
                "message": if target_idx == "1" {
                    "NTFS 8.3 kısa dosya adı üretimi tüm birimlerde kapatıldı (node_modules / target hızlandırması)."
                } else {
                    "NTFS 8.3 kısa dosya adı ayarı güncellendi."
                }
            }))
        }
        "compact_wsl_vhdx" => {
            if req.value != "compact" {
                return Err("Geçersiz VHDX sıkıştırma komutu.".into());
            }
            let valid_path = crate::dev_workspace::validate_wsl_vhdx_path(&req.target)?;
            #[cfg(windows)]
            {
                use std::io::Write;
                let size_before = std::fs::metadata(&valid_path)
                    .map(|m| m.len())
                    .map_err(|_| "VHDX dosyası boyutu okunamadı.")?;
                let _ = process::run(
                    &platform::system_executable("wsl.exe"),
                    &["--shutdown".into()],
                    Duration::from_secs(30),
                    &core.stop,
                );
                for _ in 0..15 {
                    if core.stop.load(std::sync::atomic::Ordering::SeqCst) {
                        return Err(process::CANCELLED.into());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let mut script = tempfile::Builder::new()
                    .suffix(".txt")
                    .tempfile()
                    .map_err(|e| format!("diskpart geçici betiği oluşturulamadı: {e}"))?;
                let body = format!(
                    "select vdisk file=\"{}\"\r\nattach vdisk readonly\r\ncompact vdisk\r\ndetach vdisk\r\nexit\r\n",
                    valid_path.display()
                );
                script
                    .write_all(body.as_bytes())
                    .and_then(|()| script.flush())
                    .map_err(|e| format!("diskpart betiği yazılamadı: {e}"))?;
                let out = process::run(
                    &platform::system_executable("diskpart.exe"),
                    &["/s".into(), script.path().to_string_lossy().into_owned()],
                    Duration::from_secs(600),
                    &core.stop,
                )?;
                if out.code != 0 {
                    let detail = if out.stderr.trim().is_empty() {
                        &out.stdout
                    } else {
                        &out.stderr
                    };
                    return Err(format!(
                        "diskpart compact başarısız (kod {}): {}",
                        out.code,
                        detail.chars().take(600).collect::<String>()
                    ));
                }
                let size_after = std::fs::metadata(&valid_path)
                    .map(|m| m.len())
                    .unwrap_or(size_before);
                let freed_bytes = size_before.saturating_sub(size_after);
                Ok(json!({
                    "status": "completed",
                    "action": "compact_wsl_vhdx",
                    "path": valid_path.to_string_lossy(),
                    "size_before": size_before,
                    "size_after": size_after,
                    "freed_bytes": freed_bytes,
                    "message": format!(
                        "{} sıkıştırıldı ({:.2} GiB → {:.2} GiB · {:.2} GiB geri kazanıldı).",
                        valid_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        size_before as f64 / (1024.0 * 1024.0 * 1024.0),
                        size_after as f64 / (1024.0 * 1024.0 * 1024.0),
                        freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                    )
                }))
            }
            #[cfg(not(windows))]
            {
                let _ = valid_path;
                Err("Yalnız Windows desteklenmektedir.".into())
            }
        }
        _ => Err("Desteklenmeyen optimizasyon işlemi.".into()),
    }
}
