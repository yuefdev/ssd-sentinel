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

fn validate_letter(letter: &str, en: bool) -> Result<(), String> {
    if !platform::is_valid_volume_id(letter) {
        return Err(crate::i18n::pick(
            en,
            "Geçersiz bölüm/bağlama noktası.",
            "Invalid volume identifier.",
        )
        .into());
    }
    Ok(())
}

fn split_target(target: &str, en: bool) -> Result<(&str, &str), String> {
    let (disk_id, letter) = target.split_once(':').ok_or(crate::i18n::pick(
        en,
        "Hedef biçimi geçersiz (<disk_id>:<harf> bekleniyor).",
        "Invalid target format (expected <disk_id>:<letter>).",
    ))?;
    if disk_id.is_empty() {
        return Err(
            crate::i18n::pick(en, "Disk kimliği boş olamaz.", "Drive ID cannot be empty.").into(),
        );
    }
    validate_letter(letter, en)?;
    Ok((disk_id, letter))
}

fn verify_disk(core: &Core, disk_id: &str) -> Result<Disk, String> {
    let disk = core.live_disk(disk_id)?;
    let inventory = platform::inventory(&core.stop)?;
    platform::verify_disk_mapping(&core.disks()?, &disk, &inventory)
}

#[cfg(windows)]
fn set_write_cache_ioctl(disk_number: u64, enable: bool, en: bool) -> Result<bool, String> {
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
        return Err(if en {
            format!(
                "Could not open the physical drive: {}",
                std::io::Error::last_os_error()
            )
        } else {
            format!(
                "Fiziksel sürücü açılamadı: {}",
                std::io::Error::last_os_error()
            )
        });
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
        return Err(if en {
            format!("Could not read write cache information: {err}")
        } else {
            format!("Yazma önbelleği bilgisi okunamadı: {err}")
        });
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
        return Err(if en {
            format!("The device rejected the write cache change: {err}")
        } else {
            format!("Aygıt yazma önbelleği değişimini reddetti: {err}")
        });
    }
    Ok(previous)
}

#[cfg(windows)]
fn set_autostart_registry(enable: bool, en: bool) -> Result<(), String> {
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
            return Err(if en {
                format!("Could not write the startup entry (error code {rc}).")
            } else {
                format!("Başlangıç kaydı yazılamadı (hata kodu {rc}).")
            });
        }
    } else {
        let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, sub_w.as_ptr(), name_w.as_ptr()) };
        if rc != 0 && rc != 2 {
            return Err(if en {
                format!("Could not remove the startup entry (error code {rc}).")
            } else {
                format!("Başlangıç kaydı silinemedi (hata kodu {rc}).")
            });
        }
    }
    Ok(())
}

#[cfg(windows)]
fn set_long_paths_registry(enable: bool, en: bool) -> Result<bool, String> {
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
        return Err(if en {
            format!("Could not write the LongPathsEnabled registry value (error code {rc}).")
        } else {
            format!("LongPathsEnabled kayıt defteri yazılamadı (hata kodu {rc}).")
        });
    }
    Ok(prev_enabled)
}

#[cfg(windows)]
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

#[cfg(windows)]
fn query_partition_bounds(core: &Core, letter: &str) -> Result<(u64, u64, u64), String> {
    let en = core.is_en();
    validate_letter(letter, en)?;
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $s = Get-PartitionSupportedSize -DriveLetter '{letter}'; \
         $p = Get-Partition -DriveLetter '{letter}'; \
         @{{ size_min = [uint64]$s.SizeMin; size_max = [uint64]$s.SizeMax; size = [uint64]$p.Size }} | ConvertTo-Json -Compress"
    );
    let stdout = run_ps(core, &script, 30)?;
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|e| {
        if en {
            format!("Could not read partition bounds: {e}")
        } else {
            format!("Bölüm sınırları okunamadı: {e}")
        }
    })?;
    let size_min = v["size_min"]
        .as_u64()
        .ok_or(core.t("SizeMin okunamadı.", "Could not read SizeMin."))?;
    let size_max = v["size_max"]
        .as_u64()
        .ok_or(core.t("SizeMax okunamadı.", "Could not read SizeMax."))?;
    let size = v["size"].as_u64().ok_or(core.t(
        "Bölüm boyutu okunamadı.",
        "Could not read the partition size.",
    ))?;
    Ok((size_min, size_max, size))
}

