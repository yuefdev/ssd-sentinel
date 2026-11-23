use crate::{
    model::{Alert, Disk},
    process,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub fn system_executable(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
        .join("System32")
        .join(name)
}

pub fn is_admin() -> bool {
    #[cfg(windows)]
    {
        unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn powershell(
    mode: &str,
    args: &[String],
    cancel: &AtomicBool,
    seconds: u64,
) -> Result<Value, String> {
    if !matches!(mode, "Inventory" | "Performance" | "ReTrim") {
        return Err("Geçersiz Windows sorgusu.".into());
    }
    let mut file = tempfile::Builder::new()
        .suffix(".ps1")
        .tempfile()
        .map_err(|e| e.to_string())?;
    file.write_all(include_bytes!("../../tools/windows_metrics.ps1"))
        .map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    let mut command: Vec<String> = [
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    command.extend([
        file.path().to_string_lossy().into_owned(),
        "-Mode".into(),
        mode.into(),
    ]);
    command.extend_from_slice(args);
    let result = process::run(
        &system_executable("WindowsPowerShell/v1.0/powershell.exe"),
        &command,
        Duration::from_secs(seconds),
        cancel,
    )?;
    if result.code != 0 {
        return Err(result.stderr.chars().take(1200).collect());
    }
    let mut value: Value = serde_json::from_str(result.stdout.trim())
        .map_err(|e| format!("Windows çıktısı okunamadı: {e}"))?;
    if mode == "Inventory" {
        let output = value["trim_output"].as_str().unwrap_or("");
        value["trim_enabled"] = output
            .lines()
            .find(|s| s.contains("NTFS DisableDeleteNotify"))
            .and_then(|s| s.split('=').nth(1))
            .and_then(|s| s.trim().chars().next())
            .filter(|c| matches!(c, '0' | '1'))
            .map(|c| json!(c == '0'))
            .unwrap_or(Value::Null);
    }
    Ok(value)
}

pub fn smart(exe: &Path, args: &[String], cancel: &AtomicBool) -> Result<Value, String> {
    let mut command = vec!["-j".into()];
    command.extend_from_slice(args);
    let output = process::run(exe, &command, Duration::from_secs(45), cancel)?;
    let mut value: Value = serde_json::from_str(output.stdout.trim())
        .map_err(|_| format!("smartctl geçerli JSON döndürmedi (kod {}).", output.code))?;
    if !value.is_object() {
        return Err("SMART çıktısı nesne değil.".into());
    }
    if !value["smartctl"].is_object() {
        value["smartctl"] = json!({});
    }
    value["smartctl"]["exit_status"] = json!(output.code);
    Ok(value)
}

#[cfg(windows)]
mod win32 {
    use super::*;
    use std::collections::BTreeMap;
    use std::ptr::{null, null_mut};
    use windows_sys::core::GUID;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_Parent, SetupDiDestroyDeviceInfoList,
        SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
        SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
    };
    use windows_sys::Win32::Devices::Properties::DEVPROP_TYPE_UINT32;
    use windows_sys::Win32::Foundation::{
        CloseHandle, LocalFree, DEVPROPKEY, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        FILE_SHARE_READ, FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        DISK_PERFORMANCE, DRIVE_LAYOUT_INFORMATION_EX, GUID_DEVINTERFACE_DISK,
        IOCTL_DISK_GET_DRIVE_LAYOUT_EX, IOCTL_DISK_PERFORMANCE, IOCTL_STORAGE_GET_DEVICE_NUMBER,
        IOCTL_STORAGE_QUERY_PROPERTY, PARTITION_INFORMATION_EX, STORAGE_DEVICE_DESCRIPTOR,
        STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY, STORAGE_WRITE_CACHE_PROPERTY,
        VOLUME_DISK_EXTENTS,
    };
    use windows_sys::Win32::System::Power::{
        GetPwrCapabilities, PowerGetActiveScheme, PowerReadACValueIndex, SYSTEM_POWER_CAPABILITIES,
    };
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_HANDLE,
        SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const FILE_DEVICE_DISK: u32 = 7;
    const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub struct OwnedHandle(pub HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct OwnedDevInfo(HDEVINFO);
    impl Drop for OwnedDevInfo {
        fn drop(&mut self) {
            if self.0 != 0 && self.0 != -1isize {
                unsafe {
                    SetupDiDestroyDeviceInfoList(self.0);
                }
            }
        }
    }

    struct OwnedService(SC_HANDLE);
    impl Drop for OwnedService {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseServiceHandle(self.0);
                }
            }
        }
    }

    pub fn open_device(path: &str, access: u32) -> Option<OwnedHandle> {
        let w = wide(path);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE || h.is_null() {
            None
        } else {
            Some(OwnedHandle(h))
        }
    }

    fn utf16_z(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }

    fn c_str(buf: &[u8], offset: u32) -> String {
        let off = offset as usize;
        if off == 0 || off >= buf.len() {
            return String::new();
        }
        let end = buf[off..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| off + p)
            .unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[off..end]).trim().to_owned()
    }

    fn read_reg_dword(
        hkey: windows_sys::Win32::System::Registry::HKEY,
        sub: &str,
        name: &str,
    ) -> Option<u32> {
        let sub_w = wide(sub);
        let name_w = wide(name);
        let mut val: u32 = 0;
        let mut size: u32 = std::mem::size_of::<u32>() as u32;
        let rc = unsafe {
            RegGetValueW(
                hkey,
                sub_w.as_ptr(),
                name_w.as_ptr(),
                RRF_RT_REG_DWORD,
                null_mut(),
                (&mut val as *mut u32).cast(),
                &mut size,
            )
        };
        (rc == 0).then_some(val)
    }

    fn reg_sz_exists(
        hkey: windows_sys::Win32::System::Registry::HKEY,
        sub: &str,
        name: &str,
    ) -> bool {
        read_reg_sz(hkey, sub, name).is_some()
    }

    fn read_reg_sz(
        hkey: windows_sys::Win32::System::Registry::HKEY,
        sub: &str,
        name: &str,
    ) -> Option<String> {
        let sub_w = wide(sub);
        let name_w = wide(name);
        let mut buf = [0u16; 512];
        let mut size: u32 = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                hkey,
                sub_w.as_ptr(),
                name_w.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc != 0 || size <= 2 {
            return None;
        }
        let chars = (size as usize / 2).min(buf.len());
        let text = utf16_z(&buf[..chars]);
        (!text.is_empty()).then_some(text)
    }

    fn service_running(name: &str) -> Option<bool> {
        let scm = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
        if scm.is_null() {
            return None;
        }
        let _scm = OwnedService(scm);
        let name_w = wide(name);
        let svc = unsafe { OpenServiceW(scm, name_w.as_ptr(), SERVICE_QUERY_STATUS) };
        if svc.is_null() {
            return None;
        }
        let _svc = OwnedService(svc);
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        let ok = unsafe { QueryServiceStatus(svc, &mut status) };
        (ok != 0).then_some(status.dwCurrentState == SERVICE_RUNNING)
    }

    fn read_aspm_ac() -> Option<u32> {
        let mut scheme: *mut GUID = null_mut();
        if unsafe { PowerGetActiveScheme(null_mut(), &mut scheme) } != 0 || scheme.is_null() {
            return None;
        }
        let sub_pcie = GUID::from_u128(0x501a4d13_42af_4429_9fd1_a8218c268e20);
        let aspm = GUID::from_u128(0xee12f906_d277_404b_b6da_e5fa1a576df5);
        let mut val: u32 = 0;
        let rc = unsafe { PowerReadACValueIndex(null_mut(), scheme, &sub_pcie, &aspm, &mut val) };
        unsafe {
            LocalFree(scheme.cast());
        }
        (rc == 0).then_some(val)
    }

    fn devnode_u32(inst: u32, pid: u32) -> Option<u32> {
        let key = DEVPROPKEY {
            fmtid: GUID::from_u128(0x3AB22E31_8264_4B4E_9AF5_A8D2D8E33E62),
            pid,
        };
        let mut ptype = 0u32;
        let mut buf = [0u8; 4];
        let mut len = 4u32;
        let rc = unsafe {
            CM_Get_DevNode_PropertyW(inst, &key, &mut ptype, buf.as_mut_ptr(), &mut len, 0)
        };
        (rc == 0 && ptype == DEVPROP_TYPE_UINT32 && len == 4).then(|| u32::from_ne_bytes(buf))
    }

    fn query_pcie_parent(dev_inst: u32) -> Option<Value> {
        let mut current = dev_inst;
        for _ in 0..3 {
            let mut parent = 0u32;
            if unsafe { CM_Get_Parent(&mut parent, current, 0) } != 0 {
                return None;
            }
            if let (Some(cg), Some(cw), Some(mg), Some(mw)) = (
                devnode_u32(parent, 9),
                devnode_u32(parent, 10),
                devnode_u32(parent, 11),
                devnode_u32(parent, 12),
            ) {
                if cg > 0 && cw > 0 && mg > 0 && mw > 0 {
                    return Some(json!({
                        "current_gen": cg,
                        "current_width": cw,
                        "max_gen": mg,
                        "max_width": mw
                    }));
                }
            }
            current = parent;
        }
        None
    }

    fn query_write_cache(h: HANDLE) -> Value {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: 4, // StorageDeviceWriteCacheProperty
            QueryType: 0,
            AdditionalParameters: [0],
        };
        let mut buf = [0u8; 64];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut ret,
                null_mut(),
            )
        };
        if ok == 0 || (ret as usize) < std::mem::size_of::<STORAGE_WRITE_CACHE_PROPERTY>() {
            return Value::Null;
        }
        let prop: STORAGE_WRITE_CACHE_PROPERTY =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
        let cache_type = match prop.WriteCacheType {
            1 => "none",
            2 => "write_back",
            3 => "write_through",
            _ => "unknown",
        };
        let enabled = match prop.WriteCacheEnabled {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        };
        let changeable = prop.WriteCacheChangeable == 2;
        json!({
            "type": cache_type,
            "enabled": enabled,
            "changeable": changeable,
            "flush_supported": prop.FlushCacheSupported
        })
    }

    fn query_layout(h: HANDLE) -> Value {
        let mut buf = vec![0u8; 16384];
        let mut ret = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_DISK_GET_DRIVE_LAYOUT_EX,
                null(),
                0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut ret,
                null_mut(),
            )
        };
        let header_size = std::mem::offset_of!(DRIVE_LAYOUT_INFORMATION_EX, PartitionEntry);
        if ok == 0 || (ret as usize) < header_size {
            return Value::Null;
        }
        let style = u32::from_ne_bytes(buf[0..4].try_into().unwrap_or([0; 4]));
        let count = u32::from_ne_bytes(buf[4..8].try_into().unwrap_or([0; 4])) as usize;
        let entry_size = std::mem::size_of::<PARTITION_INFORMATION_EX>();
        let mut allocated_bytes: u64 = 0;
        let mut misaligned_partitions: u64 = 0;
        let mut active_partitions: u64 = 0;
        for i in 0..count {
            let off = header_size + i * entry_size;
            if off + entry_size > ret as usize {
                break;
            }
            let part: PARTITION_INFORMATION_EX =
                unsafe { std::ptr::read_unaligned(buf[off..].as_ptr().cast()) };
            if part.PartitionLength <= 0 {
                continue;
            }
            let is_active = if style == 1 {
                let gpt = unsafe { part.Anonymous.Gpt };
                gpt.PartitionType.data1 != 0
                    || gpt.PartitionType.data2 != 0
                    || gpt.PartitionType.data3 != 0
                    || gpt.PartitionType.data4 != [0; 8]
            } else if style == 0 {
                part.PartitionNumber > 0
            } else {
                false
            };
            if !is_active {
                continue;
            }
            active_partitions += 1;
            allocated_bytes = allocated_bytes.saturating_add(part.PartitionLength as u64);
            if part.StartingOffset < 0 || !(part.StartingOffset as u64).is_multiple_of(4096) {
                misaligned_partitions += 1;
            }
        }
        json!({
            "style": match style { 0 => "MBR", 1 => "GPT", _ => "RAW" },
            "partition_count": active_partitions,
            "allocated_bytes": allocated_bytes,
            "misaligned_partitions": misaligned_partitions
        })
    }

    pub fn inventory(cancel: &AtomicBool) -> Result<Value, String> {
        if cancel.load(Ordering::SeqCst) {
            return Err(process::CANCELLED.into());
        }
        let mut volumes_by_disk: BTreeMap<u32, Vec<Value>> = BTreeMap::new();
        let mask = unsafe { GetLogicalDrives() };
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = ((b'A' + i as u8) as char).to_string();
            let root_w = wide(&format!("{letter}:\\"));
            if unsafe { GetDriveTypeW(root_w.as_ptr()) } != 3 {
                continue;
            }
            let mut label_buf = [0u16; 256];
            let mut fs_buf = [0u16; 64];
            let vol_ok = unsafe {
                GetVolumeInformationW(
                    root_w.as_ptr(),
                    label_buf.as_mut_ptr(),
                    label_buf.len() as u32,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    fs_buf.as_mut_ptr(),
                    fs_buf.len() as u32,
                )
            };
            let label = if vol_ok != 0 {
                let s = utf16_z(&label_buf);
                if s.is_empty() {
                    Value::Null
                } else {
                    json!(s)
                }
            } else {
                Value::Null
            };
            let filesystem = if vol_ok != 0 {
                utf16_z(&fs_buf)
            } else {
                String::new()
            };
            let mut free_avail = 0u64;
            let mut total_bytes = 0u64;
            let mut total_free = 0u64;
            let sp_ok = unsafe {
                GetDiskFreeSpaceExW(
                    root_w.as_ptr(),
                    &mut free_avail,
                    &mut total_bytes,
                    &mut total_free,
                )
            };
            let Some(vh) = open_device(&format!("\\\\.\\{letter}:"), 0) else {
                continue;
            };
            let mut ext_buf = [0u8; 512];
            let mut ext_ret = 0u32;
            let ext_ok = unsafe {
                DeviceIoControl(
                    vh.0,
                    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
                    null(),
                    0,
                    ext_buf.as_mut_ptr().cast(),
                    ext_buf.len() as u32,
                    &mut ext_ret,
                    null_mut(),
                )
            };
            if ext_ok == 0 || (ext_ret as usize) < std::mem::size_of::<VOLUME_DISK_EXTENTS>() {
                continue;
            }
            let ext: VOLUME_DISK_EXTENTS =
                unsafe { std::ptr::read_unaligned(ext_buf.as_ptr().cast()) };
            if ext.NumberOfDiskExtents != 1 {
                continue;
            }
            let disk_num = ext.Extents[0].DiskNumber;
            volumes_by_disk.entry(disk_num).or_default().push(json!({
                "letter": letter,
                "label": label,
                "filesystem": filesystem,
                "size_bytes": if sp_ok != 0 { json!(total_bytes) } else { Value::Null },
                "free_bytes": if sp_ok != 0 { json!(total_free) } else { Value::Null }
            }));
        }

        let dev_info = unsafe {
            SetupDiGetClassDevsW(
                &GUID_DEVINTERFACE_DISK,
                null(),
                null_mut(),
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        if dev_info == -1isize || dev_info == 0 {
            return Err("Fiziksel disk arayüzleri listelenemedi.".into());
        }
        let _dev_info = OwnedDevInfo(dev_info);
        let mut disks = Vec::new();
        let mut index = 0u32;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(process::CANCELLED.into());
            }
            let mut if_data: SP_DEVICE_INTERFACE_DATA = unsafe { std::mem::zeroed() };
            if_data.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if unsafe {
                SetupDiEnumDeviceInterfaces(
                    dev_info,
                    null(),
                    &GUID_DEVINTERFACE_DISK,
                    index,
                    &mut if_data,
                )
            } == 0
            {
                break;
            }
            index += 1;
            let mut req_size = 0u32;
            unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &if_data,
                    null_mut(),
                    0,
                    &mut req_size,
                    null_mut(),
                );
            }
            if req_size < std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
                continue;
            }
            let mut detail_buf = vec![0u8; req_size as usize];
            let cb_size = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            detail_buf[0..4].copy_from_slice(&cb_size.to_ne_bytes());
            let mut dev_data: SP_DEVINFO_DATA = unsafe { std::mem::zeroed() };
            dev_data.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    dev_info,
                    &if_data,
                    detail_buf.as_mut_ptr().cast(),
                    req_size,
                    null_mut(),
                    &mut dev_data,
                )
            } == 0
            {
                continue;
            }
            let path_off = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
            let u16_slice: Vec<u16> = detail_buf[path_off..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_ne_bytes(*c))
                .take_while(|&ch| ch != 0)
                .collect();
            let dev_path = String::from_utf16_lossy(&u16_slice);
            let Some(dh) = open_device(&dev_path, 0) else {
                continue;
            };
            let mut sdn: STORAGE_DEVICE_NUMBER = unsafe { std::mem::zeroed() };
            let mut ret = 0u32;
            let sdn_ok = unsafe {
                DeviceIoControl(
                    dh.0,
                    IOCTL_STORAGE_GET_DEVICE_NUMBER,
                    null(),
                    0,
                    (&mut sdn as *mut STORAGE_DEVICE_NUMBER).cast(),
                    std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
                    &mut ret,
                    null_mut(),
                )
            };
            if sdn_ok == 0 || sdn.DeviceType != FILE_DEVICE_DISK {
                continue;
            }
            let query = STORAGE_PROPERTY_QUERY {
                PropertyId: 0, // StorageDeviceProperty
                QueryType: 0,
                AdditionalParameters: [0],
            };
            let mut desc_buf = vec![0u8; 2048];
            let mut desc_ret = 0u32;
            let desc_ok = unsafe {
                DeviceIoControl(
                    dh.0,
                    IOCTL_STORAGE_QUERY_PROPERTY,
                    (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                    std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                    desc_buf.as_mut_ptr().cast(),
                    desc_buf.len() as u32,
                    &mut desc_ret,
                    null_mut(),
                )
            };
            if desc_ok == 0
                || (desc_ret as usize) < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>()
            {
                continue;
            }
            let desc: STORAGE_DEVICE_DESCRIPTOR =
                unsafe { std::ptr::read_unaligned(desc_buf.as_ptr().cast()) };
            let desc_slice = &desc_buf[..desc_ret as usize];
            let vendor = c_str(desc_slice, desc.VendorIdOffset);
            let product = c_str(desc_slice, desc.ProductIdOffset);
            let serial = c_str(desc_slice, desc.SerialNumberOffset);
            let model = if !vendor.is_empty()
                && !product
                    .to_ascii_lowercase()
                    .starts_with(&vendor.to_ascii_lowercase())
            {
                format!("{vendor} {product}")
            } else {
                product
            }
            .trim()
            .to_owned();
            let bus_type = desc.BusType;
            let mut disk_bytes: Option<u64> = None;
            let mut len_val: i64 = 0;
            let mut len_ret = 0u32;
            if unsafe {
                DeviceIoControl(
                    dh.0,
                    IOCTL_DISK_GET_LENGTH_INFO,
                    null(),
                    0,
                    (&mut len_val as *mut i64).cast(),
                    std::mem::size_of::<i64>() as u32,
                    &mut len_ret,
                    null_mut(),
                )
            } != 0
                && len_val > 0
            {
                disk_bytes = Some(len_val as u64);
            }
            let pcie = if bus_type == 17 {
                query_pcie_parent(dev_data.DevInst).unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            let write_cache = query_write_cache(dh.0);
            let layout = query_layout(dh.0);
            let vols = volumes_by_disk
                .remove(&sdn.DeviceNumber)
                .unwrap_or_default();
            disks.push(json!({
                "number": sdn.DeviceNumber,
                "serial": serial,
                "model": model,
                "bus_type": bus_type,
                "disk_bytes": disk_bytes,
                "volumes": vols,
                "pcie": pcie,
                "write_cache": write_cache,
                "layout": layout
            }));
        }
        disks.sort_by_key(|d| d["number"].as_u64().unwrap_or(u64::MAX));

        let trim_val = read_reg_dword(
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Control\\FileSystem",
            "DisableDeleteNotification",
        )
        .or_else(|| {
            read_reg_dword(
                HKEY_LOCAL_MACHINE,
                "SYSTEM\\CurrentControlSet\\Control\\FileSystem",
                "DisableDeleteNotify",
            )
        });
        let trim_enabled = trim_val.map(|v| v == 0);
        let mut pwr_caps: SYSTEM_POWER_CAPABILITIES = unsafe { std::mem::zeroed() };
        let hiber_cap = unsafe { GetPwrCapabilities(&mut pwr_caps) };
        let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let hiber_path = PathBuf::from(format!("{sys_drive}\\hiberfil.sys"));
        let hiber_bytes = std::fs::metadata(&hiber_path).ok().map(|m| m.len());
        let hibernate_enabled = if hiber_cap {
            Some(pwr_caps.HiberFilePresent || hiber_bytes.is_some())
        } else {
            hiber_bytes.map(|_| true)
        };
        let sysmain_running = service_running("SysMain");
        let wsearch_running = service_running("WSearch");
        let pcie_aspm = read_aspm_ac();
        let autostart_enabled = reg_sz_exists(
            HKEY_CURRENT_USER,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "SSDSentinel",
        );
        let long_paths_enabled = read_reg_dword(
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Control\\FileSystem",
            "LongPathsEnabled",
        )
        .map(|v| v != 0);
        let ntfs_8dot3_creation = read_reg_dword(
            HKEY_LOCAL_MACHINE,
            "SYSTEM\\CurrentControlSet\\Control\\FileSystem",
            "NtfsDisable8dot3NameCreation",
        );
        let dev_drive_supported = read_reg_sz(
            HKEY_LOCAL_MACHINE,
            "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
            "CurrentBuildNumber",
        )
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|build| build >= 22621);

        Ok(json!({
            "disks": disks,
            "trim_enabled": trim_enabled,
            "system_tuning": {
                "trim_enabled": trim_enabled,
                "hibernate_enabled": hibernate_enabled,
                "hiberfil_bytes": hiber_bytes,
                "sysmain_running": sysmain_running,
                "wsearch_running": wsearch_running,
                "pcie_aspm": pcie_aspm,
                "autostart_enabled": autostart_enabled,
                "long_paths_enabled": long_paths_enabled,
                "ntfs_8dot3_creation": ntfs_8dot3_creation,
                "dev_drive_supported": dev_drive_supported
            },
            "error": null
        }))
    }

    pub fn performance(previous: &mut Value, cancel: &AtomicBool) -> Result<Value, String> {
        if cancel.load(Ordering::SeqCst) {
            return Err(process::CANCELLED.into());
        }
        let mut raw_disks = Vec::new();
        let mut out_disks = Vec::new();
        for n in 0..32u32 {
            let Some(h) = open_device(&format!("\\\\.\\PhysicalDrive{n}"), 0) else {
                continue;
            };
            let mut perf: DISK_PERFORMANCE = unsafe { std::mem::zeroed() };
            let mut ret = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    h.0,
                    IOCTL_DISK_PERFORMANCE,
                    null(),
                    0,
                    (&mut perf as *mut DISK_PERFORMANCE).cast(),
                    std::mem::size_of::<DISK_PERFORMANCE>() as u32,
                    &mut ret,
                    null_mut(),
                )
            };
            if ok == 0 || (ret as usize) < std::mem::size_of::<DISK_PERFORMANCE>() {
                continue;
            }
            let name = format!("{n}");
            let read_timer = json!({
                "ticks": perf.ReadTime.max(0).to_string(),
                "operations": perf.ReadCount.to_string(),
                "frequency": "10000000"
            });
            let write_timer = json!({
                "ticks": perf.WriteTime.max(0).to_string(),
                "operations": perf.WriteCount.to_string(),
                "frequency": "10000000"
            });
            let curr_raw = json!({
                "number": n,
                "name": name,
                "bytes_read": perf.BytesRead.max(0),
                "bytes_written": perf.BytesWritten.max(0),
                "read_count": perf.ReadCount,
                "write_count": perf.WriteCount,
                "idle_time": perf.IdleTime.max(0),
                "query_time": perf.QueryTime.max(0),
                "queue_length": perf.QueueDepth,
                "read_timer": read_timer,
                "write_timer": write_timer
            });
            let old = previous["disks"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["number"] == n);
            let (read_bps, write_bps, read_iops, write_iops, busy_percent, read_lat, write_lat) =
                if let Some(old) = old {
                    let dq = perf
                        .QueryTime
                        .saturating_sub(old["query_time"].as_i64().unwrap_or(0));
                    if dq > 0 {
                        let dt = dq as f64 / 10_000_000.0;
                        let dr = (perf.BytesRead as u64)
                            .saturating_sub(old["bytes_read"].as_u64().unwrap_or(0));
                        let dw = (perf.BytesWritten as u64)
                            .saturating_sub(old["bytes_written"].as_u64().unwrap_or(0));
                        let drc = perf
                            .ReadCount
                            .saturating_sub(old["read_count"].as_u64().unwrap_or(0) as u32);
                        let dwc = perf
                            .WriteCount
                            .saturating_sub(old["write_count"].as_u64().unwrap_or(0) as u32);
                        let di = (perf.IdleTime as u64)
                            .checked_sub(old["idle_time"].as_u64().unwrap_or(0))
                            .unwrap_or(dq as u64);
                        let busy = ((1.0 - (di as f64 / dq as f64)) * 100.0).clamp(0.0, 100.0);
                        (
                            dr as f64 / dt,
                            dw as f64 / dt,
                            f64::from(drc) / dt,
                            f64::from(dwc) / dt,
                            busy,
                            latency_ms(&old["read_timer"], &curr_raw["read_timer"]),
                            latency_ms(&old["write_timer"], &curr_raw["write_timer"]),
                        )
                    } else {
                        (0.0, 0.0, 0.0, 0.0, 0.0, None, None)
                    }
                } else {
                    (0.0, 0.0, 0.0, 0.0, 0.0, None, None)
                };
            raw_disks.push(curr_raw);
            out_disks.push(json!({
                "number": n,
                "name": name,
                "read_bps": read_bps,
                "write_bps": write_bps,
                "read_iops": read_iops,
                "write_iops": write_iops,
                "read_latency_ms": read_lat,
                "write_latency_ms": write_lat,
                "busy_percent": busy_percent,
                "queue_length": perf.QueueDepth
            }));
        }
        *previous = json!({ "disks": raw_disks });
        Ok(json!({ "disks": out_disks, "error": null }))
    }
}

