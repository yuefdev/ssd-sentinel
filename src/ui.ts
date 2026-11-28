import type { Lang } from "./i18n";

export function format(n: unknown, digits = 0, lang: Lang = "tr"): string {
  if (n === null || n === undefined || n === "" || typeof n === "boolean")
    return "—";
  const value = Number(n);
  return Number.isFinite(value)
    ? value.toLocaleString(lang === "en" ? "en-US" : "tr-TR", {
        maximumFractionDigits: digits,
      })
    : "—";
}
export function bytes(n: unknown, lang: Lang = "tr"): string {
  if (n === null || n === undefined) return "—";
  const value = Number(n);
  if (!Number.isFinite(value) || value < 0) return "—";
  const i = Math.min(4, Math.floor(Math.log10(Math.max(1, value)) / 3));
  return `${format(value / 1000 ** i, 1, lang)} ${["B", "KB", "MB", "GB", "TB"][i]}`;
}
export function compareRaw(a: string, b: string, lang: Lang = "tr"): number {
  if (/^\d+$/.test(a) && /^\d+$/.test(b)) {
    const first = BigInt(a),
      second = BigInt(b);
    return first === second ? 0 : first > second ? 1 : -1;
  }
  return a.localeCompare(b, lang);
}
export function escape(v: unknown): string {
  return String(v ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
}
export function canStart(state: unknown, disk: unknown): boolean {
  const s = state as {
    source?: string;
    stale?: boolean;
    busy?: boolean;
  } | null;
  const d = disk as {
    stable_identity?: boolean;
    read_incomplete?: boolean;
    os_disk_number?: number | null;
    os_serial?: string | null;
    os_model?: string | null;
    volumes?: unknown[];
  } | null;
  return !!(
    s &&
    d &&
    s.source === "live" &&
    !s.stale &&
    !s.busy &&
    d.stable_identity &&
    !d.read_incomplete &&
    d.os_disk_number != null &&
    d.os_serial &&
    d.os_model &&
    d.volumes?.length
  );
}

export function mappingDisabledReason(
  state: unknown,
  disk: unknown,
  lang: Lang = "tr",
): string {
  const s = state as { admin?: boolean } | null;
  const d = disk as {
    stable_identity?: boolean;
    os_disk_number?: number | null;
    volumes?: unknown[];
  } | null;
  if (!d?.stable_identity) {
    return lang === "en"
      ? "This drive did not report a unique model/serial identity (USB bridge or controller limitation)."
      : "Bu disk benzersiz model/seri kimliği bildirmedi (USB köprüsü veya denetleyici kısıtı).";
  }
  if (d.os_disk_number != null && (!d.volumes || d.volumes.length === 0)) {
    return lang === "en"
      ? `Drive matched (Disk ${d.os_disk_number}), but no mounted Windows volume with a drive letter was found.`
      : `Disk eşleşti (Disk ${d.os_disk_number}), ancak harf atanmış uyumlu Windows bölümü bulunamadı.`;
  }
  if (!s?.admin) {
    return lang === "en"
      ? "Could not verify Windows volume mapping. Try clicking “Run as Admin” in the top bar."
      : "Windows bölüm eşlemesi kurulamadı. Sağ üstten “Yönetici olarak aç” ile tekrar deneyin.";
  }
  return lang === "en"
    ? "A verified Windows volume mapping is required (possible RAID/VMD controller or duplicate disk identity)."
    : "Güvenilir Windows bölüm eşlemesi gerekli (RAID/VMD denetleyici veya çakışan disk kimliği olabilir).";
}

export function isWriteLocked(state: unknown): boolean {
  const s = state as { read_only_guard?: boolean } | null;
  return s?.read_only_guard === true;
}

export function canWrite(state: unknown): boolean {
  const s = state as {
    source?: string;
    stale?: boolean;
    busy?: boolean;
    read_only_guard?: boolean;
  } | null;
  return !!(s && s.source === "live" && !s.stale && !s.busy && !s.read_only_guard);
}

export function canMaintain(state: unknown, disk: unknown): boolean {
  const s = state as {
    source?: string;
    stale?: boolean;
    busy?: boolean;
    admin?: boolean;
    read_only_guard?: boolean;
  } | null;
  const d = disk as {
    stable_identity?: boolean;
    read_incomplete?: boolean;
    os_disk_number?: number | null;
    os_serial?: string | null;
    os_model?: string | null;
  } | null;
  return !!(
    s &&
    d &&
    s.source === "live" &&
    !s.stale &&
    !s.busy &&
    !s.read_only_guard &&
    s.admin &&
    d.stable_identity &&
    !d.read_incomplete &&
    d.os_disk_number != null &&
    d.os_serial &&
    d.os_model
  );
}

export function canOptimizeSystem(state: unknown): boolean {
  const s = state as {
    source?: string;
    stale?: boolean;
    busy?: boolean;
    admin?: boolean;
    read_only_guard?: boolean;
  } | null;
  return !!(
    s &&
    s.source === "live" &&
    !s.stale &&
    !s.busy &&
    !s.read_only_guard &&
    s.admin
  );
}

export function maskSerial(serial: string | null | undefined): string {
  const s = (serial ?? "").trim();
  if (!s) return "—";
  if (s.length <= 4) return "•".repeat(s.length);
  const dots = "•".repeat(Math.min(8, Math.max(4, s.length - 4)));
  return `${s.slice(0, 2)}${dots}${s.slice(-2)}`;
}

function recoverySearchKey(value: string): string {
  return value.toLowerCase().normalize("NFD").replace(/\p{M}/gu, "").replace(/\u0131/g, "i").replaceAll("\\", "/");
}

export function aiNaturalFilter<
  T extends {
    name: string;
    category: string;
    file_kind?: string;
    confidence: number;
    extension: string;
    original_path: string;
    sub_type?: string;
    product_name?: string | null;
    publisher?: string | null;
    ai_summary?: string;
  },
>(items: T[], query: string, categoryFilter: string | ((item: T) => boolean) = "all"): T[] {
  const tokens = recoverySearchKey(query.trim()).split(/\s+/).filter(Boolean);
  const systemTerms = ["sistem", "surucu", "kutuphane", "system", "driver", "library"];
  const categoryTerms: [string[], string[]][] = [
    [["uygulama", "program", "kurulum", "setup", "app", "apps", "programs", "installer"], ["exe_setup", "game_portable"]],
    [["oyun", "game", "games", "steam"], ["game_portable"]],
    [["belge", "dokuman", "word", "excel", "docs", "document", "documents"], ["document"]],
    [["foto", "fotograf", "resim", "gorsel", "photo", "photos", "image", "images", "picture", "pictures"], ["photo"]],
    [["video", "videos", "ses", "muzik", "medya", "audio", "sound", "music", "media"], ["video_audio"]],
    [["arsiv", "archive", "archives"], ["archive"]],
    [["veritabani", "kod", "ayar", "database", "code", "config", "vibe", "proje", "project"], ["database_code"]],
    [systemTerms, ["system_lib"]],
  ];
  const extensions = new Set(["exe", "msi", "pdf", "doc", "png", "jpg", "mp4", "mp3", "zip", "rar", "7z", "sqlite", "json", "dll", "sys"]);
  const healthTerms = ["saglam", "yuksek", "kesin", "bozulmamis", "healthy", "intact", "good", "solid", "high"];
  const matchesSystem = tokens.some((token) => systemTerms.includes(token) || /(?:^|\.)(dll|sys)$/.test(token));
  const matchesCategory = typeof categoryFilter === "function" ? categoryFilter : (item: T) =>
    categoryFilter === "all" ? (matchesSystem || item.category !== "system_lib") : item.category === categoryFilter;

  return items.filter((item) => {
    if (!matchesCategory(item)) return false;
    if (!tokens.length) return true;
    const extension = recoverySearchKey(item.extension ?? "").replace(/^\./, "");
    const screenshotTerms = item.file_kind === "screenshot" ? "screenshot screenshots ekran goruntusu ekran goruntuleri ekran alintisi" : "";
    const text = recoverySearchKey([
      item.name, item.sub_type, item.product_name, item.publisher, item.extension, item.original_path, item.ai_summary, screenshotTerms,
    ].join(" "));

    return tokens.every((token) => {
      const ext = token.replace(/^\./, "");
      if (extensions.has(ext)) return extension === ext;
      if (healthTerms.includes(token)) return item.confidence >= 75;
      const categories = categoryTerms.find(([terms]) => terms.includes(token))?.[1];
      return !!categories?.includes(item.category) || text.includes(token);
    });
  });
}

export function formatInterface(
  iface: string | null | undefined,
  diskType = "",
): string {
  const s = (iface ?? "").trim();
  if (!s) return "—";
  if (/^\d+\.\d+(?:\.\d+)?$/.test(s) && (!diskType || /nvme/i.test(diskType))) {
    return `NVMe ${s}`;
  }
  return s;
}

export function alignmentDescription(
  aligned: boolean | null | undefined,
  lang: Lang = "tr",
): string {
  if (lang === "en") {
    if (aligned === true)
      return "All partitions start on a physical 4K sector boundary.";
    if (aligned === false)
      return "Misaligned partitions slow down random writes and increase flash wear.";
    return "4K alignment cannot be verified without Windows partition mapping.";
  }
  if (aligned === true)
    return "Tüm bölümler fiziksel 4K sektör sınırında başlıyor.";
  if (aligned === false)
    return "Hizasız bölüm rastgele yazmayı yavaşlatır ve aşınmayı artırır.";
  return "Windows bölüm eşlemesi olmadan 4K hizalama doğrulanamaz.";
}

export function hasNormalizedSmart(
  rows: {
    value: number | null;
    worst: number | null;
    threshold: number | null;
  }[],
): boolean {
  return rows.some(
    (r) => r.value !== null || r.worst !== null || r.threshold !== null,
  );
}

export function isUnopenableMedia(item: {
  openable?: boolean;
  media_status?: string | null;
}): boolean {
  return item.openable === false || item.media_status === "unrecoverable";
}

export function mediaStatusBadge(
  item: {
    media_status?: string | null;
    openable?: boolean;
  },
  lang: Lang = "tr",
): { label: string; level: "ok" | "warning" | "critical" | "info" } | null {
  if (item.media_status === "repaired") {
    return {
      label: lang === "en" ? "Auto-Repaired" : "Otomatik Onarılacak",
      level: "ok",
    };
  }
  if (item.media_status === "thumbnail_rescued") {
    return {
      label: lang === "en" ? "Preview Rescued" : "Önizleme Kurtarıldı",
      level: "warning",
    };
  }
  if (item.media_status === "unrecoverable" || item.openable === false) {
    return { label: lang === "en" ? "Unopenable" : "Açılamaz", level: "critical" };
  }
  if (item.media_status === "intact") {
    return {
      label: lang === "en" ? "Openable · Intact" : "Açılabilir · Sağlam",
      level: "ok",
    };
  }
  return null;
}

export function isRecoverableRow(item: {
  source: string;
  trim_zeroed: boolean;
  openable?: boolean;
  media_status?: string | null;
}): boolean {
  return (
    !item.trim_zeroed &&
    !isUnopenableMedia(item) &&
    item.source !== "usn_journal" &&
    item.source !== "indx_slack"
  );
}

export type AppRecoveryStatus =
  | "full_restore"
  | "installer_found"
  | "reinstall_only";

export type AppFilter =
  | "all"
  | "full_restore"
  | "installer_found"
  | "games"
  | "reinstall_only";

export function inferAppRecoveryStatus(app: {
  recovery_status?: string;
  vss_recoverable?: boolean;
  installer_found?: boolean;
  recoverable_item_id?: string | null;
}): AppRecoveryStatus {
  if (
    app.recovery_status === "full_restore" ||
    app.recovery_status === "installer_found" ||
    app.recovery_status === "reinstall_only"
  ) {
    return app.recovery_status;
  }
  if (app.vss_recoverable) return "full_restore";
  if (app.installer_found || app.recoverable_item_id) return "installer_found";
  return "reinstall_only";
}

export function filterDeletedApps<
  T extends {
    name: string;
    publisher?: string | null;
    exe_path: string;
    category?: string;
    recovery_status?: string;
    vss_recoverable?: boolean;
    installer_found?: boolean;
    recoverable_item_id?: string | null;
    winget_hint?: string;
    sources?: string[];
    drive_letter?: string;
  },
>(apps: T[], query: string, appFilter: AppFilter = "all"): T[] {
  let list = apps.filter((app) => {
    const status = inferAppRecoveryStatus(app);
    if (appFilter === "full_restore") return status === "full_restore";
    if (appFilter === "installer_found") return status === "installer_found";
    if (appFilter === "reinstall_only") return status === "reinstall_only";
    if (appFilter === "games") {
      const cat = (app.category ?? "").toLocaleLowerCase("tr");
      return cat === "oyun" || cat === "game";
    }
    return true;
  });
  const q = recoverySearchKey(query.trim());
  if (!q) return list;
  const tokens = q.split(/\s+/).filter(Boolean);
  for (const tok of tokens) {
    list = list.filter((app) =>
      recoverySearchKey(`${app.name} ${app.publisher ?? ""} ${app.exe_path} ${app.category ?? ""} ${app.winget_hint ?? ""} ${(app.sources ?? []).join(" ")} ${app.drive_letter ?? ""}`).includes(tok),
    );
  }
  return list;
}

export interface BenchmarkEvaluation {
  label: string;
  level: "ok" | "warning" | "critical" | "";
  detail: string;
  band: "NVMe" | "SATA SSD" | "HDD";
}

export function evaluateBenchmarkSpeed(
  seqReadMbS: number | null | undefined,
  diskType: string,
  iface?: string | null,
  busLink?: { ceiling_mb_s?: number | null; bottleneck?: boolean } | null,
  lang: Lang = "tr",
): BenchmarkEvaluation | null {
  if (
    seqReadMbS === null ||
    seqReadMbS === undefined ||
    !Number.isFinite(seqReadMbS) ||
    seqReadMbS <= 0
  ) {
    return null;
  }
  if (/hdd/i.test(diskType)) {
    if (seqReadMbS >= 150) {
      return {
        band: "HDD",
        label: lang === "en" ? "Very fast for HDD" : "HDD için çok iyi",
        level: "ok",
        detail:
          lang === "en"
            ? "Top tier sequential read speed for mechanical drives (150+ MB/s)."
            : "Mekanik diskler için üst sıralı okuma bandında (150+ MB/sn).",
      };
    }
    if (seqReadMbS >= 80) {
      return {
        band: "HDD",
        label: lang === "en" ? "Normal for HDD" : "HDD için normal",
        level: "ok",
        detail:
          lang === "en"
            ? "Within standard 5400/7200 RPM HDD performance range (80–150 MB/s)."
            : "Standart 5400/7200 RPM HDD hız aralığında (80–150 MB/sn).",
      };
    }
    return {
      band: "HDD",
      label: lang === "en" ? "Low for HDD" : "HDD için düşük",
      level: "warning",
      detail:
        lang === "en"
          ? "Below 80 MB/s is slow for a mechanical drive; check fragmentation or weak sectors."
          : "80 MB/sn altı mekanik disk için yavaş; parçalanma veya zayıf sektör kontrolü önerilir.",
    };
  }
  if (/nvme/i.test(`${diskType} ${iface ?? ""}`)) {
    const bottleneckNote = busLink?.bottleneck
      ? lang === "en"
        ? " A PCIe link bottleneck appears active."
        : " PCIe bağlantı darboğazı aktif görünüyor."
      : "";
    if (seqReadMbS >= 3000) {
      return {
        band: "NVMe",
        label: lang === "en" ? "Very fast for NVMe" : "NVMe için çok hızlı",
        level: "ok",
        detail:
          lang === "en"
            ? "High PCIe 3.0/4.0 NVMe sequential performance band (3,000+ MB/s)."
            : "PCIe 3.0/4.0 NVMe üst performans bandında (3.000+ MB/sn).",
      };
    }
    if (seqReadMbS >= 1500) {
      return {
        band: "NVMe",
        label: lang === "en" ? "Normal for NVMe" : "NVMe için normal",
        level: "ok",
        detail:
          lang === "en"
            ? `Standard NVMe sequential read band (1,500–3,000 MB/s).${bottleneckNote}`
            : `Standart NVMe sıralı okuma bandında (1.500–3.000 MB/sn).${bottleneckNote}`,
      };
    }
    if (seqReadMbS >= 800) {
      return {
        band: "NVMe",
        label:
          lang === "en"
            ? "Lane-limited / moderate for NVMe"
            : "NVMe için hat sınırlı / orta",
        level: "warning",
        detail:
          lang === "en"
            ? `Below typical NVMe speeds; PCIe x2 width, power saving, or thermal throttling may limit bandwidth.${bottleneckNote}`
            : `NVMe için ortalamanın altında; PCIe x2 hattı, güç tasarrufu veya termal fren etkiliyor olabilir.${bottleneckNote}`,
      };
    }
    return {
      band: "NVMe",
      label:
        lang === "en"
          ? "Slower than expected for NVMe"
          : "NVMe için beklenenden yavaş",
      level: "critical",
      detail:
        lang === "en"
          ? `Below 800 MB/s is unusual for NVMe SSD; inspect PCIe link width, temperature, and TRIM.${bottleneckNote}`
          : `800 MB/sn altı NVMe SSD için olağandışı; PCIe hattı, sıcaklık ve TRIM durumunu kontrol edin.${bottleneckNote}`,
    };
  }
  if (seqReadMbS >= 450) {
    return {
      band: "SATA SSD",
      label: lang === "en" ? "Very fast for SATA SSD" : "SATA SSD için çok iyi",
      level: "ok",
      detail:
        lang === "en"
          ? "Near SATA III (6 Gb/s) interface saturation (450+ MB/s)."
          : "SATA III (6 Gb/sn) tavan hızına yakın (450+ MB/sn).",
    };
  }
  if (seqReadMbS >= 250) {
    return {
      band: "SATA SSD",
      label: lang === "en" ? "Normal for SATA SSD" : "SATA SSD için normal",
      level: "ok",
      detail:
        lang === "en"
          ? "Normal sequential read range for SATA SSDs (250–450 MB/s)."
          : "SATA SSD için olağan sıralı okuma bandında (250–450 MB/sn).",
    };
  }
  return {
    band: "SATA SSD",
    label: lang === "en" ? "Low for SATA SSD" : "SATA SSD için düşük",
    level: "warning",
    detail:
      lang === "en"
        ? "Below 250 MB/s may indicate a SATA II port, full SLC cache, or disabled TRIM."
        : "250 MB/sn altı SATA II hattı, dolu SLC önbellek veya TRIM eksikliğine işaret edebilir.",
  };
}

export type StepState = "done" | "active" | "pending";

export function computeSurfaceStepper(opts: {
  hasFullScan: boolean;
  hasDiagnosis: boolean;
  hasPinpoint: boolean;
  runningMode?: string | null;
  hasTargets?: boolean;
}): { activeStep: 1 | 2 | 3; states: [StepState, StepState, StepState] } {
  const { hasFullScan, hasDiagnosis, hasPinpoint, runningMode, hasTargets } =
    opts;
  if (runningMode === "physical_scan") {
    return { activeStep: 1, states: ["active", "pending", "pending"] };
  }
  if (
    runningMode === "magnetic_refresh" ||
    runningMode === "bad_zone_isolate"
  ) {
    return { activeStep: 3, states: ["done", "done", "active"] };
  }
  if (hasPinpoint) {
    return { activeStep: 3, states: ["done", "done", "done"] };
  }
  if (hasFullScan || hasDiagnosis) {
    if (hasTargets === false) {
      return { activeStep: 2, states: ["done", "done", "pending"] };
    }
    return { activeStep: 2, states: ["done", "active", "pending"] };
  }
  return { activeStep: 1, states: ["active", "pending", "pending"] };
}

export function checkpointProgress(
  cp:
    | {
        next_zone?: number;
        completed_zones?: number;
        total_zones?: number;
        done_bytes?: number;
        disk_bytes?: number;
        progress_percent?: number;
      }
    | null
    | undefined,
): { completed: number; total: number; percent: number } {
  if (!cp) return { completed: 0, total: 200, percent: 0 };
  const total = Math.max(1, Number(cp.total_zones ?? 200));
  const completed = Math.max(
    0,
    Math.min(total, Number(cp.completed_zones ?? cp.next_zone ?? 0)),
  );
  const percent =
    typeof cp.progress_percent === "number" &&
    Number.isFinite(cp.progress_percent)
      ? Math.max(0, Math.min(100, Math.round(cp.progress_percent)))
      : cp.disk_bytes && cp.done_bytes != null && cp.disk_bytes > 0
        ? Math.max(
            0,
            Math.min(100, Math.round((cp.done_bytes / cp.disk_bytes) * 100)),
          )
        : Math.round((completed / total) * 100);
  return { completed, total, percent };
}

export interface TierBadge {
  label: string;
  level: "ok" | "warning" | "critical" | "info" | "";
}

export function slcPressureBadge(
  pressure: string | null | undefined,
  isSsd = true,
  lang: Lang = "tr",
): TierBadge {
  const p = (pressure ?? "low").toLowerCase();
  if (!isSsd) {
    if (p === "high")
      return {
        label: lang === "en" ? "High Usage" : "Yüksek Doluluk",
        level: "critical",
      };
    if (p === "moderate")
      return {
        label: lang === "en" ? "Moderate Usage" : "Orta Doluluk",
        level: "warning",
      };
    return {
      label: lang === "en" ? "Low Usage" : "Düşük Doluluk",
      level: "ok",
    };
  }
  if (p === "high") {
    return {
      label:
        lang === "en"
          ? "High — SLC Cache Shrinking"
          : "Yüksek — SLC Önbellek Daralıyor",
      level: "critical",
    };
  }
  if (p === "moderate") {
    return { label: lang === "en" ? "Moderate" : "Orta", level: "warning" };
  }
  return { label: lang === "en" ? "Low" : "Düşük", level: "ok" };
}

export function tierCategoryBadge(
  category: string | null | undefined,
  isDir = false,
  lang: Lang = "tr",
): TierBadge {
  const en = lang === "en";
  switch ((category ?? "").toLowerCase()) {
    case "video":
      return { label: "Video", level: "info" };
    case "archive":
      return { label: en ? "Archive" : "Arşiv", level: "warning" };
    case "iso":
      return { label: "ISO", level: "warning" };
    case "installer":
      return { label: en ? "Installer" : "Kurulum", level: "info" };
    case "game":
      return { label: en ? "Game" : "Oyun", level: "ok" };
    case "photo":
      return { label: en ? "Photo" : "Fotoğraf", level: "info" };
    case "audio":
      return { label: en ? "Audio" : "Ses", level: "info" };
    case "code":
      return { label: en ? "Code" : "Kod", level: "" };
    case "database":
      return { label: en ? "Database" : "Veritabanı", level: "" };
    case "folder":
      return { label: en ? "Folder" : "Klasör", level: "" };
    default:
      return isDir
        ? { label: en ? "Folder" : "Klasör", level: "" }
        : { label: en ? "File" : "Dosya", level: "" };
  }
}

export function tierScoreBadge(
  score: number,
  direction: string | null | undefined,
  lang: Lang = "tr",
): TierBadge {
  const s = Math.max(0, Math.min(100, Math.round(Number(score) || 0)));
  if (direction === "to_ssd") {
    return {
      label: lang === "en" ? `${s}% Hot` : `%${s} Sıcak`,
      level: s >= 75 ? "ok" : s >= 50 ? "warning" : "",
    };
  }
  return {
    label: lang === "en" ? `${s}% Cold` : `%${s} Soğuk`,
    level: s >= 75 ? "info" : s >= 50 ? "warning" : "",
  };
}

export function formatUntouchedDays(
  days: number | null | undefined,
  lang: Lang = "tr",
): string {
  if (days === null || days === undefined || !Number.isFinite(Number(days))) {
    return "—";
  }
  const d = Math.max(0, Math.round(Number(days)));
  if (lang === "en") {
    if (d === 0) return "Used today";
    return d === 1 ? "1 day untouched" : `${d} days untouched`;
  }
  if (d === 0) return "Bugün kullanıldı";
  return `${d} gün dokunulmadı`;
}

export function findCompanionDrive<
  T extends {
    id: string;
    model: string;
    type: string;
    volumes: {
      letter: string;
      filesystem: string;
      free_bytes: number | null;
    }[];
  },
>(
  disks: T[],
  selectedDisk: T | null | undefined,
): {
  disk_id: string;
  model: string;
  letter: string;
  type: string;
  free_bytes: number;
  role: "hdd_archive" | "ssd_boost";
} | null {
  if (!selectedDisk) return null;
  const sourceIsHdd = /hdd/i.test(selectedDisk.type);
  const minFreeBytes = sourceIsHdd ? 20 * 1024 ** 3 : 0;
  let best: {
    disk_id: string;
    model: string;
    letter: string;
    type: string;
    free_bytes: number;
    role: "hdd_archive" | "ssd_boost";
  } | null = null;
  for (const d of disks) {
    if (d.id === selectedDisk.id) continue;
    const targetIsHdd = /hdd/i.test(d.type);
    if (sourceIsHdd === targetIsHdd) continue;
    for (const v of d.volumes ?? []) {
      if (!/^ntfs$/i.test((v.filesystem ?? "").trim())) continue;
      const free = Number(v.free_bytes ?? 0);
      if (!Number.isFinite(free) || free < minFreeBytes) continue;
      if (!best || free > best.free_bytes) {
        best = {
          disk_id: d.id,
          model: d.model,
          letter: v.letter,
          type: d.type,
          free_bytes: free,
          role: sourceIsHdd ? "ssd_boost" : "hdd_archive",
        };
      }
    }
  }
  return best;
}

export function devStackBadge(stack: string | null | undefined): TierBadge {
  const s = (stack ?? "").trim();
  switch (s) {
    case "Node / JS":
      return { label: "Node / JS", level: "info" };
    case "Rust":
      return { label: "Rust", level: "warning" };
    case "Python":
      return { label: "Python", level: "ok" };
    case "Go":
      return { label: "Go", level: "info" };
    case ".NET":
      return { label: ".NET", level: "info" };
    case "Java / Gradle":
      return { label: "Java / Gradle", level: "warning" };
    case "Flutter / Dart":
      return { label: "Flutter / Dart", level: "info" };
    default:
      return { label: s || "Git Repo", level: "" };
  }
}

export function devCacheKindBadge(
  kind: string | null | undefined,
  lang: Lang = "tr",
): TierBadge {
  const en = lang === "en";
  switch ((kind ?? "").toLowerCase()) {
    case "ai_model":
      return { label: en ? "Local LLM" : "Yerel LLM", level: "warning" };
    case "pkg_cache":
      return { label: en ? "Pkg Cache" : "Paket Önbelleği", level: "info" };
    case "toolchain":
      return { label: en ? "Toolchain" : "Araç Zinciri", level: "" };
    default:
      return { label: en ? "Cache" : "Önbellek", level: "" };
  }
}

export function ntfs8dot3Label(
  val: number | null | undefined,
  lang: Lang = "tr",
): string {
  const en = lang === "en";
  if (val === 1) return en ? "Disabled (Fast)" : "Kapalı (Hızlı)";
  if (val === 0) return en ? "Enabled on all" : "Tümünde Açık";
  if (val === 2) return en ? "Per-volume (Default)" : "Birim Bazlı (Varsayılan)";
  if (val === 3) return en ? "Disabled on system" : "Sistemde Kapalı";
  return en ? "Unknown" : "Bilinmiyor";
}

export function devEnvSnippet(targetLetter: string): string {
  const clean = (targetLetter || "D").trim().replace(/:+$/, "").toUpperCase() || "D";
  const root = `${clean}:\\DevCaches`;
  return [
    `[Environment]::SetEnvironmentVariable("CARGO_HOME", "${root}\\cargo", "User")`,
    `[Environment]::SetEnvironmentVariable("NPM_CONFIG_CACHE", "${root}\\npm-cache", "User")`,
    `[Environment]::SetEnvironmentVariable("PIP_CACHE_DIR", "${root}\\pip-cache", "User")`,
    `[Environment]::SetEnvironmentVariable("UV_CACHE_DIR", "${root}\\uv-cache", "User")`,
    `[Environment]::SetEnvironmentVariable("OLLAMA_MODELS", "${root}\\ollama-models", "User")`,
  ].join("\n");
}

export function computePollSignature(
  state: unknown,
  opts: {
    page: string;
    selected: string;
    letter: string;
    connectionError: string;
    lang?: Lang;
    detailCounts?: string;
    forecast?: unknown;
    checkpoint?: unknown;
    tierAnalysis?: unknown;
    tierMoves?: unknown;
    tierBusy?: boolean;
    devAnalysis?: unknown;
    devBusy?: boolean;
    devFilter?: string;
    devCustomRoot?: string;
  },
): string {
  const s = state as Record<string, unknown> | null;
  if (!s) return `empty:${opts.page}:${opts.connectionError}:${opts.lang ?? "tr"}`;
  const core: Record<string, unknown> = {
    source: s.source,
    admin: s.admin,
    updating: s.updating,
    stale: s.stale,
    busy: s.busy,
    updated_at: s.updated_at,
    error: s.error,
    partial_warnings: s.partial_warnings,
    minimise_to_tray: s.minimise_to_tray,
    read_only_guard: s.read_only_guard,
    auto_check_updates: s.auto_check_updates,
    health_mode: s.health_mode,
    language: s.language,
    portable: s.portable,
    data_dir: s.data_dir,
    version: s.version,
    windows: s.windows,
    optimizations: s.optimizations,
    tier_moves: s.tier_moves,
    disks: s.disks,
    job: s.job,
    page: opts.page,
    selected: opts.selected,
    letter: opts.letter,
    lang: opts.lang ?? "tr",
    connectionError: opts.connectionError,
    detailCounts: opts.detailCounts ?? "",
    forecast: opts.forecast ?? null,
    checkpoint: opts.checkpoint ?? null,
    tierAnalysis: opts.tierAnalysis ?? null,
    tierMoves: opts.tierMoves ?? null,
    tierBusy: !!opts.tierBusy,
    devAnalysis: opts.devAnalysis ?? null,
    devBusy: !!opts.devBusy,
    devFilter: opts.devFilter ?? "all",
    devCustomRoot: opts.devCustomRoot ?? "",
  };
  if (opts.page === "performance") {
    core.performance = s.performance;
  }
  return JSON.stringify(core);
}