#[cfg(target_os = "linux")]
fn set_autostart_xdg(enable: bool, en: bool) -> Result<(), String> {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| std::path::PathBuf::from(h).join(".config"))
        })
        .ok_or_else(|| {
            crate::i18n::pick(
                en,
                "XDG_CONFIG_HOME veya HOME belirlenemedi.",
                "Could not locate XDG_CONFIG_HOME or HOME.",
            )
            .to_owned()
        })?;
    let dir = config_home.join("autostart");
    let file = dir.join("ssd-sentinel.desktop");
    if enable {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=SSD Sentinel\n\
             Comment=SSD & HDD Health Diagnostics\n\
             Exec=\"{}\" --minimized\n\
             Icon=com.ssdsentinel.desktop\n\
             Terminal=false\n\
             Categories=System;Utility;\n\
             X-GNOME-Autostart-enabled=true\n",
            exe.display()
        );
        std::fs::write(&file, desktop).map_err(|e| e.to_string())?;
    } else if file.exists() {
        std::fs::remove_file(&file).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_block_name(device_path: &str) -> Result<String, String> {
    let name = std::path::Path::new(device_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_owned();
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err("Geçersiz Linux blok aygıtı adı.".into());
    }
    Ok(name)
}

#[cfg(target_os = "linux")]
fn query_linux_bounds(mount_point: &str) -> Result<(u64, u64, u64), String> {
    let (total, free) = crate::platform_linux::statvfs_bytes(std::path::Path::new(mount_point))
        .ok_or("Linux bölüm kapasitesi okunamadı.")?;
    let reserve_floor = 10 * 1024 * 1024 * 1024u64;
    let used = total.saturating_sub(free);
    let min_size = used.saturating_add(reserve_floor).min(total);
    Ok((min_size, total, total))
}

pub fn execute(core: &Core, req: &OptimizeRequest) -> Result<Value, String> {
    let _permit = core.reserve()?;
    execute_unlocked(core, req)
}

pub(crate) fn execute_unlocked(core: &Core, req: &OptimizeRequest) -> Result<Value, String> {
    let en = core.is_en();
    if !matches!(
        req.action.as_str(),
        "write_cache"
            | "ntfs_trim"
            | "pcie_aspm"
            | "hibernation"
            | "hdd_apm"
            | "io_scheduler"
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
        return Err(core
            .t(
                "Desteklenmeyen optimizasyon işlemi.",
                "Unsupported optimization action.",
            )
            .into());
    }
    if req.confirmation != format!("optimize:{}:{}", req.action, req.target) {
        return Err(core
            .t(
                "Optimizasyon işlemi için açık onay anahtarı eksik veya hatalı.",
                "The confirmation key for this optimization is missing or invalid.",
            )
            .into());
    }
    {
        let data = lock(&core.data);
        if core.config.snapshot_dir.is_some() || data["source"] == "replay" {
            return Err(core
                .t(
                    "Kayıt modunda sistem ve disk optimizasyonları kapalıdır.",
                    "System and drive optimizations are disabled in replay mode.",
                )
                .into());
        }
        if data["stale"] != false {
            return Err(core
                .t(
                    "Disk verileri güncel değil; önce SMART verilerini yenileyin.",
                    "Drive data is stale; refresh SMART data first.",
                )
                .into());
        }
    }
    if !matches!(req.action.as_str(), "query_op_bounds" | "autostart") {
        core.require_write_unlocked(&req.action)?;
    }
    if req.action != "autostart"
        && !(cfg!(target_os = "linux")
            && matches!(
                req.action.as_str(),
                "query_op_bounds" | "long_paths" | "ntfs_8dot3"
            ))
        && !platform::is_admin()
    {
        return Err(core
            .t(
                "Bu işlem için Yönetici / Polkit yetkisi gereklidir.",
                "Administrator / Polkit authorization is required for this operation.",
            )
            .into());
    }

    match req.action.as_str() {
        "autostart" => {
            if !matches!(req.value.as_str(), "enable" | "disable") {
                return Err(core
                    .t("Geçersiz başlangıç tercihi.", "Invalid startup preference.")
                    .into());
            }
            let enable = req.value == "enable";
            #[cfg(windows)]
            {
                set_autostart_registry(enable, en)?;
                Ok(json!({
                    "status": "completed",
                    "action": "autostart",
                    "enabled": enable,
                    "message": if enable {
                        core.t("SSD Sentinel Windows açılışında sistem tepsisinde başlatılacak.", "SSD Sentinel will start in the system tray when Windows starts.")
                    } else {
                        core.t("Windows açılışında otomatik başlatma kapatıldı.", "Automatic startup with Windows was turned off.")
                    }
                }))
            }
            #[cfg(target_os = "linux")]
            {
                set_autostart_xdg(enable, en)?;
                Ok(json!({
                    "status": "completed",
                    "action": "autostart",
                    "enabled": enable,
                    "message": if enable {
                        core.t("SSD Sentinel oturum açılışında (~/.config/autostart) başlatılacak.", "SSD Sentinel will start on desktop login (~/.config/autostart).")
                    } else {
                        core.t("Oturum açılışında otomatik başlatma kapatıldı.", "Desktop login autostart was turned off.")
                    }
                }))
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            {
                let _ = enable;
                Err("Desteklenmeyen işletim sistemi.".into())
            }
        }
        "write_cache" => {
            if !matches!(req.value.as_str(), "enable" | "disable" | "rollback") {
                return Err(core
                    .t(
                        "Geçersiz yazma önbelleği değeri.",
                        "Invalid write cache value.",
                    )
                    .into());
            }
            let mapped = verify_disk(core, &req.target)?;
            let num = mapped.os_disk_number.ok_or(core.t(
                "Fiziksel disk numarası doğrulanamadı.",
                "The physical drive number could not be verified.",
            ))?;
            let key = format!("write_cache:{}", req.target);
            let target_enable = if req.value == "rollback" {
                let saved = lock(&core.store).get_optimization(&key)?.ok_or(core.t(
                    "Geri alınacak yazma önbelleği kaydı bulunamadı.",
                    "No write cache record to roll back was found.",
                ))?;
                saved["previous_value"].as_str() == Some("enable")
            } else {
                req.value == "enable"
            };
            #[cfg(windows)]
            let prev = {
                let was_enabled = set_write_cache_ioctl(num, target_enable, en)?;
                if was_enabled {
                    "enable"
                } else {
                    "disable"
                }
            };
            #[cfg(target_os = "linux")]
            let prev = {
                let _ = num;
                let dev_name = linux_block_name(&mapped.device)?;
                let sysfs_path = format!("/sys/block/{dev_name}/queue/write_cache");
                let cur = std::fs::read_to_string(&sysfs_path)
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                let was = if cur == "write back" {
                    "enable"
                } else {
                    "disable"
                };
                let target_str = if target_enable {
                    "write back"
                } else {
                    "write through"
                };
                if crate::linux_priv::call(crate::linux_priv::HelperOp::WriteSysfs {
                    path: sysfs_path,
                    value: target_str.to_owned(),
                })
                .is_err()
                {
                    crate::linux_priv::call(crate::linux_priv::HelperOp::HdparmWriteCache {
                        device: mapped.device.clone(),
                        enable: target_enable,
                    })?;
                }
                was
            };
            #[cfg(not(any(windows, target_os = "linux")))]
            let prev = {
                let _ = (num, target_enable);
                return Err("Desteklenmeyen platform.".into());
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
                           "message": if en {
            format!("Hardware write cache for {} was {}.", mapped.model, if target_enable { "enabled" } else { "disabled" })
            } else {
            format!("{} için donanım yazma önbelleği {}.", mapped.model, if target_enable { "etkinleştirildi" } else { "kapatıldı" })
            }
                       }))
        }
        "ntfs_trim" => {
            if !matches!(req.value.as_str(), "enable" | "rollback") {
                return Err(core
                    .t("Geçersiz TRIM ayarı.", "Invalid TRIM setting.")
                    .into());
            }
            let key = "ntfs_trim:system";
            let current = lock(&core.data)["windows"]["trim_enabled"].as_bool();
            let prev = match current {
                Some(true) => "enable",
                Some(false) => "disable",
                None => "enable",
            };
            let target_enable = if req.value == "rollback" {
                let saved = lock(&core.store).get_optimization(key)?.ok_or(core.t(
                    "Geri alınacak TRIM kaydı bulunamadı.",
                    "No TRIM record to roll back was found.",
                ))?;
                saved["previous_value"].as_str() != Some("disable")
            } else {
                true
            };
            #[cfg(windows)]
            {
                let target_flag = if target_enable { "0" } else { "1" };
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
                    return Err(if en {
                        format!("fsutil failed (code {}): {}", out.code, out.stderr)
                    } else {
                        format!("fsutil başarısız (kod {}): {}", out.code, out.stderr)
                    });
                }
            }
            #[cfg(target_os = "linux")]
            {
                if target_enable {
                    let _ = crate::platform_linux::fstrim_volume("/");
                }
                let _ =
                    crate::linux_priv::call(crate::linux_priv::HelperOp::SystemctlFstrimTimer {
                        enable: target_enable,
                    });
            }
            if req.value == "rollback" {
                lock(&core.store).remove_optimization(key)?;
            } else {
                lock(&core.store).record_optimization(
                    key,
                    "system",
                    if cfg!(target_os = "linux") {
                        "Linux TRIM / fstrim.timer"
                    } else {
                        "Windows NTFS/ReFS TRIM (DisableDeleteNotify)"
                    },
                    prev,
                    "enable",
                )?;
            }
            Ok(json!({
                "status": "completed",
                "action": "ntfs_trim",
                "message": if target_enable {
                    core.t("Sistem TRIM (fstrim / TRIM) etkinleştirildi.", "System TRIM was enabled.")
                } else {
                    core.t("TRIM ayarı önceki değerine döndürüldü.", "The TRIM setting was reverted to its previous value.")
                }
            }))
        }
        "pcie_aspm" => {
            if !matches!(req.value.as_str(), "off" | "moderate" | "rollback") {
                return Err(core
                    .t("Geçersiz PCIe ASPM modu.", "Invalid PCIe ASPM mode.")
                    .into());
            }
            let key = "pcie_aspm:system";
            #[cfg(windows)]
            {
                let prev_idx = lock(&core.data)["windows"]["system_tuning"]["pcie_aspm"]
                    .as_u64()
                    .unwrap_or(1)
                    .to_string();
                let target_idx = if req.value == "rollback" {
                    let saved = lock(&core.store).get_optimization(key)?.ok_or(core.t(
                        "Geri alınacak PCIe ASPM kaydı bulunamadı.",
                        "No PCIe ASPM record to roll back was found.",
                    ))?;
                    let val = saved["previous_value"].as_str().unwrap_or("1").to_owned();
                    if !matches!(val.as_str(), "0" | "1" | "2") {
                        return Err(core
                            .t(
                                "Kayıtlı ASPM değeri geçersiz.",
                                "The saved ASPM value is invalid.",
                            )
                            .into());
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
                    return Err(if en {
                        format!("powercfg failed: {}", out.stderr)
                    } else {
                        format!("powercfg başarısız: {}", out.stderr)
                    });
                }
                let act = process::run(
                    &powercfg,
                    &["/setactive".into(), "SCHEME_CURRENT".into()],
                    Duration::from_secs(15),
                    &core.stop,
                )?;
                if act.code != 0 {
                    return Err(if en {
                        format!("Could not activate the power plan: {}", act.stderr)
                    } else {
                        format!("Güç planı etkinleştirilemedi: {}", act.stderr)
                    });
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
                    "message": core.t("Aktif güç planında PCIe ASPM ayarı güncellendi.", "The PCIe ASPM setting was updated in the active power plan.")
                }))
            }
            #[cfg(target_os = "linux")]
            {
                let (_, cur_policy) = crate::platform_linux::read_pcie_aspm_policy();
                let prev_policy = cur_policy.unwrap_or_else(|| "default".into());
                let target_policy = if req.value == "rollback" {
                    let saved = lock(&core.store).get_optimization(key)?.ok_or(core.t(
                        "Geri alınacak PCIe ASPM kaydı bulunamadı.",
                        "No PCIe ASPM record to roll back was found.",
                    ))?;
                    saved["previous_value"]
                        .as_str()
                        .unwrap_or("default")
                        .to_owned()
                } else if req.value == "off" {
                    "performance".into()
                } else {
                    "default".into()
                };
                crate::linux_priv::call(crate::linux_priv::HelperOp::WriteSysfs {
                    path: "/sys/module/pcie_aspm/parameters/policy".into(),
                    value: target_policy.clone(),
                })
                .map_err(|e| {
                    if en {
                        format!("Kernel/BIOS locked PCIe ASPM policy ({e}). Boot with pcie_aspm=force if needed.")
                    } else {
                        format!("Çekirdek/BIOS PCIe ASPM politikasını kilitlemiş ({e}). Gerekirse pcie_aspm=force çekirdek parametresi kullanın.")
                    }
                })?;
                if req.value == "rollback" {
                    lock(&core.store).remove_optimization(key)?;
                } else {
                    lock(&core.store).record_optimization(
                        key,
                        "system",
                        "Linux PCIe ASPM Politikası",
                        &prev_policy,
                        &target_policy,
                    )?;
                }
                Ok(json!({
                    "status": "completed",
                    "action": "pcie_aspm",
                    "value": target_policy,
                    "message": core.t("Linux PCIe ASPM politikası güncellendi.", "Linux PCIe ASPM policy updated.")
                }))
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            {
                let _ = key;
                Err("Desteklenmeyen platform.".into())
            }
        }
        "hibernation" => {
            if !matches!(req.value.as_str(), "off" | "on" | "rollback") {
                return Err(core
                    .t(
                        "Geçersiz hazırda bekletme / takas ayarı.",
                        "Invalid hibernation / swap tuning value.",
                    )
                    .into());
            }
            let key = "hibernation:system";
            #[cfg(windows)]
            {
                let current = lock(&core.data)["windows"]["system_tuning"]["hibernate_enabled"]
                    .as_bool()
                    .unwrap_or(true);
                let prev = if current { "on" } else { "off" };
                let target = if req.value == "rollback" {
                    let saved = lock(&core.store).get_optimization(key)?.ok_or(core.t(
                        "Geri alınacak hazırda bekletme kaydı bulunamadı.",
                        "No hibernation record to roll back was found.",
                    ))?;
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
                    return Err(if en {
                        format!("powercfg /h failed: {}", out.stderr)
                    } else {
                        format!("powercfg /h başarısız: {}", out.stderr)
                    });
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
                        core.t("Hazırda bekletme kapatıldı ve hiberfil.sys alanı geri kazanıldı.", "Hibernation was turned off and hiberfil.sys space was reclaimed.")
                    } else {
                        core.t("Hazırda bekletme etkinleştirildi.", "Hibernation was enabled.")
                    }
                }))
            }
            #[cfg(target_os = "linux")]
            {
                let prev_swap = crate::platform_linux::read_swappiness()
                    .unwrap_or(60)
                    .to_string();
                let target_swap = if req.value == "rollback" {
                    let saved = lock(&core.store).get_optimization(key)?.ok_or(core.t(
                        "Geri alınacak swappiness kaydı bulunamadı.",
                        "No swappiness record to roll back was found.",
                    ))?;
                    saved["previous_value"].as_str().unwrap_or("60").to_owned()
                } else if req.value == "off" {
                    "10".into()
                } else {
                    "60".into()
                };
                crate::linux_priv::call(crate::linux_priv::HelperOp::WriteSysfs {
                    path: "/proc/sys/vm/swappiness".into(),
                    value: target_swap.clone(),
                })?;
                if req.value == "rollback" {
                    lock(&core.store).remove_optimization(key)?;
                } else {
                    lock(&core.store).record_optimization(
                        key,
                        "system",
                        "Linux vm.swappiness (SSD yazma azaltma)",
                        &prev_swap,
                        &target_swap,
                    )?;
                }
                Ok(json!({
                    "status": "completed",
                    "action": "hibernation",
                    "value": target_swap,
                    "message": if en {
                        format!("Linux vm.swappiness set to {target_swap} (previous {prev_swap}).")
                    } else {
                        format!("Linux vm.swappiness değeri {target_swap} olarak ayarlandı (önceki {prev_swap}).")
                    }
                }))
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            {
                let _ = key;
                Err("Desteklenmeyen platform.".into())
            }
        }
        "hdd_apm" => {
            if !matches!(req.value.as_str(), "254" | "128" | "off" | "rollback") {
                return Err(core
                    .t("Geçersiz HDD APM seviyesi.", "Invalid HDD APM level.")
                    .into());
            }
            let mapped = verify_disk(core, &req.target)?;
            if mapped.kind != "HDD" {
                return Err(core
                    .t(
                        "APM kafa park kontrolü yalnız mekanik HDD'lerde sunulur.",
                        "APM head-park control is available on mechanical HDDs only.",
                    )
                    .into());
            }
            let key = format!("hdd_apm:{}", req.target);
            let level = if req.value == "rollback" {
                let saved = lock(&core.store).get_optimization(&key)?.ok_or(core.t(
                    "Geri alınacak HDD APM kaydı bulunamadı.",
                    "No HDD APM record to roll back was found.",
                ))?;
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
            );
            let accepted = res
                .as_ref()
                .is_ok_and(|v| v["smartctl"]["exit_status"].as_u64().unwrap_or(7) & 7 == 0);
            if !accepted {
                #[cfg(target_os = "linux")]
                {
                    let lvl_num = if level == "off" {
                        255u8
                    } else {
                        level.parse::<u8>().unwrap_or(254)
                    };
                    crate::linux_priv::call(crate::linux_priv::HelperOp::HdparmApm {
                        device: mapped.device.clone(),
                        level: lvl_num,
                    })?;
                }
                #[cfg(not(target_os = "linux"))]
                return Err(core
                    .t(
                        "Aygıt APM komutunu kabul etmedi.",
                        "The device did not accept the APM command.",
                    )
                    .into());
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
                "message": if en {
                    format!("APM level for {} set to '{level}'.", mapped.model)
                } else {
                    format!("{} için APM seviyesi '{level}' olarak ayarlandı.", mapped.model)
                }
            }))
        }
        "io_scheduler" => {
            #[cfg(target_os = "linux")]
            {
                if !matches!(
                    req.value.as_str(),
                    "none" | "mq-deadline" | "kyber" | "bfq" | "auto" | "rollback"
                ) {
                    return Err(core
                        .t(
                            "Geçersiz I/O zamanlayıcı değeri.",
                            "Invalid I/O scheduler value.",
                        )
                        .into());
                }
                let mapped = verify_disk(core, &req.target)?;
                let dev_name = linux_block_name(&mapped.device)?;
                let sysfs_path = format!("/sys/block/{dev_name}/queue/scheduler");
                let raw = std::fs::read_to_string(&sysfs_path)
                    .map_err(|e| format!("Zamanlayıcı okunamadı: {e}"))?;
                let (cur, avail) = crate::platform_linux::parse_scheduler_str(&raw);
                let prev = cur.unwrap_or_else(|| "none".into());
                let key = format!("io_scheduler:{}", req.target);
                let target_sched = if req.value == "rollback" {
                    let saved = lock(&core.store).get_optimization(&key)?.ok_or(core.t(
                        "Geri alınacak I/O zamanlayıcı kaydı bulunamadı.",
                        "No I/O scheduler record to roll back was found.",
                    ))?;
                    saved["previous_value"].as_str().unwrap_or(&prev).to_owned()
                } else if req.value == "auto" {
                    let candidate = if mapped.protocol.as_deref() == Some("NVMe") {
                        "none"
                    } else if mapped.kind == "HDD" {
                        "bfq"
                    } else {
                        "mq-deadline"
                    };
                    if avail.iter().any(|s| s == candidate) {
                        candidate.to_owned()
                    } else {
                        avail.first().cloned().unwrap_or_else(|| "none".into())
                    }
                } else {
                    if !avail.iter().any(|s| s == &req.value) {
                        return Err(format!(
                            "{} zamanlayıcısı bu disk tarafından sunulmuyor (mevcut: {}).",
                            req.value,
                            avail.join(", ")
                        ));
                    }
                    req.value.clone()
                };
                crate::linux_priv::call(crate::linux_priv::HelperOp::WriteSysfs {
                    path: sysfs_path,
                    value: target_sched.clone(),
                })?;
                if req.value == "rollback" {
                    lock(&core.store).remove_optimization(&key)?;
                } else {
                    lock(&core.store).record_optimization(
                        &key,
                        &req.target,
                        &format!("Linux I/O Zamanlayıcı · {}", mapped.model),
                        &prev,
                        &target_sched,
                    )?;
                }
                Ok(json!({
                    "status": "completed",
                    "action": "io_scheduler",
                    "value": target_sched,
                    "message": if en {
                        format!("I/O scheduler for {} set to '{target_sched}'.", mapped.model)
                    } else {
                        format!("{} için I/O zamanlayıcı '{target_sched}' olarak ayarlandı.", mapped.model)
                    }
                }))
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err("I/O zamanlayıcı seçimi yalnız Linux'ta desteklenir.".into())
            }
        }
        "query_op_bounds" => {
            let (disk_id, letter) = split_target(&req.target, en)?;
            let mapped = verify_disk(core, disk_id)?;
            if mapped.kind == "HDD" {
                return Err(core
                    .t(
                        "Over-Provisioning yalnız SSD'lerde desteklenir.",
                        "Over-Provisioning is supported on SSDs only.",
                    )
                    .into());
            }
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or(core.t(
                    "Seçilen bölüm diskte bulunamadı.",
                    "The selected volume was not found on the drive.",
                ))?;
            platform::check_filesystem(vol, false)?;
            #[cfg(windows)]
            let (size_min, size_max, size) = query_partition_bounds(core, letter)?;
            #[cfg(target_os = "linux")]
            let (size_min, size_max, size) = query_linux_bounds(letter)?;
            #[cfg(not(any(windows, target_os = "linux")))]
            let (size_min, size_max, size) = (0u64, 0u64, 0u64);
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
                        core.t(
                            "Ölü kuyruk koruma oranı %5, %10 veya %15 olmalıdır.",
                            "The dead-tail guard ratio must be 5%, 10%, or 15%.",
                        )
                        .into()
                    } else {
                        core.t(
                            "Over-Provisioning oranı %5, %7 veya %10 olmalıdır.",
                            "The Over-Provisioning ratio must be 5%, 7%, or 10%.",
                        )
                        .into()
                    })
                }
            };
            let (disk_id, letter) = split_target(&req.target, en)?;
            let mapped = verify_disk(core, disk_id)?;
            if !is_tail && mapped.kind == "HDD" {
                return Err(core
                    .t(
                        "Over-Provisioning yalnız SSD'lerde desteklenir.",
                        "Over-Provisioning is supported on SSDs only.",
                    )
                    .into());
            }
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or(core.t(
                    "Seçilen bölüm diskte bulunamadı.",
                    "The selected volume was not found on the drive.",
                ))?;
            platform::check_filesystem(vol, false)?;
            let disk_cap = mapped.metrics["capacity_bytes"]
                .as_u64()
                .or_else(|| {
                    mapped.metrics["capacity_bytes"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                })
                .ok_or(core.t(
                    "Disk toplam kapasitesi okunamadı.",
                    "Could not read the drive's total capacity.",
                ))?;
            let unallocated = mapped.optimization["unallocated_bytes"]
                .as_u64()
                .unwrap_or(0);
            let target_unalloc = disk_cap / 100 * pct;
            if unallocated >= target_unalloc {
                return Ok(json!({
                                   "status": "already_satisfied",
                                   "message": if en {
                format!("The drive already has {pct}% or more unallocated protection space at its end.")
                } else {
                format!("Disk sonunda zaten %{pct} veya daha fazla ayrılmamış koruma alanı mevcut.")
                }
                               }));
            }
            let delta = target_unalloc - unallocated;
            let vol_free = vol["free_bytes"].as_u64().unwrap_or(0);
            let min_keep_free = 10 * 1024 * 1024 * 1024u64;
            if vol_free.saturating_sub(delta) < min_keep_free {
                return Err(core
                    .t(
                        "Bölüm daraltıldıktan sonra en az 10 GiB boş alan kalmalıdır.",
                        "At least 10 GiB of free space must remain after shrinking the volume.",
                    )
                    .into());
            }
            #[cfg(windows)]
            let new_size = {
                let (size_min, _size_max, current_size) = query_partition_bounds(core, letter)?;
                let new_size =
                    ((current_size.saturating_sub(delta)) / (1024 * 1024)) * (1024 * 1024);
                let safety_min = size_min.saturating_add(64 * 1024 * 1024);
                if new_size < safety_min || new_size >= current_size {
                    return Err(
                        core.t("Taşınamayan sistem dosyaları nedeniyle bölüm istenen boyuta daraltılamıyor.", "The volume cannot be shrunk to the requested size because of unmovable system files.")
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
                new_size
            };
            #[cfg(target_os = "linux")]
            let new_size = {
                let (_, _, current_size) = query_linux_bounds(letter)?;
                let root = platform::volume_root(letter)?;
                let reserve_name = if is_tail {
                    ".sentinel-tail-guard"
                } else {
                    ".sentinel-op-reserve"
                };
                let reserve_file = root.join(reserve_name);
                let f = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&reserve_file)
                    .map_err(|e| {
                        format!(
                            "Koruma ayrılmış dosyası oluşturulamadı ({}): {e}",
                            reserve_file.display()
                        )
                    })?;
                // Allocate metadata reservation cleanly without burning NAND erase cycles.
                f.set_len(delta)
                    .map_err(|e| format!("Rezerv boyutu ayarlanamadı: {e}"))?;
                let _ = f.sync_all();
                drop(f);
                if mapped.kind != "HDD" {
                    let _ = crate::platform_linux::fstrim_volume(letter);
                }
                let new_size = current_size.saturating_sub(delta);
                let key = if is_tail {
                    format!("tail:{disk_id}:{letter}")
                } else {
                    format!("op:{disk_id}:{letter}")
                };
                let title = if is_tail {
                    format!("Ölü Kuyruk Koruması (%{pct}) · {letter}")
                } else {
                    format!("SSD Over-Provisioning Rezervi (%{pct}) · {letter}")
                };
                lock(&core.store).record_optimization(
                    &key,
                    &req.target,
                    &title,
                    &current_size.to_string(),
                    &new_size.to_string(),
                )?;
                new_size
            };
            #[cfg(not(any(windows, target_os = "linux")))]
            let new_size = 0u64;
            Ok(json!({
                "status": "completed",
                "action": req.action,
                "letter": letter,
                "new_size": new_size,
                "message": if is_tail {
                    if en {
                        format!("Volume {letter} reserved {pct}% dead-tail protection space.")
                    } else {
                        format!("{letter} bölümünde %{pct} ölü kuyruk koruma alanı ayrıldı.")
                    }
                } else if en {
                    format!("Volume {letter} reserved {pct}% Over-Provisioning space and ran TRIM.")
                } else {
                    format!("{letter} bölümünde %{pct} Over-Provisioning koruma alanı ayrıldı ve TRIM çalıştırıldı.")
                }
            }))
        }
        "reclaim_op" | "expand_tail" => {
            let is_tail = req.action == "expand_tail";
            let (disk_id, letter) = split_target(&req.target, en)?;
            let mapped = verify_disk(core, disk_id)?;
            if !is_tail && mapped.kind == "HDD" {
                return Err(core
                    .t(
                        "Over-Provisioning yalnız SSD'lerde desteklenir.",
                        "Over-Provisioning is supported on SSDs only.",
                    )
                    .into());
            }
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or(core.t(
                    "Seçilen bölüm diskte bulunamadı.",
                    "The selected volume was not found on the drive.",
                ))?;
            platform::check_filesystem(vol, false)?;
            #[cfg(windows)]
            let size_max = {
                let (_size_min, size_max, current_size) = query_partition_bounds(core, letter)?;
                if size_max <= current_size {
                    return Ok(json!({
                        "status": "already_satisfied",
                        "message": core.t("Bölüm arkasında geri katılabilecek ayrılmamış alan bulunmuyor.", "There is no unallocated space behind the volume that can be merged back.")
                    }));
                }
                let script = format!(
                    "$ErrorActionPreference='Stop'; \
                     Resize-Partition -DriveLetter '{letter}' -Size {size_max} -ErrorAction Stop"
                );
                run_ps(core, &script, 180)?;
                size_max
            };
            #[cfg(target_os = "linux")]
            let size_max = {
                let root = platform::volume_root(letter)?;
                let reserve_name = if is_tail {
                    ".sentinel-tail-guard"
                } else {
                    ".sentinel-op-reserve"
                };
                let reserve_file = root.join(reserve_name);
                if reserve_file.exists() {
                    std::fs::remove_file(&reserve_file).map_err(|e| e.to_string())?;
                } else {
                    return Ok(json!({
                        "status": "already_satisfied",
                        "message": core.t("Geri alınacak koruma rezervi bulunamadı.", "No protection reserve to reclaim was found.")
                    }));
                }
                let (_, size_max, _) = query_linux_bounds(letter)?;
                size_max
            };
            #[cfg(not(any(windows, target_os = "linux")))]
            let size_max = 0u64;
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
                "message": if en {
                    format!("Volume {letter} protection space was reclaimed ({size_max} bytes).")
                } else {
                    format!("{letter} bölümündeki koruma alanı geri kazanıldı ({size_max} bayt).")
                }
            }))
        }
        "release_quarantine" => {
            let (disk_id, letter) = split_target(&req.target, en)?;
            let mapped = verify_disk(core, disk_id)?;
            let vol = mapped
                .volumes
                .iter()
                .find(|v| v["letter"] == letter)
                .ok_or(core.t(
                    "Seçilen bölüm diskte bulunamadı.",
                    "The selected volume was not found on the drive.",
                ))?;
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
                           "message": if en {
            format!("The quarantine shield on {letter}: was removed ({removed} zone files cleaned).")
            } else {
            format!("{letter}: üzerindeki karantina kalkanı kaldırıldı ({removed} bölge dosyası temizlendi).")
            }
                       }))
        }
        "long_paths" => {
            if req.target != "system" {
                return Err(core
                    .t("Geçersiz uzun yol hedefi.", "Invalid long path target.")
                    .into());
            }
            if !matches!(req.value.as_str(), "enable" | "disable" | "rollback") {
                return Err(core
                    .t("Geçersiz uzun yol değeri.", "Invalid long path value.")
                    .into());
            }
            #[cfg(target_os = "linux")]
            {
                Ok(json!({
                    "status": "completed",
                    "action": "long_paths",
                    "enabled": true,
                    "message": core.t(
                        "Linux çekirdeğinde uzun dosya yolları (PATH_MAX = 4096) doğal olarak etkindir.",
                        "Long file paths (PATH_MAX = 4096) are natively enabled on Linux."
                    )
                }))
            }
            #[cfg(windows)]
            {
                let key = "long_paths:system";
                let target_enable = if req.value == "rollback" {
                    let saved = lock(&core.store).get_optimization(key)?.ok_or(core.t(
                        "Geri alınacak uzun yol kaydı bulunamadı.",
                        "No long path record to roll back was found.",
                    ))?;
                    saved["previous_value"].as_str() == Some("enable")
                } else {
                    req.value == "enable"
                };
                let prev = if set_long_paths_registry(target_enable, en)? {
                    "enable"
                } else {
                    "disable"
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
                        core.t("Windows uzun dosya yolu desteği (LongPathsEnabled) etkinleştirildi.", "Windows long path support (LongPathsEnabled) was enabled.")
                    } else {
                        core.t("Windows uzun dosya yolu desteği kapatıldı.", "Windows long path support was turned off.")
                    }
                }))
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            Err("Desteklenmeyen platform.".into())
        }
        "ntfs_8dot3" => {
            if req.target != "system" {
                return Err(core
                    .t("Geçersiz NTFS 8.3 hedefi.", "Invalid NTFS 8.3 target.")
                    .into());
            }
            if !matches!(req.value.as_str(), "disable" | "default" | "rollback") {
                return Err(core
                    .t("Geçersiz NTFS 8.3 ayarı.", "Invalid NTFS 8.3 setting.")
                    .into());
            }
            #[cfg(target_os = "linux")]
            {
                Ok(json!({
                    "status": "completed",
                    "action": "ntfs_8dot3",
                    "value": "native",
                    "message": core.t(
                        "Linux dosya sistemlerinde (ext4/btrfs/xfs) 8.3 kısa ad yükü zaten yoktur; ekstra node_modules hızı için /etc/fstab içinde noatime/relatime kullanabilirsiniz.",
                        "Linux file systems (ext4/btrfs/xfs) have zero 8.3 short-name overhead; use noatime/relatime in /etc/fstab for extra node_modules performance."
                    )
                }))
            }
            #[cfg(windows)]
            {
                let key = "ntfs_8dot3:system";
                let prev_idx = lock(&core.data)["windows"]["system_tuning"]["ntfs_8dot3_creation"]
                    .as_u64()
                    .filter(|v| *v <= 3)
                    .unwrap_or(2)
                    .to_string();
                let target_idx = if req.value == "rollback" {
                    let saved = lock(&core.store).get_optimization(key)?.ok_or(core.t(
                        "Geri alınacak NTFS 8.3 kaydı bulunamadı.",
                        "No NTFS 8.3 record to roll back was found.",
                    ))?;
                    let val = saved["previous_value"].as_str().unwrap_or("2").to_owned();
                    if !matches!(val.as_str(), "0" | "1" | "2" | "3") {
                        return Err(core
                            .t(
                                "Kayıtlı NTFS 8.3 değeri geçersiz.",
                                "The saved NTFS 8.3 value is invalid.",
                            )
                            .into());
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
                    return Err(if en {
                        format!(
                            "fsutil disable8dot3 failed (code {}): {}",
                            out.code, out.stderr
                        )
                    } else {
                        format!(
                            "fsutil disable8dot3 başarısız (kod {}): {}",
                            out.code, out.stderr
                        )
                    });
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
                        core.t("NTFS 8.3 kısa dosya adı üretimi tüm birimlerde kapatıldı (node_modules / target hızlandırması).", "NTFS 8.3 short file name generation was turned off on all volumes (speeds up node_modules / target).")
                    } else {
                        core.t("NTFS 8.3 kısa dosya adı ayarı güncellendi.", "The NTFS 8.3 short file name setting was updated.")
                    }
                }))
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            Err("Desteklenmeyen platform.".into())
        }
        "compact_wsl_vhdx" => {
            if req.value != "compact" {
                return Err(core
                    .t(
                        "Geçersiz VHDX sıkıştırma komutu.",
                        "Invalid VHDX compaction command.",
                    )
                    .into());
            }
            let valid_path = crate::dev_workspace::validate_wsl_vhdx_path(&req.target)?;
            #[cfg(target_os = "linux")]
            {
                let size_before = std::fs::metadata(&valid_path)
                    .map(|m| m.len())
                    .map_err(|e| e.to_string())?;
                if valid_path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("qcow2"))
                {
                    let qimg =
                        crate::linux_priv::find_trusted_binary(&["qemu-img"]).ok_or_else(|| {
                            core.t(
                                "qcow2 sıkıştırması için qemu-img kurulu olmalıdır.",
                                "qemu-img must be installed to compact qcow2 images.",
                            )
                            .to_owned()
                        })?;
                    let tmp = valid_path.with_extension("qcow2.sentinel-compact");
                    let out = process::run(
                        &qimg,
                        &[
                            "convert".into(),
                            "-O".into(),
                            "qcow2".into(),
                            "-c".into(),
                            valid_path.to_string_lossy().into_owned(),
                            tmp.to_string_lossy().into_owned(),
                        ],
                        Duration::from_secs(600),
                        &core.stop,
                    )?;
                    if out.code != 0 {
                        let _ = std::fs::remove_file(&tmp);
                        return Err(format!(
                            "qemu-img başarısız (kod {}): {}",
                            out.code, out.stderr
                        ));
                    }
                    std::fs::rename(&tmp, &valid_path).map_err(|e| {
                        let _ = std::fs::remove_file(&tmp);
                        e.to_string()
                    })?;
                } else if let Some(falloc) = crate::linux_priv::find_trusted_binary(&["fallocate"])
                {
                    let out = process::run(
                        &falloc,
                        &[
                            "--dig-holes".into(),
                            valid_path.to_string_lossy().into_owned(),
                        ],
                        Duration::from_secs(300),
                        &core.stop,
                    )?;
                    if out.code != 0 {
                        return Err(format!("fallocate --dig-holes başarısız: {}", out.stderr));
                    }
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
                    "message": if en {
                        format!("{} compacted ({:.2} GiB → {:.2} GiB).", valid_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), size_before as f64 / (1024.0 * 1024.0 * 1024.0), size_after as f64 / (1024.0 * 1024.0 * 1024.0))
                    } else {
                        format!("{} sıkıştırıldı ({:.2} GiB → {:.2} GiB).", valid_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), size_before as f64 / (1024.0 * 1024.0 * 1024.0), size_after as f64 / (1024.0 * 1024.0 * 1024.0))
                    }
                }))
            }
            #[cfg(windows)]
            {
                use std::io::Write;
                let size_before =
                    std::fs::metadata(&valid_path)
                        .map(|m| m.len())
                        .map_err(|_| {
                            core.t(
                                "VHDX dosyası boyutu okunamadı.",
                                "Could not read the VHDX file size.",
                            )
                        })?;
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
                let mut script =
                    tempfile::Builder::new()
                        .suffix(".txt")
                        .tempfile()
                        .map_err(|e| {
                            if en {
                                format!("Could not create the temporary diskpart script: {e}")
                            } else {
                                format!("diskpart geçici betiği oluşturulamadı: {e}")
                            }
                        })?;
                let body = format!(
                    "select vdisk file=\"{}\"\r\nattach vdisk readonly\r\ncompact vdisk\r\ndetach vdisk\r\nexit\r\n",
                    valid_path.display()
                );
                script
                    .write_all(body.as_bytes())
                    .and_then(|()| script.flush())
                    .map_err(|e| {
                        if en {
                            format!("Could not write the diskpart script: {e}")
                        } else {
                            format!("diskpart betiği yazılamadı: {e}")
                        }
                    })?;
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
                    return Err(if en {
                        format!(
                            "diskpart compact failed (code {}): {}",
                            out.code,
                            detail.chars().take(600).collect::<String>()
                        )
                    } else {
                        format!(
                            "diskpart compact başarısız (kod {}): {}",
                            out.code,
                            detail.chars().take(600).collect::<String>()
                        )
                    });
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
                                   "message": if en {
                format!("{} compacted ({:.2} GiB → {:.2} GiB · {:.2} GiB reclaimed).", valid_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                       size_before as f64 / (1024.0 * 1024.0 * 1024.0),
                                       size_after as f64 / (1024.0 * 1024.0 * 1024.0),
                                       freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0))
                } else {
                format!("{} sıkıştırıldı ({:.2} GiB → {:.2} GiB · {:.2} GiB geri kazanıldı).", valid_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                       size_before as f64 / (1024.0 * 1024.0 * 1024.0),
                                       size_after as f64 / (1024.0 * 1024.0 * 1024.0),
                                       freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0))
                }
                               }))
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            {
                let _ = valid_path;
                Err(core
                    .t(
                        "Bu işlem bu platformda desteklenmiyor.",
                        "This operation is not supported on this platform.",
                    )
                    .into())
            }
        }
        _ => Err(core
            .t(
                "Desteklenmeyen optimizasyon işlemi.",
                "Unsupported optimization action.",
            )
            .into()),
    }
}