pub fn inventory(cancel: &AtomicBool) -> Result<Value, String> {
    #[cfg(windows)]
    {
        win32::inventory(cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = cancel;
        Ok(
            json!({"disks":[],"trim_enabled":null,"system_tuning":{},"error":"Yalnız Windows desteklenmektedir."}),
        )
    }
}

fn canonical(text: &str) -> String {
    text.trim().to_lowercase()
}

fn matches(disk: &Disk, item: &Value) -> bool {
    let serial = canonical(disk.serial.as_deref().unwrap_or(""));
    let reported = canonical(item["serial"].as_str().unwrap_or(""));
    if canonical(&disk.model) != canonical(item["model"].as_str().unwrap_or("")) {
        return false;
    }
    if !serial.is_empty() && serial == reported {
        return true;
    }
    let eui = reported
        .trim_start_matches("eui.")
        .replace('_', "")
        .trim_end_matches('.')
        .to_string();
    eui.len() == 16 && eui.bytes().all(|b| b.is_ascii_hexdigit()) && disk.nvme_eui64.contains(&eui)
}

fn pcie_ceiling_mb_s(gen: u64, width: u64) -> Option<u64> {
    let per_lane: u64 = match gen {
        1 => 250,
        2 => 500,
        3 => 985,
        4 => 1969,
        5 => 3938,
        _ => return None,
    };
    Some(per_lane.saturating_mul(width))
}

fn metric_u64(val: &Value) -> Option<u64> {
    val.as_u64()
        .or_else(|| val.as_str().and_then(|s| s.parse::<u64>().ok()))
}

pub fn attach_volumes(disks: &mut [Disk], inventory: &Value) {
    let os: Vec<&Value> = inventory["disks"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let mappings: Vec<Option<usize>> = disks
        .iter()
        .map(|disk| {
            if !disk.stable_identity {
                return None;
            }
            let candidates: Vec<_> = os
                .iter()
                .enumerate()
                .filter(|(_, item)| matches(disk, item))
                .collect();
            if candidates.len() != 1 {
                return None;
            }
            let (index, candidate) = candidates[0];
            (disks.iter().filter(|d| matches(d, candidate)).count() == 1
                && disks
                    .iter()
                    .filter(|d| {
                        canonical(d.serial.as_deref().unwrap_or(""))
                            == canonical(disk.serial.as_deref().unwrap_or(""))
                    })
                    .count()
                    == 1)
                .then_some(index)
        })
        .collect();
    for (disk, index) in disks.iter_mut().zip(mappings) {
        disk.alerts.retain(|a| {
            !a.code.starts_with("space-")
                && a.code != "pcie-link-bottleneck"
                && a.code != "alignment-4k"
        });
        disk.os_disk_number = None;
        disk.os_serial = None;
        disk.os_model = None;
        disk.volumes.clear();
        disk.optimization = Value::Null;
        if disk.protocol.as_deref() == Some("NVMe") {
            disk.bus_link = Value::Null;
        }
        if let Some(index) = index {
            let item = os[index];
            disk.os_disk_number = item["number"].as_u64();
            disk.os_serial = item["serial"].as_str().map(str::to_owned);
            disk.os_model = item["model"].as_str().map(str::to_owned);
            disk.volumes = item["volumes"].as_array().cloned().unwrap_or_default();
            for volume in &disk.volumes {
                if let (Some(size), Some(free)) =
                    (volume["size_bytes"].as_f64(), volume["free_bytes"].as_f64())
                {
                    if size > 0.0 && free / size < 0.1 {
                        disk.alerts.push(Alert {
                            code: format!("space-{}", volume["letter"].as_str().unwrap_or("?")),
                            level: "warning".into(),
                            title: format!(
                                "{}: boş alanı %10'un altında",
                                volume["letter"].as_str().unwrap_or("?")
                            ),
                            detail: "Büyük dosyaları taşıyın veya Windows depolama ayarlarını inceleyin.".into(),
                        });
                    }
                }
            }
            if disk.protocol.as_deref() == Some("NVMe") && item["pcie"].is_object() {
                let pcie = &item["pcie"];
                let cg = pcie["current_gen"].as_u64().unwrap_or(0);
                let mg = pcie["max_gen"].as_u64().unwrap_or(0);
                let cw = pcie["current_width"].as_u64().unwrap_or(0);
                let mw = pcie["max_width"].as_u64().unwrap_or(0);
                if cg > 0 && cw > 0 && mg > 0 && mw > 0 {
                    let bottleneck = cg < mg || cw < mw;
                    disk.bus_link = json!({
                        "kind": "PCIe",
                        "current": format!("PCIe {cg}.0 x{cw}"),
                        "max": format!("PCIe {mg}.0 x{mw}"),
                        "current_gen": cg,
                        "max_gen": mg,
                        "current_width": cw,
                        "max_width": mw,
                        "ceiling_mb_s": pcie_ceiling_mb_s(cg, cw),
                        "bottleneck": bottleneck
                    });
                    if bottleneck {
                        disk.alerts.push(Alert {
                            code: "pcie-link-bottleneck".into(),
                            level: "warning".into(),
                            title: "PCIe hat hızı darboğazı".into(),
                            detail: format!("NVMe sürücü şu an PCIe {cg}.0 x{cw} hattında çalışıyor; desteklenen tepe hat PCIe {mg}.0 x{mw}. M.2 yuvasını veya BIOS ayarlarını kontrol edin."),
                        });
                    }
                }
            }
            let cap =
                metric_u64(&disk.metrics["capacity_bytes"]).or_else(|| item["disk_bytes"].as_u64());
            let allocated = item["layout"]["allocated_bytes"].as_u64();
            let unallocated = cap.and_then(|c| allocated.map(|a| c.saturating_sub(a)));
            let op_pct = cap
                .zip(unallocated)
                .filter(|(c, _)| *c > 0)
                .map(|(c, u)| (u as f64 / c as f64) * 100.0);
            let misaligned = item["layout"]["misaligned_partitions"].as_u64();
            let aligned_4k = misaligned.map(|m| m == 0);
            if !item["write_cache"].is_null() || !item["layout"].is_null() {
                disk.optimization = json!({
                    "write_cache": item["write_cache"],
                    "aligned_4k": aligned_4k,
                    "misaligned_partitions": misaligned,
                    "allocated_bytes": allocated,
                    "unallocated_bytes": unallocated,
                    "over_provisioning_percent": op_pct
                });
            }
            if misaligned.is_some_and(|m| m > 0) {
                disk.alerts.push(Alert {
                    code: "alignment-4k".into(),
                    level: "warning".into(),
                    title: "4K bölüm hizalaması bozuk".into(),
                    detail: "En az bir bölüm 4096 bayt sınırına hizalanmamış; rastgele yazma hızı düşebilir.".into(),
                });
            }
        }
    }
}

pub fn verify_disk_mapping(
    disks: &[Disk],
    expected: &Disk,
    inventory: &Value,
) -> Result<Disk, String> {
    let mut current = disks.to_vec();
    attach_volumes(&mut current, inventory);
    let matches: Vec<_> = current
        .into_iter()
        .filter(|disk| disk.id == expected.id)
        .collect();
    if matches.len() != 1 {
        return Err("Disk kimliği benzersiz değil.".into());
    }
    let mapped = &matches[0];
    if expected.os_disk_number.is_none()
        || expected.os_serial.is_none()
        || expected.os_model.is_none()
        || mapped.os_disk_number != expected.os_disk_number
        || mapped.os_serial != expected.os_serial
        || mapped.os_model != expected.os_model
    {
        return Err("Güncel Windows disk kimliği değişti veya eşleme belirsiz.".into());
    }
    Ok(mapped.clone())
}
pub fn check_filesystem(volume: &Value, ntfs_only: bool) -> Result<(), String> {
    if ntfs_only && volume["filesystem"] != "NTFS" {
        return Err("chkdsk /scan yalnız güncel NTFS bölümünde sunulur.".into());
    }
    if !matches!(volume["filesystem"].as_str(), Some("NTFS" | "ReFS")) {
        return Err("Yalnız NTFS / ReFS destekleniyor.".into());
    }
    Ok(())
}

pub fn pin_directory(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(windows_sys::Win32::Storage::FileSystem::FILE_READ_ATTRIBUTES)
            .share_mode(3)
            .custom_flags(0x02000000 | 0x00200000);
    }
    options.open(path).map_err(|e| e.to_string())
}

pub fn delete_owned(path: &Path, identity: &same_file::Handle) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
            FILE_READ_ATTRIBUTES,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(DELETE | FILE_READ_ATTRIBUTES)
            .share_mode(3)
            .custom_flags(0x02000000 | 0x00200000)
            .open(path)
            .map_err(|e| e.to_string())?;
        if same_file::Handle::from_file(file.try_clone().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
            != *identity
        {
            return Err("Silinecek dosyanın gerçek kimliği değişti; silme reddedildi.".into());
        }
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        if same_file::Handle::from_path(path).map_err(|e| e.to_string())? != *identity {
            return Err("Dosya kimliği değişti.".into());
        }
        if path.is_dir() {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        }
        .map_err(|e| e.to_string())
    }
}

