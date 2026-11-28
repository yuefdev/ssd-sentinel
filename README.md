# SSD Sentinel

SSD Sentinel is a Windows tool for checking SSD and hard drive health, testing drive speed, and finding deleted files on NTFS volumes.

[Download](https://github.com/yuefdev/ssd-sentinel/releases/latest) · [Website](https://yuefdev.github.io/ssd-sentinel/) · [Changelog](CHANGELOG.md) · [Türkçe](#türkçe-özet)

## What you can do

- Check SMART readings, track temperature, and review SSD wear over time.
- Run speed tests, scan the disk surface, and inspect slow or unreadable areas.
- Look for deleted files and preview supported images and text. Scans are read-only. Save recovered files to another drive.
- Move selected files between NTFS drives with copy verification and rollback.

## File recovery on `main`

Find files by name, folder, or extension. Results are split into pages, and selections carry across pages.

Recognized screenshots and editable project files appear near the top. Caches and build output sit lower. Use the file type filter to jump to screenshots or projects.

![File Recovery showing screenshots and project files in an example scan](docs/screenshots/recovery-center.png)

## Install

Download the installer or portable ZIP for Windows 10 or 11 (x64). Use `SHA256SUMS.txt` to check the download. [Releases](https://github.com/yuefdev/ssd-sentinel/releases) · [Signing policy](CODE_SIGNING.md)

## Build from source

The build uses Rust, Node.js, and smartmontools. Setup steps are in the [contribution guide](CONTRIBUTING.md).

```powershell
npm ci
npm run desktop:build
```

You can open the sample disks in replay mode with the built app.

```powershell
.\src-tauri\target\release\ssd-sentinel.exe --snapshot-dir .\src-tauri\tests\fixtures
```

[Architecture](docs/ARCHITECTURE.md) · [Disk safety](docs/SAFETY_SPEC.md) · [Privacy](https://yuefdev.github.io/ssd-sentinel/privacy/) · [License](LICENSE)

## Türkçe özet

SSD Sentinel ile Windows’ta disk sağlığını, sıcaklığı ve SSD aşınmasını takip edebilir, hız testi ve yüzey taraması yapabilirsin.

`main` dalındaki dosya kurtarma ekranı sonuçları sayfalıyor. Ad, klasör veya uzantıyla arayabilirsin. Tanınan ekran görüntüleri ve düzenlenebilir proje dosyaları üstte, önbellek ve derleme çıktıları daha aşağıda görünüyor.
