use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const CANCELLED: &str = "İşlem iptal edildi.";
pub struct Output {
    pub code: u32,
    pub stdout: String,
    pub stderr: String,
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

pub fn run(
    program: &Path,
    args: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Output, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(CANCELLED.into());
    }
    let mut out = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut errors = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(|e| e.to_string())?)
        .stderr(errors.try_clone().map_err(|e| e.to_string())?);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = OwnedChild(
        command
            .spawn()
            .map_err(|e| format!("{} başlatılamadı: {e}", program.display()))?,
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELLED.into());
        }
        if Instant::now() >= deadline {
            return Err("İşlem zaman aşımına uğradı; alt süreç durduruldu.".into());
        }
        if out.metadata().map_err(|e| e.to_string())?.len()
            + errors.metadata().map_err(|e| e.to_string())?.len()
            > 8 * 1024 * 1024
        {
            return Err("Süreç çıktısı 8 MiB sınırını aştı.".into());
        }
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let read = |file: &mut std::fs::File| -> Result<String, String> {
        let size = file.metadata().map_err(|e| e.to_string())?.len();
        if size > 8 * 1024 * 1024 {
            return Err("Çıktı boyut sınırını aştı.".into());
        }
        file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&bytes)
            .trim_start_matches('\u{feff}')
            .to_owned())
    };
    Ok(Output {
        code: status.code().unwrap_or(-1) as u32,
        stdout: read(&mut out)?,
        stderr: read(&mut errors)?,
    })
}