pub fn latency_ms(before: &Value, after: &Value) -> Option<f64> {
    let number = |v: &Value, key: &str| -> Option<u128> { v[key].as_str()?.parse().ok() };
    let frequency = number(after, "frequency")?;
    if frequency == 0 || frequency != number(before, "frequency")? {
        return None;
    }
    let ticks = number(after, "ticks")?.checked_sub(number(before, "ticks")?)?;
    let operations = number(after, "operations")?.checked_sub(number(before, "operations")?)?;
    if operations == 0 {
        return None;
    }
    let value = ticks as f64 / frequency as f64 / operations as f64 * 1000.0;
    value.is_finite().then_some(value)
}

pub fn performance(previous: &mut Value, cancel: &AtomicBool) -> Result<Value, String> {
    #[cfg(windows)]
    {
        win32::performance(previous, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (previous, cancel);
        Ok(json!({"disks":[],"error":"Yalnız Windows desteklenmektedir."}))
    }
}

pub fn volume_root(letter: &str) -> Result<PathBuf, String> {
    if letter.len() != 1 || !letter.as_bytes()[0].is_ascii_uppercase() {
        return Err("Geçersiz bölüm harfi.".into());
    }
    #[cfg(windows)]
    {
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        if unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) } != 3 {
            return Err("Yalnız yerel sabit sürücü test edilebilir.".into());
        }
        Ok(root.into())
    }
    #[cfg(not(windows))]
    {
        Err("Bu işlem Windows yerel bölüm gerektirir.".into())
    }
}

pub fn reparse(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return true;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

pub fn file_within(file: &std::fs::File, root: &Path) -> Result<(), String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let actual = {
        use std::os::windows::io::AsRawHandle;
        let mut text = vec![0u16; 32768];
        let n = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                text.as_mut_ptr(),
                text.len() as u32,
                0,
            )
        } as usize;
        if n == 0 || n >= text.len() {
            return Err("Dosyanın gerçek hedefi doğrulanamadı.".into());
        }
        PathBuf::from(String::from_utf16(&text[..n]).map_err(|e| e.to_string())?)
    };
    #[cfg(not(windows))]
    let actual = {
        use std::os::unix::io::AsRawFd;
        std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .map_err(|e| e.to_string())?
    };
    if actual.starts_with(root) {
        Ok(())
    } else {
        Err("Açılan dosya doğrulanan bölümün dışında.".into())
    }
}
