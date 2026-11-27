import { invoke } from "@tauri-apps/api/core";
import brandLogo from "../assets/sentinel.svg";
import { icon } from "./icons";
import {
  L,
  detectInitialLang,
  getLang,
  localeTag,
  setLang,
  translateAlert,
  translateDiagnosis,
  translateDiskType,
  translateForecast,
  translateHealthExplanation,
  translatePinpointLabel,
  translateSmartField,
  translateTierAnalysis,
  translateUpdateMessage,
  type Lang,
} from "./i18n";
import {
  aiNaturalFilter,
  alignmentDescription,
  bytes,
  canStart,
  canMaintain,
  canOptimizeSystem,
  canWrite,
  checkpointProgress,
  compareRaw,
  computePollSignature,
  computeSurfaceStepper,
  devCacheKindBadge,
  devEnvSnippet,
  devStackBadge,
  escape as e,
  evaluateBenchmarkSpeed,
  filterDeletedApps,
  findCompanionDrive,
  format as f,
  formatInterface,
  formatUntouchedDays,
  hasNormalizedSmart,
  inferAppRecoveryStatus,
  isRecoverableRow,
  isUnopenableMedia,
  isWriteLocked,
  mappingDisabledReason,
  mediaStatusBadge,
  ntfs8dot3Label,
  slcPressureBadge,
  tierCategoryBadge,
  tierScoreBadge,
  type AppFilter,
} from "./ui";
import type {
  AppState,
  BatchRestoreResult,
  Block,
  DeletedAppBundle,
  DevWorkspaceAnalysis,
  Disk,
  Event,
  HealthForecast,
  History,
  Job,
  OptimizationRecord,
  PinpointRepairRow,
  RecoveredItem,
  RecoveryPreview,
  RepairSummary,
  SmartTierSummary,
  SurfaceCheckpoint,
  SurfaceDiagnosis,
  SurfaceTarget,
  TierAnalysis,
  TierCandidate,
  TierMoveRecord,
} from "./types";
import "./style.css";

type Page =
  | "overview"
  | "smart"
  | "performance"
  | "optimize"
  | "surface"
  | "recovery"
  | "history"
  | "tools";
const getPages = (): [Page, string, string][] => [
  ["overview", L("Genel Bakış", "Overview"), "overview"],
  ["optimize", L("Optimizasyon", "Optimize"), "bolt"],
  ["surface", L("Yüzey Onarımı", "Surface Repair"), "surface"],
  ["recovery", L("Dosya Kurtarma", "File Recovery"), "recover"],
  ["performance", L("Hız Testi", "Benchmark"), "performance"],
  ["smart", L("SMART Verileri", "SMART Data"), "smart"],
  ["history", L("Geçmiş", "History"), "history"],
  ["tools", L("Bakım & Ayarlar", "Tools & Settings"), "tools"],
];
const mainPages = new Set<Page>(["overview", "optimize", "surface", "recovery"]);
const hadStoredLang = (() => {
  try {
    return !!localStorage.getItem("sentinel-lang");
  } catch {
    return false;
  }
})();
setLang(detectInitialLang());
let lastSyncedBackendLang = "";
let state: AppState | null = null;
let selected = "",
  page: Page = "overview",
  search = "",
  filter = "all",
  advanced = false,
  sort = "default",
  field = "",
  blockIndex = 0;
let history: History[] = [],
  events: Event[] = [],
  benchmarkHistory: Job[] = [],
  surfaceHistory: Job[] = [],
  defragHistory: Job[] = [],
  smartTierHistory: Job[] = [],
  autopilotHistory: Job[] = [],
  recoveryHistory: Job[] = [],
  healthForecast: HealthForecast | null = null,
  surfaceCheckpoint: SurfaceCheckpoint | null = null,
  tierAnalysis: TierAnalysis | null = null,
  tierMoves: TierMoveRecord[] = [],
  tierAnalyzing = false,
  tierTransferring = false,
  devAnalysis: DevWorkspaceAnalysis | null = null,
  devScanning = false,
  devCleaning = false,
  devCustomRoot = "",
  devFilter: "all" | "stale" = "all",
  viewedJob: Job | null = null;
let recoverySearch = "",
  recoveryCat = "all",
  recoverySort: "best" | "size" | "recent" | "name" = "best",
  recoveryTab: "apps" | "files" = "apps",
  recoveryTabManual = false,
  lastRecoveryJobId = "",
  appSearch = "",
  appFilter: AppFilter = "all",
  showUnopenableMedia = false;
const selectedRecoveryIds = new Set<string>();
// Budgets are always "auto": the Rust core picks the most thorough safe value.
const recConfig: { mode: string; size_mib: number | "auto" } = {
  mode: "quick",
  size_mib: "auto",
};
let opBounds: {
  disk_id: string;
  letter: string;
  size_min: number;
  size_max: number;
  current_size: number;
  free_bytes: number;
} | null = null;
let updateStatus: {
  repo?: string | null;
  status: string;
  current: string;
  latest?: string | null;
  url?: string;
  download_url?: string | null;
  asset_name?: string | null;
  asset_size?: number | null;
  sha256_url?: string | null;
  notes?: string | null;
  message: string;
  message_en?: string;
} | null = null;
let autoUpdateChecked = false;
let updateBannerDismissed = false;
let pending = false,
  polling = false,
  detailKey = "",
  loadSequence = 0,
  lastPollSignature = "",
  connectionError = "";
let benchPreset: "quick" | "full" | "custom" = "quick";
const bench = {
  profile: "standard",
  mode: "read",
  size_mib: 256,
  repeats: 1,
  seconds: 5,
  interval: 0,
  data: "random",
  row: "all",
};
const scan: { mode: string; size_mib: number | "auto" } = {
  mode: "",
  size_mib: "auto",
};
let letter = "";
let unit: "mb_s" | "iops" | "latency_us" = "mb_s";
const livePoints = new Map<string, { read: number; write: number }[]>();
const root = document.querySelector<HTMLDivElement>("#app")!;
const dialog = document.querySelector<HTMLDialogElement>("#confirmation")!;
const theme =
  localStorage.getItem("sentinel-theme") === "light" ? "light" : "dark";
document.documentElement.dataset.theme = theme;
const fmt = (v: unknown, digits = 0) => f(v, digits, getLang());
const fmtBytes = (v: unknown) => bytes(v, getLang());
const syncSkipLink = () => {
  const skip = document.querySelector<HTMLAnchorElement>(".skip-link");
  if (skip) skip.textContent = L("İçeriğe geç", "Skip to content");
};
syncSkipLink();
const date = (v: string | null | undefined) =>
  v
    ? new Date(v).toLocaleString(localeTag(getLang()), {
        day: "2-digit",
        month: "short",
        hour: "2-digit",
        minute: "2-digit",
      })
    : L("Henüz okunmadı", "Not read yet");
const disk = () => state?.disks.find((d) => d.id === selected);
const active = (job: Job | null | undefined) =>
  !!job && ["preparing", "running", "cancelling"].includes(job.status);
const pill = (label: string, type = "") =>
  `<span class="pill ${type}">${e(label)}</span>`;
const statusText = (s: string) =>
  ({
    ok: L("Normal", "Normal"),
    warning: L("İnceleme önerilir", "Attention advised"),
    critical: L("Kritik", "Critical"),
    unknown: L("Bilinmiyor", "Unknown"),
    completed: L("Tamamlandı", "Completed"),
    cancelled: L("İptal edildi", "Cancelled"),
    failed: L("Tamamlanamadı", "Failed"),
    running: L("Çalışıyor", "Running"),
    preparing: L("Hazırlanıyor", "Preparing"),
    cancelling: L("Durduruluyor", "Stopping"),
  })[s] ?? s;
const button = (
  action: string,
  title: string,
  ico: string,
  cls = "",
  disabled = false,
) =>
  `<button type="button" data-action="${action}" class="button ${cls}" ${disabled ? "disabled" : ""}>${icon(ico)}<span>${title}</span></button>`;
const currentJob = (kind: string) =>
  viewedJob?.kind === kind && viewedJob.disk_id === selected
    ? viewedJob
    : state?.job?.kind === kind && state.job.disk_id === selected
      ? state.job
      : null;
const empty = (title: string, detail: string, ico = "info") =>
  `<div class="empty-state">${icon(ico)}<h3>${e(title)}</h3><p>${e(detail)}</p></div>`;

function toast(text: string, error = false) {
  const node = document.querySelector<HTMLDivElement>("#toast")!;
  node.textContent = text;
  node.className = `visible ${error ? "error" : ""}`;
  window.setTimeout(() => {
    if (node.textContent === text) node.className = "";
  }, 6500);
}

function chart(
  points: (number | null)[],
  colour = "var(--accent)",
  label = L("Ölçüm geçmişi", "Measurement history"),
): string {
  const known = points.flatMap((v, i) =>
    v === null || !Number.isFinite(v) ? [] : [{ v, i }],
  );
  if (!known.length)
    return `<div class="chart-empty">${L("Henüz ölçüm yok", "No measurements yet")}</div>`;
  const min = Math.min(...known.map((p) => p.v)),
    max = Math.max(...known.map((p) => p.v));
  const y = (v: number) => 90 - ((v - min) / Math.max(1, max - min)) * 65;
  const x = (i: number) => 12 + (i / Math.max(1, points.length - 1)) * 576;
  const commands: string[] = [];
  let pen = false;
  points.forEach((v, i) => {
    if (v === null || !Number.isFinite(v)) {
      pen = false;
      return;
    }
    commands.push(`${pen ? "L" : "M"}${x(i).toFixed(1)},${y(v).toFixed(1)}`);
    pen = true;
  });
  return `<svg class="line-chart" viewBox="0 0 600 110" preserveAspectRatio="none" role="img" aria-label="${e(label)}"><path d="M0 25h600M0 55h600M0 90h600" class="chart-grid"/><path d="${commands.join(" ")}" stroke="${colour}" stroke-width="2.5" fill="none" vector-effect="non-scaling-stroke"/>${known.length === 1 ? `<circle cx="${x(known[0].i)}" cy="${y(known[0].v)}" r="3" fill="${colour}"/>` : ""}</svg>`;
}

function navItem([key, title, ico]: [Page, string, string]): string {
  const alerts = events.filter((v) => v.level !== "info").length;
  return `<button type="button" class="nav-item ${page === key ? "selected" : ""}" data-page="${key}" ${page === key ? 'aria-current="page"' : ""}>${icon(ico)}<span>${title}</span>${key === "history" && alerts ? `<span class="nav-count">${alerts}</span>` : ""}</button>`;
}

function navigation(): string {
  const lang = getLang();
  const pages = getPages();
  const verShort = state?.version?.split(".").slice(0, 2).join(".") || "1.10";
  return `<aside class="sidebar">
    <div class="brand"><span class="brand-mark"><img class="brand-logo" src="${brandLogo}" alt="" /></span><div>SSD Sentinel<small>STORAGE DIAGNOSTICS</small></div><span class="version">${e(verShort)}</span></div>
    <div class="nav-label">${L("ANA İŞLEMLER", "CORE WORKSPACE")}</div>
    <nav aria-label="${L("Ana navigasyon", "Primary navigation")}">${pages.filter(([key]) => mainPages.has(key)).map(navItem).join("")}</nav>
    <div class="nav-label">${L("DETAYLAR", "DIAGNOSTICS")}</div>
    <nav aria-label="${L("Ayrıntılı ekranlar", "Detailed views")}">${pages.filter(([key]) => !mainPages.has(key)).map(navItem).join("")}</nav>
    <div class="nav-label disk-label"><span>${L("BAĞLI DİSKLER", "CONNECTED DRIVES")}</span><span>${state?.disks.length ?? 0}</span></div>
    <div class="disk-list" aria-label="${L("Disk seçimi", "Drive selector")}">${state?.disks.length ? state.disks.map((d) => `<button type="button" data-disk="${d.id}" class="disk-tile ${selected === d.id ? "selected" : ""}" aria-pressed="${selected === d.id}"><span class="disk-symbol">${icon("disk")}</span><span class="disk-tile-main"><strong>${e(d.model)}</strong><small>${e(translateDiskType(d.type, lang))} <span>·</span> ${fmtBytes(d.metrics.capacity_bytes)}</small><span class="disk-tile-bottom"><i class="dot ${d.health.status}"></i>${d.health.percent === null ? L("Bilinmiyor", "Unknown") : L(`%${d.health.percent}`, `${d.health.percent}%`)}<em>${fmt(d.metrics.temperature_c)} °C</em></span></span></button>`).join("") : `<div class="sidebar-empty">${L("Disk bilgileri bekleniyor", "Waiting for drive telemetry")}</div>`}</div>
    <div class="sidebar-bottom"><div class="local-badge">${icon("shield")}<div><strong>${L("Yerel ve size özel", "Local & private")}</strong><small>${L("Verileriniz bu cihazda kalır", "All data stays on this device")}</small></div></div><div class="sidebar-controls"><button type="button" class="theme-button" data-action="theme">${icon(document.documentElement.dataset.theme === "dark" ? "sun" : "moon")}<span>${document.documentElement.dataset.theme === "dark" ? L("Açık tema", "Light theme") : L("Koyu tema", "Dark theme")}</span>${icon("settings")}</button><div class="lang-toggle" role="group" aria-label="${L("Dil seçimi", "Language")}"><button type="button" data-lang="tr" class="${lang === "tr" ? "selected" : ""}" aria-pressed="${lang === "tr"}">TR</button><button type="button" data-lang="en" class="${lang === "en" ? "selected" : ""}" aria-pressed="${lang === "en"}">EN</button></div></div></div>
  </aside>`;
}

function header(d?: Disk): string {
  const warnings =
    state?.disks.reduce(
      (n, d) =>
        n +
        d.alerts.filter((a) => a.level === "warning" || a.level === "critical")
          .length,
      0,
    ) ?? 0;
  const pageTitle = getPages().find((p) => p[0] === page)?.[1];
  const writeLocked = isWriteLocked(state);
  const guardBtn =
    state?.source === "live"
      ? `<button type="button" data-action="toggle-write-guard" class="button compact write-guard-btn ${writeLocked ? "locked" : "unlocked"}" aria-pressed="${!writeLocked}" title="${e(writeLocked ? L("Salt-okunur koruma etkin: disklere yazan işlemler kilitli", "Read-Only Guard active: mutating operations blocked") : L("Yazma modu açık: kilitlemek için tıklayın", "Write mode unlocked: click to re-lock Read-Only"))}">${icon(writeLocked ? "lock" : "unlock")}<span>${writeLocked ? L("Salt-Okunur", "Read-Only") : L("Yazma Açık", "Write Unlocked")}</span></button>`
      : "";
  return `<header class="toolbar"><div class="breadcrumb">${L("Depolama", "Storage")} <span>/</span> <strong>${e(pageTitle)}</strong></div><div class="toolbar-actions"><span class="connection ${state?.source === "replay" ? "recorded" : state?.stale ? "stale" : ""}"><i></i>${state?.source === "replay" ? L("Kaydedilmiş ölçüm", "Snapshot replay") : state?.stale ? L("Ölçüm güncel değil", "Stale telemetry") : L("Canlı izleme", "Live monitoring")}</span>${guardBtn}<button type="button" class="icon-button notification-button" data-goto="history" aria-label="${L("Uyarıları aç", "Open alerts")}">${icon("bell")}${warnings ? "<i></i>" : ""}</button><span class="toolbar-divider"></span><button type="button" class="button compact accent-outline toolbar-recovery-btn ${page === "recovery" ? "toggled" : ""}" data-goto="recovery">${icon("recover")}<span>${L("Dosya Kurtar", "File Recovery")}</span></button>${!state?.admin && state?.source === "live" ? button("elevate", L("Yönetici olarak aç", "Run as Admin"), "lock", "compact primary", pending) : ""}${button("refresh", state?.updating ? L("Okunuyor…", "Reading…") : L("Yenile", "Refresh"), "refresh", "compact", pending || !!state?.updating)}${button("export", L("Rapor al", "Export JSON"), "export", "compact quiet", !d)}${button("export-html", "HTML", "export", "compact quiet", !d)}</div></header>`;
}

function heading(
  d: Disk,
  kicker: string,
  title: string,
  subtitle: string,
  extra = "",
): string {
  const typeLabel = translateDiskType(d.type, getLang());
  return `<div class="page-heading"><div><div class="eyebrow">${e(kicker)}</div><h1>${e(title)}</h1><p>${e(subtitle)}</p></div><div class="heading-meta">${extra || `${pill(typeLabel)}<span>${icon("clock")}${date(state?.updated_at)}</span>`}</div></div>`;
}

function forecastCard(d: Disk): string {
  const lang = getLang();
  const fc = healthForecast;
  const isHdd = d.type.includes("HDD");
  const tFc = fc ? translateForecast(fc, isHdd, lang) : null;
  if (!fc || !fc.ready) {
    return `<article class="card forecast-card">
      <div class="section-title">
        <div>
          <h2>${L("Ömür &amp; Sağlık Öngörüsü", "Endurance &amp; Health Forecast")}</h2>
          <p>${L("Gerçek yerel ölçüm geçmişine dayalı eğilim analizi", "Trend analysis based on local telemetry history")}</p>
        </div>
        ${pill(L("Veri birikiyor", "Collecting samples"))}
      </div>
      <div class="forecast-placeholder">
        ${icon("clock")}
        <div>
          <strong>${L("Öngörü için birkaç saatlik ölçüm geçmişi birikiyor", "Collecting a few hours of local history for projection")}</strong>
          <p>${e(tFc?.summary || L("Günlük yazma hızı, tahmini kalan ömür ve 7 günlük hata eğilimi en az 1 saat aralıklı iki yerel ölçümden sonra hesaplanır.", "Average daily writes, estimated remaining life, and 7-day error trends require at least two local measurements taken at least 1 hour apart."))}</p>
        </div>
      </div>
    </article>`;
  }
  const risk =
    fc.risk_level === "critical"
      ? "critical"
      : fc.risk_level === "warning"
        ? "warning"
        : "ok";
  const riskLabel =
    risk === "critical"
      ? L("Yüksek Risk", "High Risk")
      : risk === "warning"
        ? L("İzleme Önerilir", "Monitor Closely")
        : L("Kararlı Eğilim", "Stable Trend");
  const pDelta = Number(fc.pending_delta_7d ?? 0);
  const rDelta = Number(fc.reallocated_delta_7d ?? 0);
  const uDelta = Number(fc.uncorrectable_delta_7d ?? 0);
  const anyDelta = pDelta > 0 || rDelta > 0 || uDelta > 0;
  const deltaText = anyDelta
    ? [
        pDelta > 0 ? `+${pDelta} ${L("bekleyen", "pending")}` : "",
        rDelta > 0 ? `+${rDelta} ${L("eşlenen", "reallocated")}` : "",
        uDelta > 0 ? `+${uDelta} ${L("düzeltilemeyen", "uncorrectable")}` : "",
      ]
        .filter(Boolean)
        .join(" · ")
    : L("7 gün hata artışı: 0", "7d error growth: 0");
  const yearsText =
    fc.estimated_remaining_years != null
      ? fc.estimated_remaining_years >= 20
        ? L("20+ yıl", "20+ yr")
        : `~${fmt(fc.estimated_remaining_years, 1)} ${L("yıl", "yr")}`
      : isHdd
        ? L("Sayaç tabanlı", "Counter-based")
        : L("Hesaplanamadı", "Unavailable");
  return `<article class="card forecast-card forecast-${risk}">
    <div class="section-title">
      <div>
        <h2>${L("Ömür &amp; Sağlık Öngörüsü", "Endurance &amp; Health Forecast")}</h2>
        <p>${fc.sample_count ? `${fmt(fc.sample_count)} ${L("yerel ölçüm", "local samples")}` : L("Yerel ölçüm geçmişi", "Local sample history")}${fc.span_hours ? ` · ${fmt(fc.span_hours, 1)} ${L("saat aralık", "h span")}` : ""}</p>
      </div>
      <div class="inline-actions">
        ${pill(deltaText, anyDelta ? (uDelta > 0 || pDelta > 0 ? "critical" : "warning") : "ok")}
        ${pill(riskLabel, risk)}
      </div>
    </div>
    <div class="forecast-metrics">
      <div><span>${L("Günlük yazma", "Daily writes")}</span><strong>${fc.daily_written_gb != null ? `${fmt(fc.daily_written_gb, 1)} ${L("GB/gün", "GB/day")}` : "—"}</strong></div>
      <div><span>${L("Tahmini kalan ömür", "Est. remaining life")}</span><strong>${yearsText}</strong></div>
      <div><span>${L("7 gün ort. sıcaklık", "7d avg temperature")}</span><strong>${fc.temp_avg_7d != null ? `${fmt(fc.temp_avg_7d, 1)} °C` : "—"}</strong></div>
    </div>
    ${tFc?.summary ? `<p class="forecast-summary">${e(tFc.summary)}</p>` : ""}
    ${tFc?.advice ? `<p class="forecast-advice">${icon("shield")}<span>${e(tFc.advice)}</span></p>` : ""}
  </article>`;
}

function overview(d: Disk): string {
  const lang = getLang();
  const health = d.health.percent;
  const warnings = d.alerts.filter((a) => a.level !== "info");
  const c =
    d.health.status === "critical"
      ? "critical"
      : warnings.length
        ? "warning"
        : "ok";
  const capacity = d.volumes.reduce((n, v) => n + (v.size_bytes ?? 0), 0),
    free = d.volumes.reduce((n, v) => n + (v.free_bytes ?? 0), 0);
  const used = capacity ? Math.round((1 - free / capacity) * 100) : null;
  const ifaceLabel = d.interface
    ? formatInterface(d.interface, d.type)
    : L("Arayüz bilgisi yok", "No interface reported");
  const linkLabel =
    d.bus_link?.current ??
    (d.interface ? formatInterface(d.interface, d.type) : L("Bilinmiyor", "Unknown"));
  const healthExp = translateHealthExplanation(d, lang);
  const translatedWarnings = warnings.map((a) => ({
    ...a,
    ...translateAlert(a, lang),
  }));
  return `${heading(d, L("DİSK DURUMU", "DRIVE OVERVIEW"), d.model, L("Diskinizin durumu tek bakışta. Ne yapacağınızı bilmiyorsanız aşağıdaki sırayı izleyin.", "Your drive status at a glance. Follow the steps below if you're unsure where to start."))}
    ${startGuide(d, c)}
    <section class="overview-grid" aria-label="${L("Disk sağlık özeti", "Drive health summary")}">
      <article class="card health-card"><div class="card-title">${icon("shield")}<h2>${L("Sağlık ve dayanım", "Health &amp; endurance")}</h2><span class="card-tag">SMART</span></div><div class="health-main"><div class="health-ring ${c}"><svg viewBox="0 0 160 160" aria-hidden="true"><circle class="ring-track" cx="80" cy="80" r="67"/><circle class="ring-progress" cx="80" cy="80" r="67" stroke-dasharray="421" stroke-dashoffset="${health === null ? 421 : 421 * (1 - health / 100)}"/></svg><div><strong>${health ?? "—"}<small>${health === null ? "" : "%"}</small></strong><span>${health === null ? L("BİLİNMİYOR", "UNKNOWN") : L("KALAN DAYANIM", "ENDURANCE LEFT")}</span></div></div><div class="health-description">${pill(health === null ? L("Puan doğrulanamadı", "Score unverified") : c === "ok" ? L("Normal durumda", "Healthy") : L("İnceleme önerilir", "Attention advised"), c)}<h3>${health === null ? L("Daha fazla veri gerekli", "More data required") : c === "ok" ? L("Diskiniz iyi görünüyor.", "Your drive looks healthy.") : L("Önce verilerinizi koruyun.", "Protect your data first.")}</h3><p>${e(health === null ? healthExp : (translatedWarnings[0]?.detail ?? L("İzlenen sağlık alanlarında bir uyarı bulunmuyor. Düzenli yedek almayı unutmayın.", "No warnings in monitored SMART attributes. Continue maintaining regular backups.")))}</p></div></div><details class="health-method" data-details-key="health-method"><summary>${L("Bu değer nasıl hesaplanıyor?", "How is this score calculated?")}${icon("chevron")}</summary><p>${e(healthExp)}</p></details></article>
      <article class="card temperature-card"><div class="card-title">${icon("temperature")}<h2>${L("Sıcaklık", "Temperature")}</h2><span class="mini-status ${Number(d.metrics.temperature_c) >= 55 ? "warning" : ""}">${d.metrics.temperature_c === null ? L("Bilinmiyor", "Unknown") : Number(d.metrics.temperature_c) >= 55 ? L("Yüksek", "High") : L("Normal", "Normal")}</span></div><div class="temperature-value">${fmt(d.metrics.temperature_c)}<span>°C</span></div><div class="temperature-scale"><span></span><i style="left:${Math.min(96, Math.max(2, Number(d.metrics.temperature_c) || 0))}%"></i></div><div class="scale-labels"><span>0 °C</span><span>50 °C</span><span>100 °C</span></div><p class="muted">${d.metrics.temperature_c === null ? L("Aygıt bu ölçümü bildirmedi.", "Device did not report temperature.") : L("Üreticinin bildirdiği anlık sıcaklık.", "Real-time vendor sensor reading.")}</p></article>
      ${metricCard(L("Toplam yazılan", "Total written"), fmtBytes(d.metrics.written_bytes), L("Ömür boyu host yazması", "Lifetime host writes"), "export")}
      ${metricCard(L("Çalışma süresi", "Power-on time"), `${fmt(d.metrics.power_on_hours)} <small>${L("saat", "hrs")}</small>`, `${fmt(d.metrics.power_cycles)} ${L("güç döngüsü", "power cycles")}`, "clock")}
      ${metricCard(L("Disk kapasitesi", "Drive capacity"), fmtBytes(d.metrics.capacity_bytes), `${e(ifaceLabel)}`, "disk")}
      ${metricCard(L("Kullanılan alan", "Space used"), used === null ? "—" : `${used}<small>%</small>`, capacity ? `${fmtBytes(free)} ${L("boş alan", "free")}` : L("Windows bölüm eşlemesi yok", "No mapped Windows volume"), "surface")}
    </section>
    ${forecastCard(d)}
    <div class="two-columns overview-lower"><article class="card"><div class="section-title"><div><h2>${L("Sıcaklık geçmişi", "Temperature history")}</h2><p>${state?.source === "replay" ? L("Kayıt dosyasının gerçek yakalama zamanı", "Snapshot capture timestamp") : L("Yerel geçmiş · son 720 ölçüm", "Local history · last 720 samples")}</p></div><span class="chart-key"><i></i>${L("Sıcaklık", "Temp")}</span></div>${chart(
      history.map((v) => v.temperature_c),
      "var(--accent)",
      L("Kaydedilmiş sıcaklık ölçümleri", "Recorded temperature history"),
    )}<div class="chart-footer"><span>${history.length ? date(history[0].time) : L("Ölçüm bekleniyor", "Waiting for samples")}</span><span>${history.length === 1 ? L("1 gerçek ölçüm · eğilim için daha fazla örnek gerekli", "1 real measurement · collect more for trend") : `${history.length} ${L("ölçüm", "samples")}`}</span></div></article><article class="card"><div class="section-title"><div><h2>${L("Dikkat edilmesi gerekenler", "Health notices")}</h2><p>${L("Diskin bildirdiği uyarılar", "Alerts reported by the drive")}</p></div><span class="small-count">${translatedWarnings.length}</span></div>${
      translatedWarnings.length
        ? translatedWarnings
            .slice(0, 3)
            .map(
              (a) =>
                `<div class="notice-row ${a.level}"><span class="notice-icon">${icon("alert")}</span><div><strong>${e(a.title)}</strong><p>${e(a.detail)}</p></div></div>`,
            )
            .join("")
        : `<div class="all-clear">${icon("check")}<div><strong>${L("Etkin sağlık uyarısı yok", "No active health warnings")}</strong><p>${L("Düzenli yedek ve izleme ile devam edin.", "Maintain regular backups and monitoring.")}</p></div></div>`
    }</article></div>
    <article class="card volume-card"><div class="section-title"><div><h2>${L("Bölümler ve disk bilgisi", "Volumes &amp; hardware details")}</h2><p>${L("Kimliği doğrulanmış Windows eşlemesi", "Verified Windows partition mapping")}</p></div>${pill(state?.admin ? L("Yönetici erişimi", "Administrator access") : L("Standart erişim", "Standard access"))}</div>${
      d.volumes.length
        ? `<div class="volumes">${d.volumes
            .map((v) => {
              const used =
                v.size_bytes && v.free_bytes !== null
                  ? Math.round((1 - v.free_bytes / v.size_bytes) * 100)
                  : null;
              return `<div class="volume"><span class="volume-letter">${e(v.letter)}:</span><div><strong>${e(v.label || L("Yerel Disk", "Local Disk"))}<small>${e(v.filesystem)}</small></strong><div class="volume-track"><span style="width:${used ?? 0}%" class="${used !== null && used > 90 ? "warning" : ""}"></span></div><p>${fmtBytes(v.free_bytes)} ${L("boş", "free")} <span>/ ${fmtBytes(v.size_bytes)}</span></p></div><b>${used === null ? "—" : used + "%"}</b></div>`;
            })
            .join("")}</div>`
        : `<div class="inline-empty">${icon("info")}${
            state?.source === "live" && d.os_disk_number != null
              ? L(
                  `Disk Windows Disk ${d.os_disk_number} ile eşleşti, ancak harf atanmış bir Windows bölümü bulunamadı (harfsiz, biçimlendirilmemiş veya çok diskli dinamik/RAID birim olabilir).`,
                  `Matched to Windows Disk ${d.os_disk_number}, but no mounted Windows volume with a drive letter was found (unmounted, raw, or multi-disk dynamic/RAID volume).`,
                )
              : L(
                  "Bu kaynakta güvenilir bölüm eşlemesi yok. Kapasite ve doluluk tahmin edilmiyor.",
                  "No verified Windows volume mapping available on this source.",
                )
          }</div>`
    }<div class="device-facts"><div>${L("Seri numarası", "Serial number")}<strong>${e(d.serial ?? L("Bilinmiyor", "Unknown"))}</strong></div><div>Firmware<strong>${e(d.firmware ?? L("Bilinmiyor", "Unknown"))}</strong></div><div>${L("Bağlantı hattı", "Bus link")}<strong>${e(linkLabel)}${d.bus_link?.bottleneck ? ` · ${L("Darboğaz", "Bottleneck")}` : ""}</strong></div><div>${L("Hizalama / Önbellek", "Alignment / Write cache")}<strong>${d.optimization?.aligned_4k === true ? L("4K Hizalı", "4K Aligned") : d.optimization?.aligned_4k === false ? L("Hizasız", "Misaligned") : "—"} · ${d.optimization?.write_cache?.enabled === true ? L("Önbellek açık", "Cache on") : d.optimization?.write_cache?.enabled === false ? L("Önbellek kapalı", "Cache off") : "—"}</strong></div><div>${L("Aygıt yolu", "Device path")}<strong>${e(d.device)}</strong></div><div>${L("TRIM desteği", "TRIM support")}<strong>${d.capabilities.trim === null ? L("Bilinmiyor", "Unknown") : d.capabilities.trim ? L("Destekleniyor", "Supported") : L("Bildirilmedi", "Not reported")}</strong></div></div></article>`;
}

function hasSurfaceSigns(d: Disk): boolean {
  return d.smart.some(
    (r) =>
      ["5", "196", "197", "198", "media_errors"].includes(r.id) &&
      Number.parseFloat(String(r.raw).replace(/[^\d.]/g, "")) > 0,
  );
}

function startGuide(d: Disk, status: string): string {
  const isHdd = d.type.includes("HDD");
  const surfaceFirst = status !== "ok" || hasSurfaceSigns(d);
  const step = (
    key: "optimize" | "surface" | "recovery",
    no: string,
    ico: string,
    title: string,
    text: string,
    cta: string,
    when: string,
  ) => {
    const recommended =
      (key === "optimize" && !surfaceFirst) || (key === "surface" && surfaceFirst);
    return `<article class="quick-hero-card guide-step ${key}-spot ${recommended ? "recommended" : ""}">
      <div class="quick-hero-top"><span class="guide-no">${no}</span>${recommended ? pill(L("Önce bunu yapın", "Start here"), "ok") : `<span class="guide-when">${e(when)}</span>`}</div>
      <h2>${icon(ico)}${e(title)}</h2>
      <p>${e(text)}</p>
      <div class="quick-hero-actions"><button type="button" class="button ${recommended || key === "recovery" ? "primary" : ""}" data-goto="${key}">${e(cta)}${icon("arrow")}</button></div>
    </article>`;
  };
  return `<section class="overview-quick-actions" aria-label="${L("Tanılama ve işlem kısayolları", "Diagnostics & actions")}">
    ${step("optimize", "1", "bolt", L("Politika & Katmanlama", "Policies & Tiering"), isHdd ? L("Yazma önbelleği, APM ve dosya düzenini güvenli politikayla uygular.", "Applies write caching, APM, and layout policies safely.") : L("TRIM, PCIe ASPM, yazma önbelleği ve SLC koruması uygular.", "Applies TRIM, PCIe ASPM, write cache, and SLC protection."), L("Politika & Katman", "Policies & Tiering"), L("Her disk için", "For every drive"))}
    ${step("surface", "2", "surface", L("Yüzey & LBA Taraması", "Surface & LBA Scan"), isHdd ? L("Tam salt-okunur LBA taramasıyla zayıf sektörleri bulur ve izole eder.", "Runs read-only LBA scan to locate and isolate weak sectors.") : L("Salt-okunur LBA haritası çıkarır; yavaş blokları taze hücrelere taşır.", "Builds read-only LBA map and relocates slow stale blocks."), L("Yüzeyi tara", "Scan surface"), surfaceFirst ? L("Uyarı var", "Alerts present") : L("Yavaşlık veya uyarı varsa", "If slow or warned"))}
    ${step("recovery", "?", "recover", L("Salt-Okunur Kurtarma", "Read-Only Recovery"), L("VSS, $MFT ve imza kazıma ile silinen dosya/uygulamaları geri getirir.", "Recovers deleted files and apps from VSS, $MFT, and carving."), L("Silinenleri bul", "Find deleted items"), L("Sadece dosya kaybettiyseniz", "If files were lost"))}
  </section>`;
}

function selectedVolume(d: Disk) {
  const l = letter || d.volumes[0]?.letter || "";
  return d.volumes.find((v) => v.letter === l) ?? null;
}

/** Mirrors `jobs::auto_budget_mib` so the UI can explain what the core will pick. */
function autoSurfaceMib(mode: string, d: Disk): number {
  const free = selectedVolume(d)?.free_bytes ?? null;
  const isHdd = d.type.includes("HDD");
  const wanted = ["physical_scan", "magnetic_refresh", "bad_zone_isolate"].includes(mode)
    ? 256
    : mode === "data_refresh" && !isHdd
      ? 256
      : 1024;
  const writes = ["data_refresh", "magnetic_refresh", "bad_zone_isolate"].includes(mode);
  return writes && free != null
    ? Math.max(64, Math.min(wanted, Math.floor(free / 10 / 1048576)))
    : wanted;
}

function durationText(minutes: number): string {
  const lang = getLang();
  if (minutes < 1) return L("1 dakikadan az", "Under 1 min", lang);
  if (minutes < 60) return `~${Math.round(minutes)} ${L("dk", "min", lang)}`;
  const hrs = (minutes / 60).toFixed(1);
  return lang === "en" ? `~${hrs} hrs` : `~${hrs.replace(".", ",")} saat`;
}

/** Sequential full-surface read speed assumptions (1 MiB unbuffered, QD1). */
function fullScanEstimate(d: Disk): string {
  const cap = Number(d.metrics.capacity_bytes) || 0;
  if (!cap) return L("Diskin tamamı baştan sona okunur.", "Reads the entire drive sequentially.");
  const isHdd = d.type.includes("HDD");
  const nvme = /nvme/i.test(`${d.type} ${d.interface ?? ""}`);
  const mbps = isHdd ? 140 : nvme ? 1500 : 480;
  return L(
    `Diskin tamamı (${fmtBytes(cap)}) baştan sona sektör sektör okunur · tahmini ${durationText(cap / 1e6 / mbps / 60)}. Yazma yapmaz, istediğiniz an durdurabilirsiniz.`,
    `Reads the entire drive (${fmtBytes(cap)}) sector by sector · est. ${durationText(cap / 1e6 / mbps / 60)}. Read-only; stop anytime.`,
  );
}

function deepRecoveryEstimate(d: Disk): string {
  const free = selectedVolume(d)?.free_bytes;
  if (!free) return L("Tüm boş alan taranır.", "Carves all unallocated free space.");
  const scanBytes = Math.min(free, 512 * 1024 ** 3);
  const isHdd = d.type.includes("HDD");
  const nvme = /nvme/i.test(`${d.type} ${d.interface ?? ""}`);
  const mbps = isHdd ? 120 : nvme ? 900 : 400;
  return L(
    `${fmtBytes(scanBytes)} boş alanın tamamı taranır · tahmini ${durationText(scanBytes / 1e6 / mbps / 60)}`,
    `Scans ${fmtBytes(scanBytes)} of unallocated space · est. ${durationText(scanBytes / 1e6 / mbps / 60)}`,
  );
}

function metricCard(
  label: string,
  value: string,
  detail: string,
  ico: string,
): string {
  return `<article class="card metric-card"><div class="metric-label">${icon(ico)}${label}</div><strong class="metric-value">${value}</strong><p>${detail}</p></article>`;
}

function smartView(d: Disk): string {
  const lang = getLang();
  const q = search.toLocaleLowerCase(localeTag(lang));
  const translated = d.smart.map((r) => {
    const t = translateSmartField(r, lang);
    return { r, tName: t.name, tDesc: t.description };
  });
  let rows = translated.filter(
    ({ r, tName }) =>
      (filter === "all" || r.level !== "ok") &&
      `${tName} ${r.name} ${r.original} ${r.id} ${r.raw}`
        .toLocaleLowerCase(localeTag(lang))
        .includes(q),
  );
  if (sort === "raw-desc") rows.sort((a, b) => compareRaw(b.r.raw, a.r.raw, lang));
  if (sort === "raw-asc") rows.sort((a, b) => compareRaw(a.r.raw, b.r.raw, lang));
  const activeEntry =
    translated.find(({ r }) => r.id === field) ?? rows[0];
  const hasNormalized = hasNormalizedSmart(d.smart);
  const showAdv = advanced && hasNormalized;
  return `${heading(d, L("TEKNİK GÖRÜNÜM", "TELEMETRY"), L("SMART verileri", "SMART attributes"), L("Her ölçümün anlamı, ham değeri ve takip etmeniz gereken değişiklikler.", "Raw hardware counters, threshold details, and attribute explanations."))}
  <div class="smart-layout"><article class="card smart-card"><div class="table-tools"><label class="search-field">${icon("search")}<input id="smart-search" type="search" placeholder="${L("Alan adı, ID veya ham değer ara…", "Search attribute name, ID, or raw value…")}" value="${e(search)}" aria-label="${L("SMART alanlarında ara", "Search SMART attributes")}"></label><select id="smart-filter" aria-label="${L("SMART durum filtresi", "SMART level filter")}"><option value="all" ${filter === "all" ? "selected" : ""}>${L("Tüm alanlar", "All attributes")}</option><option value="warnings" ${filter !== "all" ? "selected" : ""}>${L("Uyarı ve bilgiler", "Warnings & notices")}</option></select>${hasNormalized ? `<button class="button compact ${advanced ? "toggled" : ""}" data-action="advanced">${icon("settings")}${L("Gelişmiş", "Advanced")}</button>` : ""}</div><div class="table-wrap"><table class="smart-table"><thead><tr><th>${L("Durum", "Status")}</th><th>${L("SMART alanı", "SMART attribute")}</th><th><button data-action="sort-raw">${L("Ham değer", "Raw value")} ${sort === "raw-desc" ? "↓" : sort === "raw-asc" ? "↑" : "↕"}</button></th>${showAdv ? `<th>${L("Güncel", "Current")}</th><th>${L("En kötü", "Worst")}</th><th>${L("Eşik", "Thresh")}</th>` : ""}</tr></thead><tbody>${rows.map(({ r, tName }) => `<tr class="${activeEntry?.r.id === r.id ? "highlighted" : ""}"><td><i class="dot ${r.level}"></i><span class="sr-only">${e(statusText(r.level))}</span></td><td><button type="button" data-field="${e(r.id)}"><strong>${e(tName)}</strong><small>${e(r.id)} <span>·</span> ${e(r.original)}</small></button></td><td class="mono raw">${e(r.raw)}</td>${showAdv ? `<td class="mono">${fmt(r.value)}</td><td class="mono">${fmt(r.worst)}</td><td class="mono">${fmt(r.threshold)}</td>` : ""}</tr>`).join("") || `<tr><td colspan="${showAdv ? 6 : 3}">${L("Aramanızla eşleşen alan bulunamadı.", "No matching SMART attributes found.")}</td></tr>`}</tbody></table></div><footer class="table-footer"><span>${rows.length} / ${d.smart.length} ${L("alan", "attributes")}</span><span>${hasNormalized ? L("Ham sayılar kayıpsız gösterilir", "Raw hardware values shown unrounded") : L("NVMe ham sayaçları (normalize eşik kullanılmaz)", "NVMe raw counters (no normalized thresholds)")}</span></footer></article><aside class="smart-inspector card">${activeEntry ? `<div class="inspector-icon">${icon("info")}</div><div class="eyebrow">${L("ALAN AÇIKLAMASI", "ATTRIBUTE DETAIL")}</div><h2>${e(activeEntry.tName)}</h2><p>${e(activeEntry.tDesc)}</p><dl><dt>${L("Ham değer", "Raw value")}</dt><dd class="mono">${e(activeEntry.r.raw)}</dd><dt>${L("Alan kimliği", "Attribute ID")}</dt><dd class="mono">${e(activeEntry.r.id)}</dd><dt>${L("Durum", "Status")}</dt><dd>${pill(statusText(activeEntry.r.level), activeEntry.r.level)}</dd></dl><div class="tip">${icon("shield")}${L("Bir sayacın geçmiş toplamı, yeni bir arıza oluştuğu anlamına gelmez. Değişimleri geçmişten izleyin.", "A historical counter total does not always indicate new damage. Watch trends over time in History.")}</div>` : empty(L("SMART alanı yok", "No SMART attributes"), L("Aygıt veri bildirmedi.", "The device reported no SMART rows."))}</aside></div>`;
}

function options(
  id: string,
  label: string,
  entries: [string | number, string][],
  value: string | number,
  extraAttrs = "",
): string {
  return `<label class="form-field"><span>${label}</span><select id="${id}" ${extraAttrs}>${entries.map(([k, v]) => `<option value="${e(k)}" ${String(k) === String(value) ? "selected" : ""}>${e(v)}</option>`).join("")}</select></label>`;
}
function volumeSelect(
  d: Disk,
  id = "target-volume",
  label = L("Hedef bölüm", "Target volume"),
  extraAttrs = "",
): string {
  return options(
    id,
    label,
    d.volumes.length
      ? d.volumes.map((v) => [
          v.letter,
          `${v.letter}: ${v.label || L("Yerel Disk", "Local Disk")} · ${fmtBytes(v.free_bytes)} ${L("boş", "free")}`,
        ])
      : [["", L("Eşleşen bölüm yok", "No mapped volume")]],
    letter,
    extraAttrs,
  );
}
function jobProgress(job: Job | null): string {
  return job
    ? `<div class="job-progress ${job.status}"><div><span>${active(job) ? '<i class="live-dot"></i>' : icon(job.status === "completed" ? "check" : "info")}<strong>${e(statusText(job.status))}</strong></span><b>${job.progress}%</b></div><progress max="100" value="${job.progress}" aria-label="${L("İş ilerlemesi", "Job progress")}"></progress><p>${e(job.phase)}</p>${job.error ? `<p class="error-text">${e(job.error)}</p>` : ""}${job.cleanup_error ? `<p class="error-text">${e(job.cleanup_error)}</p>` : ""}</div>`
    : "";
}
function disabledReason(d: Disk): string {
  if (state?.source === "replay")
    return L("Kayıt modundasınız. Canlı disk işlemleri kapalı.", "Snapshot replay active. Live disk actions are disabled.");
  if (state?.stale)
    return L("Disk bilgisi güncel değil. Önce SMART verilerini yenileyin.", "Telemetry is stale. Refresh SMART data first.");
  if (state?.busy)
    return L("Başka bir okuma / disk işlemi sürüyor.", "Another read or disk operation is in progress.");
  if (d.read_incomplete)
    return L("Bu disk için SMART okuma tamamlanamadı (yönetici izni veya denetleyici erişimi gerekli).", "SMART read for this disk was incomplete (administrator rights or controller access required).");
  if (!canStart(state, d))
    return mappingDisabledReason(state, d, getLang());
  return "";
}

function writeDisabledReason(d: Disk): string {
  const base = disabledReason(d);
  if (base) return base;
  if (isWriteLocked(state)) {
    return L(
      "Salt-okunur koruma etkin. Yazma işlemi için üst çubuktan kilidi açın.",
      "Read-Only Guard is active. Unlock write mode in the top bar first.",
    );
  }
  return "";
}

function performanceView(d: Disk): string {
  const lang = getLang();
  const unitLabels = {
    mb_s: L("MB/sn", "MB/s", lang),
    iops: "IOPS",
    latency_us: "μs",
  };
  const p = state?.performance.disks.find((p) => p.number === d.os_disk_number);
  const points = livePoints.get(d.id) ?? [];
  const job = currentJob("benchmark");
  const rows = state?.profiles[bench.profile] ?? [];
  const can = canStart(state, d) && canWrite(state) && !!state?.engine.available && !pending;
  const seqReadResult = job?.results
    .filter(
      (r) => !r.row.random && (r.direction === "read" || r.direction === "mix"),
    )
    .sort((a, b) => (b.best.mb_s ?? 0) - (a.best.mb_s ?? 0))[0];
  const speedEval = evaluateBenchmarkSpeed(
    seqReadResult?.best.mb_s,
    d.type,
    d.interface,
    d.bus_link,
    lang,
  );
  const verdictHtml = speedEval
    ? `<div class="bench-verdict">
        <div class="inline-actions">
          <span class="eyebrow">${L("DİSKİNE GÖRE DEĞERLENDİRME", "DRIVE EVALUATION", lang)}</span>
          ${pill(speedEval.label, speedEval.level)}
          <span class="mono bench-verdict-speed">${fmt(seqReadResult!.best.mb_s, 1)} ${unitLabels.mb_s} · ${e(speedEval.band)}</span>
        </div>
        <p>${e(speedEval.detail)}</p>
      </div>`
    : job?.results.length
      ? `<div class="bench-verdict bench-verdict-neutral">
          <div class="inline-actions">
            <span class="eyebrow">${L("DİSKİNE GÖRE DEĞERLENDİRME", "DRIVE EVALUATION", lang)}</span>
            ${pill(L("Rastgele 4K ölçümü", "Random 4K measurement", lang))}
          </div>
          <p>${L(`Bu sonuç yalnızca rastgele 4K testi içerir. Diskinizin sınıfına göre (${e(translateDiskType(d.type, lang))}) tam değerlendirme için Hızlı veya Kapsamlı test çalıştırın.`, `This run only measured Random 4K I/O. Run Quick or Full Benchmark for a sequential evaluation of your ${e(translateDiskType(d.type, lang))} drive.`, lang)}</p>
        </div>`
      : "";

  return `${heading(d, L("HIZ TESTİ", "BENCHMARK", lang), L("Diskiniz ne kadar hızlı?", "How fast is your drive?", lang), L("Anlık okuma/yazma hızı ve kontrollü hız testi.", "Live Windows physical throughput counters and Win32 IOCP unbuffered benchmark.", lang))}
    <div class="performance-live"><article class="card"><div class="section-title"><div><h2>${L("Anlık disk etkinliği", "Live disk activity", lang)}</h2><p>${state?.source === "replay" ? L("Kayıt modunda canlı sayaçlar kapalı", "Live counters unavailable during snapshot replay", lang) : (state?.performance.error ?? L("Windows fiziksel disk sayaçları · 3 sn", "Windows physical disk counters · 3s", lang))}</p></div>${pill(p ? L("CANLI", "LIVE", lang) : L("ÖLÇÜM YOK", "NO SIGNAL", lang), p ? "ok" : "")}</div><div class="io-numbers"><div><span><i></i>${L("Okuma", "Read", lang)}</span><strong>${p ? fmt(p.read_bps / 1e6, 2) : "—"}<small>${unitLabels.mb_s}</small></strong></div><div><span><i class="blue"></i>${L("Yazma", "Write", lang)}</span><strong>${p ? fmt(p.write_bps / 1e6, 2) : "—"}<small>${unitLabels.mb_s}</small></strong></div><div><span>${L("Meşguliyet", "Busy", lang)}</span><strong>${p ? fmt(p.busy_percent, 1) : "—"}<small>%</small></strong></div></div>${
      p
        ? chart(
            points.map((p) => p.read),
            "var(--accent)",
            L("Anlık okuma geçmişi", "Live read history", lang),
          )
        : empty(
            L("Canlı ölçüm yok", "No live telemetry", lang),
            L("Bu disk için anlık aktarım ve gecikme bilgisi mevcut değil.", "Live transfer and latency telemetry is unavailable for this drive.", lang),
            "activity",
          )
    }<div class="performance-foot"><span>${L("Okuma", "Read", lang)} ${p ? fmt(p.read_iops) : "—"} IOPS</span><span>${L("Yazma", "Write", lang)} ${p ? fmt(p.write_iops) : "—"} IOPS</span><span>${L("Gecikme", "Latency", lang)} ${fmt(p?.read_latency_ms, 3)} / ${fmt(p?.write_latency_ms, 3)} ms</span></div></article></div>
    <article class="card benchmark-card"><div class="section-title"><div><h2>${L("Hız testi", "Speed benchmark", lang)}</h2><p>SEQ / RND · ${e(state?.engine.name ?? L("Direct-I/O motoru", "Direct-I/O engine", lang))} · ${state?.engine.available ? L("Hazır", "Ready", lang) : L("Kullanılamıyor", "Unavailable", lang)}</p></div><div class="segmented" aria-label="${L("Sonuç birimi", "Result unit", lang)}">${(["mb_s", "iops", "latency_us"] as const).map((k) => `<button data-unit="${k}" class="${unit === k ? "selected" : ""}">${unitLabels[k]}</button>`).join("")}</div></div>
    <div class="benchmark-layout"><div class="benchmark-config">${volumeSelect(d)}
      <div class="bench-presets" role="group" aria-label="${L("Hız testi modu", "Benchmark mode", lang)}">
        <button type="button" class="bench-preset ${benchPreset === "quick" ? "selected" : ""}" data-bench-preset="quick" aria-pressed="${benchPreset === "quick"}">
          <div><strong>${L("Hızlı Test", "Quick Test", lang)}</strong><span class="preset-time">${L("~20 sn", "~20s", lang)}</span></div>
          <small>${L("256 MiB · Okuma · 1 tur · günlük hızlı kontrol", "256 MiB · Read · 1 pass · fast health check", lang)}</small>
        </button>
        <button type="button" class="bench-preset ${benchPreset === "full" ? "selected" : ""}" data-bench-preset="full" aria-pressed="${benchPreset === "full"}">
          <div><strong>${L("Kapsamlı Test", "Full Benchmark", lang)}</strong><span class="preset-time">${L("~2 dk", "~2m", lang)}</span></div>
          <small>${L("1 GiB · Okuma + yazma · 3 tur · tam performans", "1 GiB · Read + Write · 3 passes · peak + sustained", lang)}</small>
        </button>
      </div>
      <details class="advanced-options" data-details-key="bench-advanced"><summary>${L("Ayrıntılı ayarlar", "Advanced options", lang)} ${icon("chevron")}</summary>${options(
        "bench-profile",
        L("Profil", "Profile", lang),
        [
          ["standard", L("Standart", "Standard", lang)],
          ["nvme", "NVMe SSD"],
          ["real", L("Günlük kullanım", "Real-world mix", lang)],
          ["peak", L("Tepe performans", "Peak throughput", lang)],
          ["sustained", L("SLC Önbellek / Sürekli", "SLC Cache / Sustained", lang)],
        ],
        bench.profile,
      )}<div class="form-row">${options(
        "bench-size_mib",
        L("Test dosyası", "Test file", lang),
        [
          [256, "256 MiB"],
          [1024, "1 GiB"],
          [4096, "4 GiB"],
        ],
        bench.size_mib,
      )}${options(
        "bench-repeats",
        L("Tekrar", "Passes", lang),
        [
          [1, L("1 tur", "1 pass", lang)],
          [3, L("3 tur", "3 passes", lang)],
          [5, L("5 tur", "5 passes", lang)],
        ],
        bench.repeats,
      )}</div><div class="form-row">${options(
        "bench-mode",
        L("İş yükü", "Workload", lang),
        [
          ["read", L("Okuma", "Read", lang)],
          ["write", L("Yazma", "Write", lang)],
          ["both", L("Okuma + yazma", "Read + Write", lang)],
          ["mix", L("Karma · %30 yazma", "Mixed · 30% write", lang)],
        ],
        bench.mode,
      )}${options(
        "bench-seconds",
        L("Süre / test", "Duration / row", lang),
        [
          [5, L("5 saniye", "5 seconds", lang)],
          [10, L("10 saniye", "10 seconds", lang)],
          [30, L("30 saniye", "30 seconds", lang)],
        ],
        bench.seconds,
      )}</div><div class="form-row">${options(
        "bench-data",
        L("Test verisi", "Fill pattern", lang),
        [
          ["random", L("Rastgele", "Random (incompressible)", lang)],
          ["zero", L("Sıfır", "All zeros", lang)],
        ],
        bench.data,
      )}${options(
        "bench-interval",
        L("Testler arası", "Cooldown", lang),
        [
          [0, L("Ara yok", "No pause", lang)],
          [5, L("5 saniye", "5 seconds", lang)],
          [10, L("10 saniye", "10 seconds", lang)],
        ],
        bench.interval,
      )}</div></details>${button("start-benchmark", L("Testi başlat", "Run benchmark", lang), "play", "primary full", !can)}<p class="form-hint">${e(writeDisabledReason(d) || (!state?.engine.available ? L("Yerleşik Direct-I/O motoru yalnız Windows'ta çalışır.", "Built-in Direct-I/O engine requires Windows.", lang) : L("Salt okuma testi bile hazırlıkta geçici dosya yazar.", "Even read-only tests create a temporary test file before measuring.", lang)))}</p></div>
    <div class="benchmark-results">${verdictHtml}<div class="benchmark-table"><div class="benchmark-head"><span>TEST</span><span>${L("OKUMA", "READ", lang)}</span><span>${L("YAZMA", "WRITE", lang)}</span></div>${rows
      .map(
        (row) =>
          `<div class="benchmark-result-row"><button data-bench-row="${e(row.id)}" ${!can ? "disabled" : ""}><strong>${e(row.name.split(" ")[0])}</strong><small>${e(row.name.split(" ")[1])}</small></button>${[
            "read",
            "write",
          ]
            .map((dir) => {
              const m = job?.results.find(
                (r) =>
                  r.row.id === row.id &&
                  (r.direction === dir || r.direction === "mix"),
              )?.best;
              const n = m?.[unit];
              return `<div class="score"><span style="width:${n == null ? 0 : Math.min(100, unit === "mb_s" ? n / 70 : unit === "iops" ? n / 10000 : n / 5)}%"></span><strong>${fmt(n, unit === "iops" ? 0 : 2)}</strong><small>${m ? unitLabels[unit] : L("Ölçülmedi", "Not run", lang)}</small></div>`;
            })
            .join("")}</div>`,
      )
      .join(
        "",
      )}</div>${jobProgress(job)}<div class="info-line">${icon("info")}${L("Yerleşik Direct-I/O (Win32 IOCP · FILE_FLAG_NO_BUFFERING) kullanılır; skorlar ham ölçüm değeridir.", "Built-in Direct-I/O (Win32 IOCP · FILE_FLAG_NO_BUFFERING); scores reflect raw uncached measurements.", lang)}</div></div></div></article>
    ${jobHistory(benchmarkHistory, "benchmark")}`;
}

function optRecord(key: string): OptimizationRecord | undefined {
  return state?.optimizations?.find((o) => o.key === key);
}

function optBtn(
  action: string,
  target: string,
  value: string,
  label: string,
  disabled: boolean,
  cls = "quiet",
): string {
  return `<button type="button" class="button compact ${cls}" data-opt-action="${e(action)}" data-opt-target="${e(target)}" data-opt-value="${e(value)}" ${disabled ? "disabled" : ""}>${e(label)}</button>`;
}

function autopilotHero(d: Disk): string {
  const isHdd = d.type === "HDD";
  const lastAuto = currentJob("autopilot") ?? autopilotHistory[0] ?? null;
  const ready = canMaintain(state, d) && canStart(state, d) && !pending;
  const steps = lastAuto?.repair_summary?.steps as string[] | undefined;
  const runningAuto = active(lastAuto);
  const volPicker =
    d.volumes.length > 1
      ? `<div class="autopilot-volume">${volumeSelect(d, "autopilot-volume", L("Hedef bölüm", "Target volume"), 'data-volume-select="autopilot"')}</div>`
      : d.volumes.length === 1
        ? `<div class="autopilot-vol-badge">${pill(L(`Hedef bölüm: ${d.volumes[0].letter}: (${fmtBytes(d.volumes[0].free_bytes)} boş)`, `Target volume: ${d.volumes[0].letter}: (${fmtBytes(d.volumes[0].free_bytes)} free)`), "ok")}</div>`
        : "";
  return `<section class="autopilot-hero" aria-label="${L("Otomatik Politika Profili", "Automated Policy Profile")}">
    <div class="autopilot-hero-main">
      <div class="eyebrow">${icon("shield")} ${L("OTOMATİK POLİTİKA PROFİLİ", "AUTOMATED POLICY PROFILE")}</div>
      <h2>${isHdd ? L("HDD Depolama Politikasını Uygula", "Apply HDD Policy Profile") : L("SSD Dayanım & Performans Politikasını Uygula", "Apply SSD Endurance & Policy Profile")}</h2>
      <p>${
        isHdd
          ? L("Önbellek, kafa park ayarı, dosya sistemi kontrolü ve birleştirmeyi doğru sırayla uygular.", "Applies write caching, APM tuning, NTFS checks, and defrag in safe sequence.")
          : L("TRIM, PCIe güç modu, yazma önbelleği ve ReTrim'i güvenli sırayla uygular.", "Applies Windows TRIM, low-latency PCIe ASPM, write caching, and ReTrim safely.")
      }${!ready && state?.source === "live" ? (!state?.admin ? ` <b>${L("Önce sağ üstten “Yönetici olarak aç”a basın.", "Click “Run as Admin” in the top bar first.")}</b>` : isWriteLocked(state) ? ` <b>${L("Salt-okunur koruma etkin — üst çubuktan kilidi açın.", "Read-Only Guard active — unlock write mode in the top bar first.")}</b>` : "") : ""}</p>
      ${volPicker}
      ${runningAuto ? jobProgress(lastAuto) : ""}
      ${
        !runningAuto && steps?.length
          ? `<ul class="autopilot-steps">${steps
              .map((s) => {
                const warn = /atlandı|uyarı|kritik|⚠|öneri|tasarruf|skipped|warning|critical/i.test(s);
                return `<li class="${warn ? "step-advisory" : ""}">${warn ? "!" : "✓"} ${e(s)}</li>`;
              })
              .join("")}</ul>`
          : ""
      }
    </div>
    ${button("start-autopilot", runningAuto ? L("Çalışıyor…", "Running…") : L("Güvenli Profili Uygula", "Apply Safe Profile"), "bolt", "primary", !ready)}
  </section>`;
}

function mergedTierMoves(analysis: TierAnalysis | null = null): TierMoveRecord[] {
  const byId = new Map<string, TierMoveRecord>();
  for (const m of tierMoves) byId.set(m.id, m);
  for (const m of state?.tier_moves ?? []) {
    if (!byId.has(m.id)) byId.set(m.id, m);
  }
  for (const m of analysis?.active_moves ?? []) {
    if (!byId.has(m.id)) byId.set(m.id, m);
  }
  return Array.from(byId.values());
}

function smartTierResultBanner(job: Job | null): string {
  if (!job || job.status !== "completed" || !job.repair_summary) return "";
  const rs = job.repair_summary as SmartTierSummary;
  const isSsdOpt = rs.mode === "ssd_extent_optimize";
  const extBefore = Number(rs.extents_before ?? 0);
  const extAfter = Number(rs.extents_after ?? 0);
  return `<div class="tier-job-banner" role="region" aria-label="${L("Akıllı katmanlama iş sonucu", "Smart tiering job result")}">
    <div class="tier-job-banner-head">
      <div class="inline-actions">
        ${icon("check")}
        <strong>${isSsdOpt ? L("SSD Akıllı Parça Temizliği + ReTrim Tamamlandı", "SSD Smart Extent Consolidation + ReTrim Completed") : L("HDD Dış-İz Hızlı Yerleşim Tamamlandı", "HDD Outer-Track Placement Completed")}</strong>
      </div>
      <div class="inline-actions">
        ${isSsdOpt ? pill(rs.retrim_ok ? "ReTrim ✓" : L("ReTrim Atlandı", "ReTrim Skipped"), rs.retrim_ok ? "ok" : "warning") : pill(L("ZBR Dış-İz", "ZBR Outer-Track"), "ok")}
      </div>
    </div>
    <div class="tier-metric-grid">
      <div><span>${isSsdOpt ? L("Toparlanan dosya", "Consolidated files") : L("Dış ize alınan dosya", "Outer-track files")}</span><strong>${fmt(isSsdOpt ? (rs.files_consolidated ?? 0) : (rs.files_placed_outer ?? 0))}</strong></div>
      <div><span>${L("Parça (Extent) azaltımı", "Extent reduction")}</span><strong class="mono">${fmt(extBefore)} → ${fmt(extAfter)}</strong></div>
      <div><span>${isSsdOpt ? L("Yazılan veri (2 GiB sınır)", "Data moved (2 GiB cap)") : L("Taşınan veri (4 GiB sınır)", "Data moved (4 GiB cap)")}</span><strong>${fmtBytes(isSsdOpt ? (rs.bytes_written ?? 0) : (rs.bytes_moved ?? 0))}</strong></div>
    </div>
    ${rs.scope ? `<p class="tier-job-scope">${e(rs.scope)}</p>` : ""}
  </div>`;
}

function smartTierCard(
  d: Disk,
  selectedLetter: string,
  sysLive: boolean,
  diskVolOk: boolean,
): string {
  const lang = getLang();
  const isHdd = d.type.includes("HDD");
  const analysis =
    tierAnalysis &&
    tierAnalysis.disk_id === d.id &&
    (!selectedLetter || !tierAnalysis.letter || tierAnalysis.letter === selectedLetter)
      ? tierAnalysis
      : null;
  const companion = analysis
    ? analysis.companion
    : findCompanionDrive(state?.disks ?? [], d);
  const activeTierJob = currentJob("smart_tier") ?? smartTierHistory[0] ?? null;
  const runningTier = active(activeTierJob);
  const canAnalyze =
    !pending && !tierAnalyzing && !tierTransferring && (state?.source === "replay" || !!selectedLetter);
  const headerPillText = isHdd
    ? companion
      ? L("HDD Dış-İz + SSD Terfi Hazır", "HDD Outer-Track + SSD Promote Ready", lang)
      : L("HDD Dış-İz Hazır", "HDD Outer-Track Ready", lang)
    : companion
      ? L("SSD → HDD Katmanlama Hazır", "SSD → HDD Tiering Ready", lang)
      : L("SSD Akıllı Bakım Hazır", "SSD Smart Maintenance Ready", lang);
  const subtitle = isHdd
    ? L(
        "SMART korumalı katmanlama motoru sık kullanılan parçalanmış dosyaları en hızlı dış ZBR izlerine yerleştirir ve sıcak klasörleri şeffaf NTFS Junction ile SSD'ye terfi ettirir.",
        "SMART-guarded tiering engine places frequently accessed fragmented files on fast outer ZBR tracks and promotes hot folders to SSD via transparent NTFS Junctions.",
        lang,
      )
    : L(
        "Klasik defrag yapmaz; aşırı parçalanmış (>50 parça) dosyaları temizler + ReTrim uygular ve kullanılmayan büyük dosya/klasörleri şeffaf Junction ile HDD'ye taşıyarak SSD SLC önbelleğini rahatlatır.",
        "Avoids traditional SSD defrag; consolidates only heavily fragmented (>50 extents) files + ReTrim and offloads cold folders to HDD via transparent Junctions to protect SLC cache.",
        lang,
      );

  const fillPct = analysis
    ? Math.max(0, Math.min(100, Number(analysis.fill_percent) || 0))
    : 0;
  const pressure = analysis
    ? slcPressureBadge(analysis.slc_pressure, !isHdd, lang)
    : null;
  const pressureCls =
    analysis?.slc_pressure === "high"
      ? "high"
      : analysis?.slc_pressure === "moderate"
        ? "moderate"
        : "low";
  const tTier = analysis ? translateTierAnalysis(analysis, fmtBytes, lang) : null;

  const candidateRow = (cand: TierCandidate) => {
    const cat = tierCategoryBadge(cand.category, cand.is_dir, lang);
    const sc = tierScoreBadge(
      cand.score,
      cand.recommended_direction || (isHdd ? "to_ssd" : "to_hdd"),
      lang,
    );
    const actionLabel = isHdd
      ? L(`SSD'ye Terfi Et (${analysis?.companion?.letter ?? ""}:)`, `Promote to SSD (${analysis?.companion?.letter ?? ""}:)`, lang)
      : L(`HDD'ye Taşı (${analysis?.companion?.letter ?? ""}:)`, `Offload to HDD (${analysis?.companion?.letter ?? ""}:)`, lang);
    const reason = tTier?.reasons.get(cand.id) ?? cand.reason_tr;
    return `<div class="tier-candidate-row">
      <div class="tier-cand-main">
        <div class="inline-actions">
          <strong>${e(cand.name)}</strong>
          ${pill(cat.label, cat.level)}
          ${pill(sc.label, sc.level)}
          ${cand.is_dir ? pill(L("NTFS Junction (Kısayol bozulmaz)", "NTFS Junction (Paths intact)", lang), "ok") : ""}
        </div>
        <small class="mono tier-cand-path">${e(cand.path)} · ${fmtBytes(cand.size_bytes)}${cand.is_dir && cand.file_count ? ` (${fmt(cand.file_count)} ${L("dosya", "files", lang)})` : ""} · ${e(formatUntouchedDays(cand.days_untouched, lang))}</small>
        ${reason ? `<p class="tier-cand-reason">${e(reason)}</p>` : ""}
      </div>
      <div class="tier-cand-action">
        ${
          analysis?.companion
            ? `<button type="button" class="button compact primary" data-tier-relocate="${e(cand.id)}" ${!diskVolOk || runningTier || tierTransferring ? "disabled" : ""}>${icon(isHdd ? "bolt" : "arrow")}<span>${e(actionLabel)}</span></button>`
            : pill(L("Hedef sürücü yok", "No target drive", lang))
        }
      </div>
    </div>`;
  };

  const activeMoves = mergedTierMoves(analysis);

  const analysisHtml = analysis
    ? `<div class="tier-analysis-panel">
        ${
          analysis.replay
            ? `<div class="mode-note">${icon("info")}<span>${e(L(analysis.summary_tr || analysis.slc_note_tr || "Kayıt modunda dosya sistemi taraması yapılamaz; canlı modda klasör ve parça analizi etkinleşir.", "Filesystem tiering scan is disabled in snapshot replay; switch to a live machine to analyze folders and extents.", lang))}</span></div>`
            : `<div class="tier-slc-strip">
                 <div class="tier-slc-top">
                   <div class="inline-actions">
                     <strong>${isHdd ? L("Bölüm Doluluğu & Parçalanma", "Volume Usage & Fragmentation", lang) : L("SLC Önbellek & Boş Alan", "SLC Cache & Free Space", lang)}</strong>
                     ${pressure ? pill(pressure.label, pressure.level) : ""}
                   </div>
                   <span class="mono tier-fill-label">${L(`%${fmt(fillPct, 1)} dolu`, `${fmt(fillPct, 1)}% full`, lang)}</span>
                 </div>
                 <div class="tier-fill-track" aria-hidden="true"><span class="${pressureCls}" style="width:${fillPct.toFixed(1)}%"></span></div>
                 <div class="tier-slc-metrics">
                   <div><span>${L("Boş / Toplam alan", "Free / Total", lang)}</span><strong>${fmtBytes(analysis.free_bytes)} / ${fmtBytes(analysis.size_bytes)}</strong></div>
                   <div><span>${L("Aşırı parçalanmış (&gt;50 parça)", "Heavily fragmented (&gt;50 ext)", lang)}</span><strong>${fmt(analysis.extreme_fragmented_count)} ${L("dosya", "files", lang)}${analysis.worst_extents ? ` · ${L("maks", "max", lang)} ${fmt(analysis.worst_extents)}` : ""}</strong></div>
                   <div><span>${isHdd ? L("Terfi adayı toplamı", "Promotion candidate pool", lang) : L("Kazanılabilir SSD alanı", "Reclaimable SSD space", lang)}</span><strong>${fmtBytes(analysis.total_reclaimable_bytes)}</strong></div>
                 </div>
                 ${tTier?.slcNote ? `<p class="tier-slc-note">${e(tTier.slcNote)}</p>` : ""}
               </div>
               ${
                 !analysis.companion
                   ? `<div class="mode-note">${icon("info")}<span>${
                       isHdd
                         ? L("SSD'ye terfi için en az 20 GiB boş alanı olan bağlı bir NTFS SSD bulunamadı; HDD Dış-İz Hızlı Yerleşim hemen kullanılabilir.", "No companion NTFS SSD with >=20 GiB free space found; HDD Outer-Track Placement is ready right now.", lang)
                         : L("Katmanlama için bağlı NTFS HDD bulunamadı; SSD Akıllı Parça Temizliği + ReTrim hemen kullanılabilir.", "No companion NTFS HDD detected; SSD Smart Extent Consolidation + ReTrim is ready right now.", lang)
                     }</span></div>`
                   : ""
               }
               ${
                 analysis.candidates.length
                   ? `<div class="tier-candidates-section">
                        <div class="tier-subhead">
                          <h3>${isHdd ? L("Sıcak Veri Adayları (HDD → SSD Terfi)", "Hot Data Candidates (HDD → SSD Promote)", lang) : L("Soğuk Veri Adayları (SSD → HDD Katmanlama)", "Cold Data Candidates (SSD → HDD Offload)", lang)}</h3>
                          <span class="small-count">${analysis.candidates.length}</span>
                        </div>
                        <div class="tier-candidate-list">${analysis.candidates.map(candidateRow).join("")}</div>
                      </div>`
                   : `<div class="mode-note">${icon("check")}<span>${e(tTier?.summary || (isHdd ? L("Bu bölümde SSD'ye terfi edilecek sıcak klasör tespit edilmedi.", "No hot folders detected for SSD promotion on this volume.", lang) : L("Bu bölümde taşınmaya uygun büyük soğuk dosya veya klasör tespit edilmedi.", "No large cold files or folders detected for offload on this volume.", lang)))}</span></div>`
               }`
        }
      </div>`
    : "";

  const movesHtml = activeMoves.length
    ? `<div class="tier-moves-section">
         <div class="tier-subhead">
           <h3>${L("Aktif Taşınmış Öğeler &amp; Geri Alma", "Active Relocated Items &amp; Rollback", lang)}</h3>
           <span class="small-count">${activeMoves.length}</span>
         </div>
         <div class="tier-move-list">${activeMoves
           .map((m) => {
             const label =
               m.source_path.split(/[\\/]/).filter(Boolean).pop() ||
               m.source_path;
             const rollbackLabel =
               m.direction === "to_ssd"
                 ? L("HDD'ye Geri Al", "Restore to HDD", lang)
                 : m.direction === "to_archive"
                   ? L("Orijinal Konuma Geri Al", "Restore Original Location", lang)
                   : L("SSD'ye Geri Al", "Restore to SSD", lang);
             return `<div class="tier-move-row">
               <div class="tier-move-main">
                 <div class="inline-actions">
                   <strong>${e(label)}</strong>
                   ${
                      m.direction === "to_ssd"
                        ? pill(L("HDD → SSD Terfi", "HDD → SSD Promote", lang), "ok")
                        : m.direction === "to_archive"
                          ? pill(L("Arşiv Junction", "Archive Junction", lang), "info")
                          : pill(L("SSD → HDD Katman", "SSD → HDD Tier", lang), "info")
                    }
                   ${m.junction_created ? pill("NTFS Junction", "ok") : ""}
                 </div>
                 <small class="mono tier-move-paths">${e(m.source_path)} → ${e(m.target_path)}</small>
                 <small class="muted">${fmtBytes(m.size_bytes)}${m.file_count ? ` · ${fmt(m.file_count)} ${L("dosya", "files", lang)}` : ""} · ${date(m.moved_at)}</small>
               </div>
               <div class="tier-move-action">
                 <button type="button" class="button compact quiet" data-tier-rollback="${e(m.id)}" ${!sysLive || tierTransferring ? "disabled" : ""}>${icon("reset")}<span>${e(rollbackLabel)}</span></button>
               </div>
             </div>`;
           })
           .join("")}</div>
       </div>`
    : "";

  return `<article class="card tiering-card" id="smart-tiering-card" aria-label="${L("Akıllı Depolama Katmanlama ve Optimizasyon", "Smart Storage Tiering & Optimization", lang)}">
    <div class="tier-head">
      <div class="tier-head-main">
        <div class="eyebrow">${icon("cpu")} ${L("DEPOLAMA KATMANLAMA MOTORU", "SMART STORAGE TIERING", lang)}</div>
        <h2>${L("Akıllı Depolama Katmanlama &amp; Optimizasyon", "Smart Storage Tiering &amp; Optimization", lang)}</h2>
      </div>
      ${pill(headerPillText, "ok")}
    </div>
    <p class="tier-desc">${e(subtitle)}</p>
    <div class="tier-actions">
      ${
        isHdd
          ? `<button type="button" class="button primary" data-tier-job="hdd_outer_optimize" ${!diskVolOk || runningTier || tierTransferring ? "disabled" : ""}>${icon("bolt")}<span>${L("HDD Dış-İz Hızlı Yerleşim", "HDD Outer-Track Placement", lang)}</span></button>
             <button type="button" class="button accent-outline" data-action="analyze-tiering" ${!canAnalyze ? "disabled" : ""}>${icon("search")}<span>${tierAnalyzing ? L("Analiz ediliyor…", "Analyzing…", lang) : L("Sıcak Veri & Parça Analizi", "Hot Data & Fragment Analysis", lang)}</span></button>`
          : `<button type="button" class="button primary" data-action="analyze-tiering" ${!canAnalyze ? "disabled" : ""}>${icon("search")}<span>${tierAnalyzing ? L("Analiz ediliyor…", "Analyzing…", lang) : L("Soğuk Veri & Parça Analizi", "Cold Data & Fragment Analysis", lang)}</span></button>
             <button type="button" class="button accent-outline" data-tier-job="ssd_extent_optimize" ${!diskVolOk || runningTier || tierTransferring ? "disabled" : ""}>${icon("bolt")}<span>${L("SSD Akıllı Parça Temizliği + ReTrim", "SSD Extent Consolidation + ReTrim", lang)}</span></button>`
      }
      ${
        companion
          ? `<span class="tier-companion-chip">${icon("folder")}<span>${L("Hedef:", "Target:", lang)} <b>${e(companion.letter)}:</b> (${e(companion.model)} · ${fmtBytes(companion.free_bytes)} ${L("boş", "free", lang)})</span></span>`
          : ""
      }
    </div>
    ${
      !diskVolOk && state?.source === "live"
        ? !state.admin
          ? `<p class="form-hint">${L("Klasör ve parça analizi yönetici yetkisi olmadan salt okunur çalışır; parça temizliği ve taşıma için sağ üstten “Yönetici olarak aç”a basın.", "Folder & extent analysis runs read-only without admin; click “Run as Admin” in the top bar for extent cleanup or relocation.", lang)}</p>`
          : isWriteLocked(state)
            ? `<p class="form-hint">${L("Salt-okunur koruma etkin. Parça temizliği ve taşıma için üst çubuktan yazma kilidini açın.", "Read-Only Guard is active. Unlock write mode in the top bar for extent cleanup or relocation.", lang)}</p>`
            : ""
        : ""
    }
    ${runningTier || (activeTierJob && activeTierJob.status !== "completed") ? jobProgress(activeTierJob) : ""}
    ${smartTierResultBanner(activeTierJob)}
    ${analysisHtml}
    ${movesHtml}
  </article>`;
}

function devWorkspaceCard(
  d: Disk,
  selectedLetter: string,
  sysLive: boolean,
  diskVolOk: boolean,
): string {
  const lang = getLang();
  const analysis =
    devAnalysis &&
    devAnalysis.disk_id === d.id &&
    (!selectedLetter || !devAnalysis.letter || devAnalysis.letter === selectedLetter)
      ? devAnalysis
      : null;
  const canScan =
    !pending &&
    !devScanning &&
    !devCleaning &&
    (state?.source === "replay" || !!selectedLetter);
  const busyNow = devScanning || devCleaning || tierTransferring || pending;
  const cleanOk = canWrite(state) && canStart(state, d) && !busyNow;
  const tuning = state?.windows?.system_tuning;
  const target = analysis?.offload_target ?? null;
  const visibleProjects = analysis
    ? devFilter === "stale"
      ? analysis.projects.filter((p) => p.stale)
      : analysis.projects
    : [];
  const staleCount = analysis ? analysis.projects.filter((p) => p.stale).length : 0;
  const envSnippet = devEnvSnippet(target?.letter || "D");

  const analysisHtml = analysis
    ? `<div class="dev-analysis-panel">
        ${
          analysis.replay
            ? `<div class="mode-note">${icon("info")}<span>${e(lang === "en" ? analysis.summary_en : analysis.summary_tr)}</span></div>`
            : `<div class="dev-summary-grid">
                 <div><span>${L("Yeniden Üretilebilir Yük", "Rebuildable Artifacts", lang)}</span><strong>${fmtBytes(analysis.total_artifact_bytes)}</strong><small>${L(`${fmtBytes(analysis.stale_artifact_bytes)} (14+ gün eski)`, `${fmtBytes(analysis.stale_artifact_bytes)} (14+ days old)`, lang)}</small></div>
                 <div><span>${L("Yerel LLM &amp; Paket Önbelleği", "Local LLM &amp; Pkg Caches", lang)}</span><strong>${fmtBytes(analysis.total_cache_bytes)}</strong><small>${fmt(analysis.global_caches.length)} ${L("konum", "locations", lang)}</small></div>
                 <div><span>${L("WSL2 / Docker VHDX", "WSL2 / Docker VHDX", lang)}</span><strong>${fmtBytes(analysis.total_vhdx_bytes)}</strong><small>${fmt(analysis.vhdx_files.length)} ${L("sanal disk", "virtual disks", lang)}</small></div>
               </div>
               <div class="dev-subsection">
                 <div class="tier-subhead">
                   <div class="inline-actions">
                     <h3>${L("Projeler &amp; Derleme Klasörleri", "Projects &amp; Build Artifacts", lang)}</h3>
                     <div class="cat-pills" role="group" aria-label="${L("Proje yaş filtresi", "Project age filter", lang)}">
                       <button type="button" class="button compact ${devFilter === "all" ? "selected" : "quiet"}" data-dev-filter="all">${L("Tümü", "All", lang)} (${analysis.projects.length})</button>
                       <button type="button" class="button compact ${devFilter === "stale" ? "selected" : "quiet"}" data-dev-filter="stale">${L("14+ Gün Eski", "14+ Days Old", lang)} (${staleCount})</button>
                     </div>
                   </div>
                   <span class="small-count">${visibleProjects.length}</span>
                 </div>
                 ${
                   visibleProjects.length
                     ? `<div class="dev-project-list">${visibleProjects
                         .map((p) => {
                           const sb = devStackBadge(p.stack);
                           return `<div class="dev-project-card">
                             <div class="dev-project-head">
                               <div class="dev-project-meta">
                                 <div class="inline-actions">
                                   <strong>${e(p.name)}</strong>
                                   ${pill(sb.label, sb.level)}
                                   ${pill(formatUntouchedDays(p.days_untouched, lang), p.stale ? "warning" : "")}
                                   ${p.has_env ? pill(L(".env korundu", ".env preserved", lang), "ok") : ""}
                                   ${p.recent_git_edit ? pill(L("Git düzenleme izi", "Uncommitted git", lang), "info") : ""}
                                 </div>
                                 <small class="mono tier-cand-path">${e(p.path)}</small>
                               </div>
                               <strong class="mono dev-project-bytes">${fmtBytes(p.artifact_bytes)}</strong>
                             </div>
                             ${
                               p.artifacts.length
                                 ? `<div class="dev-artifact-list">${p.artifacts
                                     .map(
                                       (art) => `<div class="dev-artifact-chip">
                                         <div class="dev-artifact-info">
                                           <strong class="mono">${e(art.name)}</strong>
                                           <small>${fmtBytes(art.size_bytes)} · ${fmt(art.file_count)} ${L("dosya", "files", lang)}</small>
                                         </div>
                                         <button type="button" class="button compact quiet" data-dev-clean-path="${e(art.path)}" data-dev-clean-token="${e(art.clean_token)}" data-dev-clean-name="${e(`${p.name}/${art.name}`)}" data-dev-clean-bytes="${art.size_bytes}" ${!cleanOk ? "disabled" : ""}>${icon("trash")}<span>${L("Güvenli Temizle", "Safe Clean", lang)}</span></button>
                                       </div>`,
                                     )
                                     .join("")}</div>`
                                 : `<p class="small-note muted">${L("Bu projede yeniden üretilebilir derleme klasörü yok.", "No rebuildable artifact folders in this project.", lang)}</p>`
                             }
                           </div>`;
                         })
                         .join("")}</div>`
                     : `<div class="mode-note">${icon("check")}<span>${L("Bu filtrede proje tespit edilmedi.", "No projects matched this filter.", lang)}</span></div>`
                 }
               </div>
               <div class="dev-subsection">
                 <div class="tier-subhead">
                   <h3>${L("Yerel LLM Modelleri &amp; Paket Önbellekleri", "Local LLM Models &amp; Package Caches", lang)}</h3>
                   <span class="small-count">${analysis.global_caches.length}</span>
                 </div>
                 ${
                   analysis.global_caches.length
                     ? `<div class="dev-cache-list">${analysis.global_caches
                         .map((c) => {
                           const kb = devCacheKindBadge(c.kind, lang);
                           const label = lang === "en" ? c.name_en : c.name;
                           const note = lang === "en" ? c.note_en : c.note_tr;
                           return `<div class="dev-cache-row">
                             <div class="tier-cand-main">
                               <div class="inline-actions">
                                 <strong>${e(label)}</strong>
                                 ${pill(kb.label, kb.level)}
                                 ${c.is_junction ? pill("NTFS Junction", "ok") : ""}
                               </div>
                               <small class="mono tier-cand-path">${e(c.path)} · ${fmtBytes(c.size_bytes)} (${fmt(c.file_count)} ${L("dosya", "files", lang)})</small>
                               <p class="tier-cand-reason">${e(note)}</p>
                             </div>
                             <div class="dev-cache-actions">
                               ${
                                 c.can_offload && target
                                   ? `<button type="button" class="button compact primary" data-dev-offload-id="${e(c.id)}" ${!diskVolOk || busyNow ? "disabled" : ""}>${icon("arrow")}<span>${L(`Junction ile Taşı (${target.letter}:)`, `Offload Junction (${target.letter}:)`, lang)}</span></button>`
                                   : ""
                               }
                               ${
                                 c.can_clean && c.clean_token
                                   ? `<button type="button" class="button compact quiet" data-dev-clean-path="${e(c.path)}" data-dev-clean-token="${e(c.clean_token)}" data-dev-clean-name="${e(label)}" data-dev-clean-bytes="${c.size_bytes}" ${!cleanOk ? "disabled" : ""}>${icon("trash")}<span>${L("Önbelleği Temizle", "Clean Cache", lang)}</span></button>`
                                   : ""
                               }
                             </div>
                           </div>`;
                         })
                         .join("")}</div>`
                     : `<div class="mode-note">${icon("info")}<span>${L("10 MiB üzerinde yerel LLM modeli veya paket önbelleği bulunamadı.", "No local LLM models or package caches above 10 MiB detected.", lang)}</span></div>`
                 }
               </div>
               <div class="dev-subsection">
                 <div class="tier-subhead">
                   <h3>${L("WSL2 &amp; Docker Sanal Diskleri (.vhdx)", "WSL2 &amp; Docker Virtual Disks (.vhdx)", lang)}</h3>
                   <span class="small-count">${analysis.vhdx_files.length}</span>
                 </div>
                 ${
                   analysis.vhdx_files.length
                     ? `<div class="dev-cache-list">${analysis.vhdx_files
                         .map(
                           (v) => `<div class="dev-vhdx-row">
                             <div class="tier-cand-main">
                               <div class="inline-actions">
                                 <strong>${e(v.name)}</strong>
                                 ${pill(v.kind === "docker_vhdx" ? "Docker Desktop" : "WSL2", "info")}
                               </div>
                               <small class="mono tier-cand-path">${e(v.path)} · ${fmtBytes(v.size_bytes)}</small>
                             </div>
                             <div class="dev-cache-actions">
                               ${optBtn("compact_wsl_vhdx", v.path, "compact", L("VHDX Sıkıştır (Compact)", "Compact VHDX", lang), !sysLive || busyNow, "primary")}
                             </div>
                           </div>`,
                         )
                         .join("")}</div>`
                     : `<div class="mode-note">${icon("info")}<span>${L("Bu bölümde WSL2 veya Docker ext4.vhdx dosyası bulunamadı.", "No WSL2 or Docker ext4.vhdx virtual disks found on this volume.", lang)}</span></div>`
                 }
               </div>`
        }
      </div>`
    : "";

  return `<article class="card dev-workspace-card" id="dev-workspace-card" aria-label="${L("Geliştirici ve Vibe-Coding Depolama Alanı", "Developer & Vibe-Coding Storage")}">
    <div class="tier-head">
      <div class="tier-head-main">
        <div class="eyebrow">${icon("code")} ${L("GELİŞTİRİCİ &amp; VIBE-CODING", "DEVELOPER &amp; VIBE-CODING", lang)}</div>
        <h2>${L("Proje Bloat Radarı, Model Önbelleği &amp; VHDX", "Project Bloat Radar, Model Caches &amp; VHDX", lang)}</h2>
      </div>
      ${pill(L("Kaynak kod & .env korumalı", "Source & .env protected", lang), "ok")}
    </div>
    <p class="tier-desc">${L("node_modules, Rust target, .venv gibi yeniden üretilebilir derleme yüklerini tarar; Ollama/Hugging Face modellerini NTFS Junction ile ikinci diske taşır ve WSL2/Docker .vhdx ölü alanını sıkıştırır.", "Scans rebuildable artifacts (node_modules, target, .venv), offloads Ollama/Hugging Face caches to another drive via NTFS Junctions, and compacts WSL2/Docker .vhdx images.", lang)}</p>
    <div class="tier-actions">
      <button type="button" class="button primary" data-action="analyze-dev" ${!canScan ? "disabled" : ""}>${icon("search")}<span>${devScanning ? L("Projeler Taranıyor…", "Scanning Projects…", lang) : L("Projeleri & Önbellekleri Tara", "Scan Projects & Caches", lang)}</span></button>
      <button type="button" class="button accent-outline" data-action="pick-dev-folder" ${state?.source === "replay" || busyNow ? "disabled" : ""}>${icon("folder")}<span>${L("Klasör Seç", "Pick Folder", lang)}</span></button>
      ${devCustomRoot ? `<span class="tier-companion-chip">${icon("folder")}<span class="mono">${e(devCustomRoot)}</span><button type="button" class="button compact quiet" data-action="clear-dev-root" aria-label="${L("Özel klasörü temizle", "Clear custom folder")}">×</button></span>` : ""}
      ${
        target
          ? `<span class="tier-companion-chip">${icon("disk")}<span>${L("Taşıma hedefi:", "Offload target:", lang)} <b>${e(target.letter)}:</b> (${e(target.model)} · ${fmtBytes(target.free_bytes)} ${L("boş", "free", lang)})</span></span>`
          : ""
      }
    </div>
    ${analysisHtml}
    <details class="dev-guide-details" data-details-key="dev-drive-guide">
      <summary><span><strong>${L("Windows 11 Dev Drive &amp; Paket Önbelleği Ortam Değişkenleri", "Windows 11 Dev Drive &amp; Cache Env Variables", lang)}</strong></span>${pill(tuning?.dev_drive_supported === true ? L("Dev Drive Hazır (ReFS)", "Dev Drive Ready (ReFS)", lang) : tuning?.dev_drive_supported === false ? L("Win 11 23H2+ Gerekir", "Needs Win 11 23H2+", lang) : L("Durum Ölçülmedi", "Status Unknown", lang), tuning?.dev_drive_supported === true ? "ok" : "")}${icon("chevron")}</summary>
      <div class="dev-guide-body">
        <p>${L("Yeni projelerdeki paket önbelleklerini ve yerel modelleri doğrudan ikincil sürücüye yönlendirmek için aşağıdaki PowerShell komutlarını kullanıcı ortamınızda çalıştırabilirsiniz:", "Run these PowerShell commands to point future package caches and local models straight to your secondary drive:", lang)}</p>
        <pre class="defrag-output">${e(envSnippet)}</pre>
        <div class="opt-actions">
          <button type="button" class="button compact quiet" data-copy-text="${e(envSnippet)}">${icon("copy")}<span>${L("PowerShell Komutlarını Kopyala", "Copy PowerShell Commands", lang)}</span></button>
        </div>
      </div>
    </details>
  </article>`;
}

function optimizeView(d: Disk): string {
  const lang = getLang();
  const sysLive = canOptimizeSystem(state) && !pending;
  const diskMaint = canMaintain(state, d) && !pending;
  const diskVolOk = diskMaint && canStart(state, d);
  const tuning = state?.windows?.system_tuning;
  const trimEnabled = tuning?.trim_enabled ?? state?.windows?.trim_enabled;
  const wc = d.optimization?.write_cache;
  const opPct = d.optimization?.over_provisioning_percent;
  const link = d.bus_link;
  const isHdd = d.type.includes("HDD");
  const selectedLetter = letter || d.volumes[0]?.letter || "";
  const activeDefrag = currentJob("defrag") ?? defragHistory[0] ?? null;

  const wcRec = optRecord(`write_cache:${d.id}`);
  const trimRec = optRecord("ntfs_trim:system");
  const aspmRec = optRecord("pcie_aspm:system");
  const hibRec = optRecord("hibernation:system");
  const longPathsRec = optRecord("long_paths:system");
  const ntfs8dot3Rec = optRecord("ntfs_8dot3:system");
  const apmRec = optRecord(`hdd_apm:${d.id}`);
  const opRec = optRecord(`op:${d.id}:${selectedLetter}`);

  const aspmText =
    tuning?.pcie_aspm === 0
      ? L("Kapalı (Tam performans)", "Off (Max Performance)", lang)
      : tuning?.pcie_aspm === 1
        ? L("Orta güç tasarrufu", "Moderate Power Savings", lang)
        : tuning?.pcie_aspm === 2
          ? L("En fazla güç tasarrufu", "Maximum Power Savings", lang)
          : L("Bilinmiyor", "Unknown", lang);
  const linkDisplay = link?.current ?? formatInterface(d.interface, d.type);

  return `${heading(d, L("1 · OPTİMİZASYON & POLİTİKA", "1 · STORAGE POLICIES", lang), L("Depolama politikaları ve katmanlama", "Storage policies &amp; tiering", lang), L("Güvenli donanım/NTFS politikaları disk türüne göre otomatik seçilir; her değişiklik SQLite kaydından geri alınabilir.", "Safe hardware and NTFS policies are selected per drive class; every change is logged in SQLite for rollback.", lang))}
    ${autopilotHero(d)}
    <div class="section-label">${L("Mevcut durum", "Current diagnostics", lang)}</div>
    <section class="opt-diag-grid" aria-label="${L("Darboğaz ve hat analizi", "Bus & alignment diagnostics", lang)}">
      <article class="card diag-card">
        <div class="card-title">${icon("cpu")}<h2>${L("Bağlantı hattı", "Bus link", lang)}</h2>${pill(link?.bottleneck ? L("Darboğaz", "Bottleneck", lang) : link?.current ? L("Normal", "Normal", lang) : L("Ölçülmedi", "Unknown", lang), link?.bottleneck ? "warning" : link?.current ? "ok" : "")}</div>
        <strong>${e(linkDisplay)}</strong>
        <p>${link?.max ? `${L("Tepe", "Max", lang)}: ${e(link.max)}${link.ceiling_mb_s ? ` · ~${fmt(link.ceiling_mb_s)} ${L("MB/sn tavan", "MB/s cap", lang)}` : ""}` : L("Aktif hat hızı okunamadı.", "Active link rate unavailable.", lang)}</p>
      </article>
      <article class="card diag-card">
        <div class="card-title">${icon("surface")}<h2>${L("4K bölüm hizalaması", "4K partition alignment", lang)}</h2>${pill(d.optimization?.aligned_4k === true ? L("Hizalı", "Aligned", lang) : d.optimization?.aligned_4k === false ? L("Hizasız", "Misaligned", lang) : "—", d.optimization?.aligned_4k === true ? "ok" : d.optimization?.aligned_4k === false ? "warning" : "")}</div>
        <strong>${d.optimization?.aligned_4k === true ? L("4096 B Uyumlu", "4096 B Aligned", lang) : d.optimization?.aligned_4k === false ? L(`${fmt(d.optimization.misaligned_partitions)} bölüm hizasız`, `${fmt(d.optimization.misaligned_partitions)} misaligned`, lang) : L("Eşleme yok", "No mapping", lang)}</strong>
        <p>${e(alignmentDescription(d.optimization?.aligned_4k, lang))}</p>
      </article>
      <article class="card diag-card">
        <div class="card-title">${icon("bolt")}<h2>${L("Yazma önbelleği", "Write cache", lang)}</h2>${pill(wc?.enabled === true ? L("Etkin", "Enabled", lang) : wc?.enabled === false ? L("Kapalı", "Disabled", lang) : L("Bilinmiyor", "Unknown", lang), wc?.enabled === true ? "ok" : wc?.enabled === false ? "warning" : "")}</div>
        <strong>${wc?.enabled === true ? L("Write-Back Açık", "Write-Back Enabled", lang) : wc?.enabled === false ? L("Önbellek Kapalı", "Cache Disabled", lang) : L("Okunamadı", "Unavailable", lang)}</strong>
        <p>${wc?.changeable ? L("Donanım önbellek değişikliğine izin veriyor.", "Hardware supports toggling write cache.", lang) : wc ? L("Denetleyici bu ayarı sabit yönetiyor.", "Controller enforces fixed caching policy.", lang) : L("Windows önbellek sorgusu mevcut değil.", "Windows cache query unavailable.", lang)}</p>
      </article>
      <article class="card diag-card">
        <div class="card-title">${icon("disk")}<h2>Over-Provisioning</h2>${pill(opPct != null && opPct >= 5 ? L("Ayrılmış", "Reserved", lang) : L("Standart", "Standard", lang), opPct != null && opPct >= 5 ? "ok" : "")}</div>
        <strong>${opPct != null ? (lang === "en" ? `${fmt(opPct, 1)}%` : `%${fmt(opPct, 1)}`) : "—"}</strong>
        <p>${d.optimization?.unallocated_bytes != null ? `${fmtBytes(d.optimization.unallocated_bytes)} ${L("ayrılmamış yedek alan", "unallocated reserve", lang)}` : L("Ayrılmamış alan bilgisi yok.", "Unallocated space info unavailable.", lang)}</p>
      </article>
    </section>

    ${smartTierCard(d, selectedLetter, sysLive, diskVolOk)}

    ${devWorkspaceCard(d, selectedLetter, sysLive, diskVolOk)}

    <details class="manual-controls" data-details-key="manual-controls">
    <summary>${icon("settings")}<span><strong>${L("Elle ayarla (ileri düzey)", "Manual tuning (advanced)", lang)}</strong><small>${L("Yukarıdaki profil bunları sırayla uygular; tekil politika kontrolü için açın.", "The policy profile applies these in sequence; expand for individual controls.", lang)}</small></span>${icon("chevron")}</summary>
    <section class="opt-grid" aria-label="${L("Hızlandırma işlemleri", "Optimization controls", lang)}">
      <article class="card opt-card">
        <div class="card-title">${icon("bolt")}<h2>${L("Donanım Yazma Önbelleği", "Hardware Write Cache", lang)}</h2>${pill(wc?.enabled === true ? L("Açık", "On", lang) : wc?.enabled === false ? L("Kapalı", "Off", lang) : "—", wc?.enabled === true ? "ok" : "")}</div>
        <p>${L("IOCTL üzerinden disk yazma önbelleğini açar veya kapatır. Kesintisiz güç kaynağı (UPS) olmayan sistemlerde dikkatli kullanın.", "Controls disk write caching via Win32 IOCTL. Avoid without battery/UPS on critical workstations.", lang)}</p>
        <div class="opt-actions">
          ${optBtn("write_cache", d.id, "enable", L("Etkinleştir", "Enable", lang), !diskMaint || wc?.enabled === true, "primary")}
          ${optBtn("write_cache", d.id, "disable", L("Kapat", "Disable", lang), !diskMaint || wc?.enabled === false)}
          ${wcRec ? optBtn("write_cache", d.id, "rollback", L("Önceki Değere Dön", "Revert", lang), !diskMaint) : ""}
        </div>
      </article>

      <article class="card opt-card">
        <div class="card-title">${icon("refresh")}<h2>Windows TRIM (DisableDeleteNotify)</h2>${pill(trimEnabled === true ? L("Etkin", "Enabled", lang) : trimEnabled === false ? L("Kapalı", "Disabled", lang) : L("Bilinmiyor", "Unknown", lang), trimEnabled === true ? "ok" : trimEnabled === false ? "warning" : "")}</div>
        <p>${L("Silinen blokların SSD denetleyicisine anında bildirilmesini sağlar. Kapalıysa rastgele yazma hızı zamanla düşer.", "Notifies SSD controllers immediately when files are deleted to keep write throughput steady.", lang)}</p>
        <div class="opt-actions">
          ${optBtn("ntfs_trim", "system", "enable", L("TRIM Etkinleştir", "Enable TRIM", lang), !sysLive || trimEnabled === true, "primary")}
          ${trimRec ? optBtn("ntfs_trim", "system", "rollback", L("Geri Al", "Revert", lang), !sysLive) : ""}
        </div>
      </article>

      <article class="card opt-card">
        <div class="card-title">${icon("cpu")}<h2>${L("PCIe ASPM Düşük Gecikme", "PCIe ASPM Low Latency", lang)}</h2>${pill(aspmText, tuning?.pcie_aspm === 0 ? "ok" : "")}</div>
        <p>${L("NVMe SSD'lerde uyku/uyanma gecikmesini azaltmak için aktif güç planında PCIe bağlantı güç tasarrufunu devre dışı bırakır.", "Disables PCIe Active State Power Management in the active power plan to lower NVMe wake latency.", lang)}</p>
        <div class="opt-actions">
          ${optBtn("pcie_aspm", "system", "off", L("Tam Performans (Kapalı)", "Max Performance (Off)", lang), !sysLive || tuning?.pcie_aspm === 0, "primary")}
          ${optBtn("pcie_aspm", "system", "moderate", L("Dengeli (Orta)", "Moderate Savings", lang), !sysLive || tuning?.pcie_aspm === 1)}
          ${aspmRec ? optBtn("pcie_aspm", "system", "rollback", L("Geri Al", "Revert", lang), !sysLive) : ""}
        </div>
      </article>

      <article class="card opt-card">
        <div class="card-title">${icon("shield")}<h2>${L("Hazırda Bekletme (hiberfil.sys)", "Hibernation (hiberfil.sys)", lang)}</h2>${pill(tuning?.hibernate_enabled ? L("Etkin", "Enabled", lang) : tuning?.hibernate_enabled === false ? L("Kapalı", "Disabled", lang) : "—", tuning?.hibernate_enabled === false ? "ok" : "")}</div>
        <p>${L(`Her uyku/kapanışta GB'larca RAM içeriğini diske yazar. Kapatmak SSD ömrünü ve boş alanı korur (${tuning?.hiberfil_bytes ? fmtBytes(tuning.hiberfil_bytes) : "dosya yok"}).`, `Writes gigabytes of RAM to disk on sleep/shutdown. Disabling saves SSD TBW and space (${tuning?.hiberfil_bytes ? fmtBytes(tuning.hiberfil_bytes) : "no file"}).`, lang)}</p>
        <div class="opt-actions">
          ${optBtn("hibernation", "system", "off", L("Kapat (SSD Tasarrufu)", "Disable (Save TBW)", lang), !sysLive || tuning?.hibernate_enabled === false, "primary")}
          ${optBtn("hibernation", "system", "on", L("Etkinleştir", "Enable", lang), !sysLive || tuning?.hibernate_enabled === true)}
          ${hibRec ? optBtn("hibernation", "system", "rollback", L("Geri Al", "Revert", lang), !sysLive) : ""}
        </div>
      </article>

      <article class="card opt-card">
        <div class="card-title">${icon("code")}<h2>${L("Windows Uzun Yol Desteği (>260 Karakter)", "Windows Long Paths (>260 chars)", lang)}</h2>${pill(tuning?.long_paths_enabled === true ? L("Etkin", "Enabled", lang) : tuning?.long_paths_enabled === false ? L("Kapalı (260 sınır)", "Disabled (260 cap)", lang) : L("Bilinmiyor", "Unknown", lang), tuning?.long_paths_enabled === true ? "ok" : tuning?.long_paths_enabled === false ? "warning" : "")}</div>
        <p>${L("Derin node_modules, Rust/Cargo ve monorepo derleme hatalarını önlemek için Win32 LongPathsEnabled anahtarını etkinleştirir.", "Enables Win32 LongPathsEnabled in the registry to avoid deep node_modules, Rust/Cargo, and monorepo path errors.", lang)}</p>
        <div class="opt-actions">
          ${optBtn("long_paths", "system", "enable", L("Uzun Yolları Aç", "Enable Long Paths", lang), !sysLive || tuning?.long_paths_enabled === true, "primary")}
          ${optBtn("long_paths", "system", "disable", L("Kapat", "Disable", lang), !sysLive || tuning?.long_paths_enabled === false)}
          ${longPathsRec ? optBtn("long_paths", "system", "rollback", L("Geri Al", "Revert", lang), !sysLive) : ""}
        </div>
      </article>

      <article class="card opt-card">
        <div class="card-title">${icon("bolt")}<h2>${L("NTFS 8.3 Kısa Ad Oluşturma", "NTFS 8.3 Short Names", lang)}</h2>${pill(ntfs8dot3Label(tuning?.ntfs_8dot3_creation, lang), tuning?.ntfs_8dot3_creation === 1 ? "ok" : "")}</div>
        <p>${L("Binlerce küçük dosya oluşturulan npm/pip/Cargo kurulumlarında gereksiz 8.3 kısa dosya adı yazımlarını kapatarak metadata I/O hızını artırır.", "Disables legacy 8.3 short-name generation to reduce metadata overhead during heavy npm/pip/Cargo installs.", lang)}</p>
        <div class="opt-actions">
          ${optBtn("ntfs_8dot3", "system", "disable", L("8.3 Üretimini Kapat", "Disable 8.3 Creation", lang), !sysLive || tuning?.ntfs_8dot3_creation === 1, "primary")}
          ${optBtn("ntfs_8dot3", "system", "default", L("Varsayılan (Birim Bazlı)", "Default (Per-Volume)", lang), !sysLive || tuning?.ntfs_8dot3_creation === 2)}
          ${ntfs8dot3Rec ? optBtn("ntfs_8dot3", "system", "rollback", L("Geri Al", "Revert", lang), !sysLive) : ""}
        </div>
      </article>

      <article class="card opt-card wide-opt">
        <div class="card-title">${icon("disk")}<h2>${L("SSD Over-Provisioning (Ek Yedek Alan)", "SSD Over-Provisioning (Spare Pool)", lang)}</h2>${pill(isHdd ? L("HDD'de Kapalı", "N/A on HDD", lang) : L(`%${fmt(opPct ?? 0, 1)} Ayrılmış`, `${fmt(opPct ?? 0, 1)}% Reserved`, lang), isHdd ? "warning" : "ok")}</div>
        ${
          isHdd
            ? `<p class="muted">${L("Over-Provisioning yalnız NAND flash bellekli SSD'lerde aşınma dengeleme ve sürdürülebilir yazma kararlılığı için sunulur.", "Over-Provisioning is only offered on NAND SSDs to widen controller spare blocks for wear leveling.", lang)}</p>`
            : `<p>${L("Bölüm sonundan güvenli miktarda alanı küçültüp TRIM ile temizleyerek SSD denetleyicisinin arka plan yedek havuzunu genişletir. Daraltma sonrası en az 10 GiB boş alan korunur.", "Safely shrinks the tail of the NTFS partition and trims unallocated blocks to expand the SSD spare pool (keeps >=10 GiB free).", lang)}</p>
               <div class="opt-inline">${volumeSelect(d)}</div>
               ${
                 opBounds && opBounds.disk_id === d.id && opBounds.letter === selectedLetter
                   ? `<div class="op-bounds"><span>${L("Mevcut", "Current")}: <b>${fmtBytes(opBounds.current_size)}</b></span><span>${L("En küçük sınır", "Min size")}: <b>${fmtBytes(opBounds.size_min)}</b></span><span>${L("En büyük sınır", "Max size")}: <b>${fmtBytes(opBounds.size_max)}</b></span></div>`
                   : ""
               }
               <div class="opt-actions">
                 ${optBtn("query_op_bounds", `${d.id}:${selectedLetter}`, "query", L("Sınırları Sorgula", "Query Bounds", lang), !diskVolOk)}
                 ${optBtn("apply_op", `${d.id}:${selectedLetter}`, "7", L("%7 OP Ayır", "Reserve 7% OP", lang), !diskVolOk, "primary")}
                 ${optBtn("apply_op", `${d.id}:${selectedLetter}`, "10", L("%10 OP Ayır", "Reserve 10% OP", lang), !diskVolOk)}
                 ${optBtn("reclaim_op", `${d.id}:${selectedLetter}`, "reclaim", L("Alanı Bölüme Geri Kat", "Reclaim Unallocated", lang), !diskVolOk)}
                 ${opRec ? `<span class="muted small-note">${L(`Önceki boyut kayıtlı (${fmtBytes(Number(opRec.previous_value))})`, `Previous size stored (${fmtBytes(Number(opRec.previous_value))})`, lang)}</span>` : ""}
               </div>`
        }
      </article>

      ${
        isHdd
          ? `<article class="card opt-card wide-opt">
               <div class="card-title">${icon("tools")}<h2>${L("Klasik HDD Birleştirme &amp; APM Kafa Park", "Classic HDD Defrag &amp; APM Head Parking", lang)}</h2>${pill(L("HDD Hazır", "HDD Ready", lang), "ok")}</div>
               <p>${L("Mekanik HDD'lerde parçalanma analizi, akıllı dosya birleştirme (defrag /O), boş alan birleştirme (defrag /X) ve gürültülü kafa park etmeyi önleyen APM güç profili sunar.", "Provides NTFS fragmentation analysis, optimization (/O), free-space consolidation (/X), and ATA APM head-parking tuning.", lang)}</p>
               <div class="opt-inline">${volumeSelect(d)}</div>
               <div class="opt-actions">
                 <button type="button" class="button compact" data-defrag-mode="analyze" ${!diskVolOk ? "disabled" : ""}>${L("Parçalanma Analizi (/A)", "Analyze (/A)", lang)}</button>
                 <button type="button" class="button compact primary" data-defrag-mode="defrag" ${!diskVolOk ? "disabled" : ""}>${L("Akıllı Birleştirme (/O)", "Optimize (/O)", lang)}</button>
                 <button type="button" class="button compact" data-defrag-mode="consolidate" ${!diskVolOk ? "disabled" : ""}>${L("Boş Alan Birleştirme (/X)", "Consolidate Free (/X)", lang)}</button>
               </div>
               <div class="opt-actions">
                 ${optBtn("hdd_apm", d.id, "254", L("APM 254 (Tam Hız)", "APM 254 (Max Perf)", lang), !diskMaint)}
                 ${optBtn("hdd_apm", d.id, "128", L("APM 128 (Dengeli)", "APM 128 (Balanced)", lang), !diskMaint)}
                 ${optBtn("hdd_apm", d.id, "off", L("APM Kapalı", "Disable APM", lang), !diskMaint)}
                 ${apmRec ? optBtn("hdd_apm", d.id, "rollback", L("APM Geri Al", "Revert APM", lang), !diskMaint) : ""}
               </div>
               ${
                 activeDefrag?.repair_summary?.output
                   ? `<pre class="defrag-output">${e(String(activeDefrag.repair_summary.output))}</pre>`
                   : ""
               }
             </article>`
          : ""
      }
    </section>

    <article class="card opt-history">
      <div class="section-title"><div><h2>${L("Uygulanan optimizasyonlar ve geri alma (Rollback)", "Applied optimizations &amp; rollback", lang)}</h2><p>${L("Değiştirilen ayarların önceki değerleri yerel SQLite veritabanında saklanır", "Previous setting values are archived locally in SQLite for safe rollback", lang)}</p></div><span class="small-count">${state?.optimizations?.length ?? 0}</span></div>
      ${
        state?.optimizations?.length
          ? state.optimizations
              .map(
                (rec) => `<div class="opt-history-row">
                  <div><strong>${e(rec.title)}</strong><small class="mono">${e(rec.previous_value)} → ${e(rec.applied_value)} · ${date(rec.updated_at)}</small></div>
                  <button type="button" class="button compact quiet" data-rollback-key="${e(rec.key)}" data-rollback-target="${e(rec.target)}" ${!sysLive ? "disabled" : ""}>${L("Önceki Değere Dön", "Revert", lang)}</button>
                </div>`,
              )
              .join("")
          : empty(L("Henüz değiştirilen ayar yok", "No modified settings yet", lang), L("Bu cihazda SSD Sentinel ile uygulanan bir sistem/disk değişikliği bulunmuyor.", "No system or drive tuning changes have been recorded yet.", lang), "bolt")
      }
    </article>
    </details>`;
}

function diagnosisCard(
  diag: SurfaceDiagnosis,
  d: Disk,
  rs: RepairSummary | undefined,
  fullScanJob: Job | null,
): string {
  const lang = getLang();
  const isHdd = d.type.includes("HDD");
  const tDiag = translateDiagnosis(diag, isHdd, lang);
  const v = diag.verdict;
  const pillCls =
    v === "healthy" ? "ok" : v === "failing" ? "critical" : "warning";
  const pillText =
    ({
      healthy: L("Temiz", "Healthy", lang),
      attention: L("Dikkat", "Attention", lang),
      repairable: L("Onarılabilir", "Repairable", lang),
      failing: L("Kritik", "Critical", lang),
    } as Record<string, string>)[v] ?? L("Durum", "Status", lang);
  const cardCls = `verdict-${["healthy", "attention", "repairable", "failing"].includes(v) ? v : "attention"}`;
  const medianMs =
    rs?.median_chunk_ms ?? fullScanJob?.repair_summary?.median_chunk_ms ?? null;
  const slowThr =
    rs?.slow_threshold_ms ?? fullScanJob?.repair_summary?.slow_threshold_ms ?? null;
  const recMode = diag.recommended_mode;
  const ctaLabel =
    recMode === "bad_zone_isolate"
      ? L("Önerilen: Ölü Bölgeleri Kapat", "Recommended: Isolate Dead Zones", lang)
      : isHdd
        ? L("Önerilen: Nokta Atışı Onar & Hızlandır", "Recommended: Pinpoint Repair & Refresh", lang)
        : L("Önerilen: Yavaş Bölgeleri Tazele", "Recommended: Refresh Slow Regions", lang);

  return `<div class="ai-diagnosis-card ${cardCls}">
    <div class="ai-diag-head">
      <div class="inline-actions"><span class="eyebrow">${L("YÜZEY TEŞHİS MOTORU", "SMART SURFACE DIAGNOSIS", lang)}</span>${pill(pillText, pillCls)}</div>
      <span class="diag-score">${diag.score}<span>/100</span></span>
    </div>
    <h3>${e(tDiag.title)}</h3>
    <p>${e(tDiag.advice)}</p>
    <div class="diag-counts">
      <div><span>${L("Okunamayan", "Unreadable", lang)}</span><strong>${fmt(diag.unreadable)}</strong></div>
      <div><span>${L("Zayıf sektör", "Weak sectors", lang)}</span><strong>${fmt(diag.weak)}</strong></div>
      <div><span>${L("Yavaş bölge", "Slow regions", lang)}</span><strong>${fmt(diag.slow)}</strong></div>
      <div><span>${L("Ölü bölge", "Dead zones", lang)}</span><strong>${fmt(diag.dead_zones)}</strong></div>
    </div>
    ${
      medianMs != null || tDiag.engine
        ? `<p class="diag-engine">${icon("cpu")}${medianMs != null ? L(`Uyarlanabilir eşik: medyan ${fmt(medianMs, 2)} ms${slowThr != null ? ` · yavaş sınırı ${fmt(slowThr, 1)} ms` : ""}`, `Adaptive threshold: median ${fmt(medianMs, 2)} ms${slowThr != null ? ` · slow threshold ${fmt(slowThr, 1)} ms` : ""}`, lang) : ""}${medianMs != null && tDiag.engine ? " · " : ""}${tDiag.engine ? e(tDiag.engine) : ""}</p>`
        : ""
    }
    ${
      recMode
        ? `<div class="ai-diag-cta"><button type="button" class="button primary" data-recommended-surface="${e(recMode)}" ${!canStart(state, d) || pending || !state?.admin || isWriteLocked(state) ? "disabled" : ""}>${icon("bolt")}<span>${e(ctaLabel)}</span></button></div>`
        : ""
    }
  </div>`;
}

function pinpointAffectedText(r: PinpointRepairRow): string {
  const lang = getLang();
  const parts: string[] = [];
  if (r.moved_files?.length) {
    const extra =
      (r.moved_count ?? r.moved_files.length) > r.moved_files.length
        ? ` (+${(r.moved_count ?? 0) - r.moved_files.length})`
        : "";
    parts.push(`${L("Taşındı", "Moved", lang)}: ${r.moved_files.join(", ")}${extra}`);
  }
  if (r.lost_files?.length) parts.push(`${L("Kayıp", "Lost", lang)}: ${r.lost_files.join(", ")}`);
  if (r.system_files?.length) parts.push(`${L("Sistem", "System", lang)}: ${r.system_files.join(", ")}`);
  if (r.blocked?.length) parts.push(`${L("Kilitli", "Locked", lang)}: ${r.blocked.join(", ")}`);
  if (r.dead_errors?.length) parts.push(r.dead_errors[0]);
  return parts.length ? parts.join(" · ") : L("Boş alan", "Unallocated space", lang);
}

function pinpointSummaryCard(
  rs: RepairSummary,
  d: Disk,
  isHdd: boolean,
  selectedVol: string,
  volReady: boolean,
  qRec: OptimizationRecord | undefined,
): string {
  const lang = getLang();
  const zbr = rs.zbr_profile;
  const spd = rs.speed;
  const hasSpeed =
    !!spd && spd.measured > 0 && spd.before_avg_ms != null && spd.after_avg_ms != null;
  const gain =
    hasSpeed && spd!.before_avg_ms! > 0 && spd!.after_avg_ms! < spd!.before_avg_ms!
      ? Math.round(((spd!.before_avg_ms! - spd!.after_avg_ms!) / spd!.before_avg_ms!) * 100)
      : 0;
  const repairs = rs.repairs ?? [];
  const rows = repairs.slice(0, 50);
  const badge = rs.recovered_pending
    ? pill(L(`${rs.recovered_pending} bekleyen sektör onarıldı`, `${rs.recovered_pending} pending sectors healed`, lang), "ok")
    : rs.isolated_clusters
      ? pill(L(`${rs.isolated_clusters} küme kapatıldı`, `${rs.isolated_clusters} clusters isolated`, lang), "warning")
      : rs.healed
        ? pill(L(`${rs.healed} bölge onarıldı`, `${rs.healed} zones repaired`, lang), "ok")
        : rs.relocated_files
          ? pill(L(`${rs.relocated_files} dosya taşındı`, `${rs.relocated_files} files moved`, lang), "ok")
          : "";

  return `<div class="heal-summary pinpoint-summary">
    <div class="section-title"><div><h3>${L("Nokta atışı onarım sonucu", "Pinpoint repair summary", lang)}</h3><p>${e(rs.scope ?? L("Tarama haritası üzerinden nokta atışı onarım özeti", "Pinpoint repair summary from scan target map", lang))}</p></div>${badge}</div>
    ${
      hasSpeed
        ? `<div class="pinpoint-speed">
             <div><span>${L(`Ortalama okuma gecikmesi (${spd!.measured} bölge)`, `Avg read latency (${spd!.measured} zones)`, lang)}</span><strong>${L("Önce", "Before", lang)}: ${fmt(spd!.before_avg_ms, 1)} ms → ${L("Sonra", "After", lang)}: ${fmt(spd!.after_avg_ms, 1)} ms</strong></div>
             ${gain > 0 ? pill(L(`%${gain} daha hızlı`, `${gain}% faster`, lang), "ok") : pill(L("Ölçüm doğrulandı", "Verified", lang))}
           </div>`
        : ""
    }
    <div class="heal-stat-grid">
      <div><span>${L("Onarılan bölge", "Zones repaired", lang)}</span><strong>${fmt(rs.healed ?? 0)}</strong></div>
      <div><span>${L("Taşınan dosya", "Files relocated", lang)}</span><strong>${fmt(rs.relocated_files ?? 0)} · ${fmtBytes(rs.relocated_bytes ?? 0)}</strong></div>
      <div><span>${L("Kapatılan alan", "Isolated space", lang)}</span><strong>${rs.isolated_bytes ? fmtBytes(rs.isolated_bytes) : "0 B"}</strong></div>
      ${
        rs.smart_before && rs.smart_after
          ? `<div><span>${L("Bekleyen (#197)", "Pending (#197)", lang)}</span><strong>${rs.smart_before.pending_197} → ${rs.smart_after.pending_197}</strong></div>
             <div><span>${L("Yeniden eşlenen (#05)", "Reallocated (#05)", lang)}</span><strong>${rs.smart_before.reallocated_5} → ${rs.smart_after.reallocated_5}</strong></div>`
          : ""
      }
      <div><span>${L("Termal fren", "Thermal pauses", lang)}</span><strong>${fmt(rs.thermal_pauses ?? 0)}</strong></div>
    </div>
    ${
      rs.advice?.length
        ? `<ul class="pinpoint-advice">${rs.advice.map((a) => `<li>${e(a)}</li>`).join("")}</ul>`
        : ""
    }
    ${
      rows.length
        ? `<div class="table-wrap"><table class="pinpoint-table"><thead><tr><th>${L("LBA / Boyut", "LBA / Size", lang)}</th><th>${L("Tür", "Type", lang)}</th><th>${L("Sonuç", "Result", lang)}</th><th>${L("Hız (Önce → Sonra)", "Speed (Before → After)", lang)}</th><th>${L("Etkilenen Dosya", "Affected Files", lang)}</th></tr></thead><tbody>${rows
            .map(
              (r) => `<tr>
                <td class="mono">LBA ${fmt(r.lba)}<small>${fmtBytes(r.bytes)}</small></td>
                <td>${e(translatePinpointLabel(r.kind_label, lang))}${r.dead_zone ? ` ${pill(L("Ölü Bölge", "Dead Zone", lang), "warning")}` : ""}</td>
                <td>${pill(translatePinpointLabel(r.result, lang), r.status === "ok" ? "ok" : r.status === "bad" ? "critical" : "warning")}</td>
                <td class="mono">${r.before_ms != null ? `${fmt(r.before_ms, 1)} ms` : L("Okunamadı", "Unreadable", lang)} → ${r.after_ms != null ? `${fmt(r.after_ms, 1)} ms` : "—"}</td>
                <td>${e(pinpointAffectedText(r))}</td>
              </tr>`,
            )
            .join("")}</tbody></table></div>${repairs.length > 50 ? `<p class="form-hint">${L(`İlk 50 / ${repairs.length} hedef gösteriliyor.`, `Showing first 50 of ${repairs.length} targets.`, lang)}</p>` : ""}`
        : ""
    }
    ${
      isHdd && selectedVol
        ? `<div class="opt-actions">
             ${optBtn("guard_tail", `${d.id}:${selectedVol}`, String(zbr?.suggested_tail_guard_pct ?? 5), L(`Ölü Kuyruğu Bölüm Dışına Kırp (%${zbr?.suggested_tail_guard_pct ?? 5})`, `Trim Degraded Tail (${zbr?.suggested_tail_guard_pct ?? 5}%)`, lang), !volReady, zbr?.tail_degraded ? "primary" : "quiet")}
             ${optBtn("guard_tail", `${d.id}:${selectedVol}`, "10", L("Kuyruğu %10 Kırp (Unallocated Guard)", "Guard Tail 10% (Unallocated)", lang), !volReady)}
             ${qRec || (rs.isolated_clusters ?? 0) > 0 ? optBtn("release_quarantine", `${d.id}:${selectedVol}`, "release", L("Karantina Kalkanını Kaldır", "Release Quarantine", lang), !volReady) : ""}
           </div>`
        : ""
    }
  </div>`;
}

function surfaceView(d: Disk): string {
  const lang = getLang();
  const job = currentJob("surface") ?? surfaceHistory[0] ?? null;
  const blocks = job?.blocks ?? [];
  const block = blocks.find((b) => b.index === blockIndex) ?? blocks[0];
  const isHdd = d.type.includes("HDD");
  const isCompletedFull = (j: Job | null | undefined) =>
    j?.options?.mode === "physical_scan" &&
    j.status === "completed" &&
    !!j.repair_summary?.full_surface;
  const fullScanJob =
    (isCompletedFull(job) ? job : null) ??
    (isCompletedFull(state?.job) && state?.job?.disk_id === d.id
      ? state!.job
      : null) ??
    surfaceHistory.find((j) => isCompletedFull(j)) ??
    null;
  const scanTargets: SurfaceTarget[] = fullScanJob?.repair_summary?.targets ?? [];
  const activeDiagnosis: SurfaceDiagnosis | null =
    job?.repair_summary?.diagnosis ?? fullScanJob?.repair_summary?.diagnosis ?? null;
  const runningSurfaceMode =
    active(state?.job) &&
    state?.job?.kind === "surface" &&
    state.job.disk_id === d.id
      ? String(state.job.options?.mode ?? "")
      : null;
  const hasPinpoint =
    job?.repair_summary?.engine === "pinpoint" ||
    surfaceHistory.some(
      (j) => j.status === "completed" && j.repair_summary?.engine === "pinpoint",
    );
  const stepper = computeSurfaceStepper({
    hasFullScan: !!fullScanJob,
    hasDiagnosis: !!activeDiagnosis,
    hasPinpoint,
    runningMode: runningSurfaceMode,
    hasTargets: fullScanJob ? scanTargets.length > 0 : undefined,
  });

  if (!scan.mode || (!isHdd && scan.mode === "bad_zone_isolate")) {
    scan.mode = state?.admin ? "physical_scan" : "read_scan";
  }
  const selectedVol = letter || d.volumes[0]?.letter || "";
  const needsAdmin = ["physical_scan", "magnetic_refresh", "bad_zone_isolate", "targeted_repair"].includes(scan.mode);
  const needsScanMap = ["magnetic_refresh", "bad_zone_isolate"].includes(scan.mode);
  const mutatingSurface = ["data_refresh", "targeted_repair", "magnetic_refresh", "bad_zone_isolate"].includes(scan.mode);
  const hasTargets = !!fullScanJob && scanTargets.length > 0;
  const startDisabled =
    !canStart(state, d) ||
    pending ||
    (needsAdmin && !state?.admin) ||
    (needsScanMap && !hasTargets) ||
    (mutatingSurface && isWriteLocked(state));
  const rs = job?.repair_summary;
  const zbr = rs?.zbr_profile;
  const hasThroughput = blocks.some((b) => b.mb_s != null);
  const qRec = selectedVol ? optRecord(`quarantine:${d.id}:${selectedVol}`) : undefined;
  const volReady = canMaintain(state, d) && canStart(state, d) && !pending && !!selectedVol;
  const autoMib = autoSurfaceMib(scan.mode, d);
  const mibText = autoMib >= 1024 ? `${autoMib / 1024} GiB` : `${autoMib} MiB`;
  const targetBudgetText = !fullScanJob
    ? L("Önce yukarıdan “Tam yüzey taraması” yapın — nokta atışı hedef haritası o taramadan çıkar.", "Run a “Full surface scan” first to build the target map.", lang)
    : scanTargets.length === 0
      ? L(`Son tam tarama (${date(fullScanJob.finished_at)}) yüzeyde sorun bulmadı; onarım gerekmez.`, `Last full scan (${date(fullScanJob.finished_at)}) found zero surface defects; no repair needed.`, lang)
      : scan.mode === "bad_zone_isolate"
        ? L(`${scanTargets.length} hedef bölge hazır (${date(fullScanJob.finished_at)}). Windows chkdsk /scan sonrası hasarlı alandaki dosyalar dış izlere taşınır ve bozuk kümeler kalıcı kilitlenir.`, `${scanTargets.length} target zones ready (${date(fullScanJob.finished_at)}). Relocates affected files to outer tracks and permanently isolates damaged clusters.`, lang)
        : isHdd
          ? L(`${scanTargets.length} hedef bölge hazır (${date(fullScanJob.finished_at)}). Önce dosyalar güvenli boş alana taşınır, sonra zayıf/bozuk sektörler yerinde yeniden yazılıp doğrulanır; düzelmeyen yoğun hasarlar otomatik kapatılır.`, `${scanTargets.length} target zones ready (${date(fullScanJob.finished_at)}). Relocates user files safely, refreshes weak sectors, and isolates hopeless zones.`, lang)
          : L(`${scanTargets.length} yavaş bölge hazır (${date(fullScanJob.finished_at)}). Etkilenen dosyalar taze NAND hücrelerine taşınır; eski hücrelerin yerine yazma yapılmaz.`, `${scanTargets.length} slow regions ready (${date(fullScanJob.finished_at)}). Moves affected files to fresh NAND cells via FSCTL_MOVE_FILE.`, lang);
  const autoText =
    ({
      physical_scan: fullScanEstimate(d),
      read_scan: L(`Dosyalarınızdan ${mibText} örnek okunur. Yaklaşık 1-2 dakika sürer.`, `Samples ${mibText} from existing files (~1-2 min).`, lang),
      magnetic_refresh: targetBudgetText,
      bad_zone_isolate: targetBudgetText,
      targeted_repair: L(`Windows dosya sistemi kontrolü + ${mibText} dosya örneği okunur.`, `Windows chkdsk /scan + ${mibText} file sample read.`, lang),
      data_refresh: L(`${mibText} geçici dosya yazılıp geri okunur ve karşılaştırılır.`, `Writes and verifies ${mibText} temporary test data.`, lang),
    } as Record<string, string>)[scan.mode] ?? "";
  const startLabel =
    ({
      physical_scan: L("Taramayı başlat", "Start full scan", lang),
      read_scan: L("Hızlı kontrolü başlat", "Start quick check", lang),
      magnetic_refresh: isHdd ? L("Nokta atışı onarımı başlat", "Start pinpoint repair", lang) : L("Yavaş bölgeleri tazele", "Refresh slow regions", lang),
      bad_zone_isolate: L("Ölü bölgeleri kapat", "Isolate dead zones", lang),
      targeted_repair: L("Kontrolü başlat", "Start check", lang),
      data_refresh: L("Doğrulamayı başlat", "Start verification", lang),
    } as Record<string, string>)[scan.mode] ?? L("Başlat", "Start", lang);
  const scanMapHint = !needsScanMap
    ? ""
    : !fullScanJob
      ? L("Önce “Tam yüzey taraması” yapın; nokta atışı onarım o haritayı kullanır.", "Run a “Full surface scan” first; pinpoint repair targets that scan map.", lang)
      : scanTargets.length === 0
        ? L("Son tam tarama temiz; onarılacak hedef yok.", "Last full scan is clean; no defect targets found.", lang)
        : "";
  const modeCard = (
    value: string,
    ico: string,
    title: string,
    text: string,
    admin = false,
    tag = "",
  ) =>
    `<label class="mode-card ${scan.mode === value ? "selected" : ""}"><input type="radio" name="scan-mode" value="${value}" ${scan.mode === value ? "checked" : ""}><span>${icon(ico)}<strong>${title}${tag ? ` <em class="mode-tag">${tag}</em>` : ""}</strong><small>${text}${admin && !state?.admin ? L(" · Yönetici gerekir", " · Admin required", lang) : ""}</small></span></label>`;

  const cp =
    surfaceCheckpoint &&
    (!surfaceCheckpoint.disk_id || surfaceCheckpoint.disk_id === d.id) &&
    !runningSurfaceMode
      ? surfaceCheckpoint
      : null;
  const cpInfo = cp ? checkpointProgress(cp) : null;
  const resumeDisabled = !canStart(state, d) || !state?.admin || pending;
  const resumeBanner =
    cp && cpInfo
      ? `<div class="resume-banner" role="region" aria-label="${L("Yarım kalan yüzey taraması", "Paused surface scan", lang)}">
          <div class="resume-banner-main">
            ${icon("history")}
            <div>
              <strong>${L(`Yarım kalan tam yüzey taraması kaydedildi (%${cpInfo.percent})`, `Paused full surface scan checkpoint (${cpInfo.percent}%)`, lang)}</strong>
              <p>${cpInfo.completed} / ${cpInfo.total} ${L("bölge tamamlandı", "zones scanned", lang)}${cp.done_bytes != null && cp.disk_bytes ? ` · ${fmtBytes(cp.done_bytes)} / ${fmtBytes(cp.disk_bytes)}` : ""}${cp.targets_count ? ` · ${cp.targets_count} ${L("hedef bulundu", "targets found", lang)}` : ""}${cp.updated_at ? ` · ${date(cp.updated_at)}` : ""}</p>
            </div>
          </div>
          <div class="resume-banner-actions">
            ${button("resume-surface", L("Kaldığın Yerden Devam Et", "Resume Scan", lang), "play", "compact primary", resumeDisabled)}
            ${button("clear-surface-checkpoint", L("Sıfırla", "Reset", lang), "reset", "compact quiet", pending)}
          </div>
        </div>`
      : "";

  const stepItems: [string, string, string, "done" | "active" | "pending"][] = [
    ["1", L("Tam Tarama", "Full Scan", lang), L("Diski baştan sona oku", "Read entire surface", lang), stepper.states[0]],
    ["2", L("Akıllı Teşhis", "Smart Diagnosis", lang), L("Skor ve hedef haritası", "Health score & target map", lang), stepper.states[1]],
    [
      "3",
      isHdd ? L("Nokta Atışı Onar", "Pinpoint Repair", lang) : L("Yavaş Bölgeleri Tazele", "Refresh Slow Regions", lang),
      isHdd ? L("Zayıf/ölü bölgeleri çöz", "Repair weak & isolate dead zones", lang) : L("Eski veriyi taze hücreye taşı", "Move cold files to fresh NAND", lang),
      stepper.states[2],
    ],
  ];

  return `${heading(d, L("2 · YÜZEY TARAMA & ONARIM", "2 · SURFACE SCAN & REPAIR", lang), L("Diskte bozuk bölge var mı?", "Any bad sectors or slow blocks?", lang), isHdd ? L("Önce tam tarama yapın. Tarama haritasına göre nokta atışı onarın veya ölü bölgeleri kapatın.", "Run a full read-only scan first, then repair weak sectors or isolate dead zones from the map.", lang) : L("Önce tam tarama yapın. Yavaşlayan eski veri bölgeleri çıkarsa tek tıkla taze hücrelere taşıyın.", "Run a full scan first; if slow stale blocks appear, refresh them onto fresh NAND cells.", lang))}
  <ol class="surface-stepper" aria-label="${L("Yüzey onarım adımları", "Surface repair workflow", lang)}">
    ${stepItems
      .map(
        ([num, title, desc, st]) =>
          `<li class="surface-step step-${st}"><span class="step-badge">${st === "done" ? icon("check") : num}</span><div><strong>${num}. ${e(title)}</strong><small>${e(desc)}</small></div></li>`,
      )
      .join("")}
  </ol>
  ${resumeBanner}
  <div class="surface-layout"><article class="card scan-settings"><div class="section-title"><div><h2>${L("Ne yapmak istiyorsunuz?", "Select surface operation", lang)}</h2><p>${L("Birini seçin, gerisini uygulama ayarlar", "Pick a mode — safe budgets are automatic", lang)}</p></div></div>${volumeSelect(d)}
    ${modeCard("physical_scan", "cpu", L("Tam yüzey taraması", "Full surface scan", lang), L("Diskin tamamını okur, bozuk ve yavaş sektörleri bulur. Uzun sürer.", "Reads every LBA sequentially to detect bad or slow blocks. Read-only.", lang), true, !fullScanJob ? L("Önerilen", "Recommended", lang) : "")}
    ${modeCard("read_scan", "search", L("Hızlı kontrol", "Quick read check", lang), L("Kısa sürede genel durumu gösterir.", "Samples existing files quickly.", lang))}
    <div class="mode-group-title">${isHdd ? L("NOKTA ATIŞI ONARIM (TARAMA HARİTASINDAN)", "PINPOINT REPAIR (FROM SCAN MAP)", lang) : L("NOKTA ATIŞI HIZLANDIRMA (TARAMA HARİTASINDAN)", "PINPOINT BOOST (FROM SCAN MAP)", lang)}</div>
    ${
      isHdd
        ? `${modeCard("magnetic_refresh", "bolt", L("Nokta atışı onar & hızlandır", "Pinpoint repair & refresh", lang), L("Tam taramanın bulduğu bozuk/zayıf sektörleri kurtarıp yeniden yazar; yoğun hasarlı alanları otomatik kapatır.", "Evacuates files from weak/bad sectors, refreshes magnetic tracks safely, and isolates severe defects.", lang), true, activeDiagnosis?.recommended_mode === "magnetic_refresh" ? L("Önerilen", "Recommended", lang) : "")}
    ${modeCard("bad_zone_isolate", "shield", L("Ölü bölgeleri kapat", "Isolate dead zones", lang), L("Hasarlı bölgedeki dosyaları sağlam dış izlere taşır, bozuk alanı kalıcı kilitler.", "Moves files out of damaged zones to healthy tracks and locks bad clusters.", lang), true, activeDiagnosis?.recommended_mode === "bad_zone_isolate" ? L("Önerilen", "Recommended", lang) : "")}`
        : `${modeCard("magnetic_refresh", "bolt", L("Yavaş bölgeleri tazele", "Refresh slow regions", lang), L("Uzun süre yazılmadığı için yavaş okunan verileri taze NAND hücrelerine taşır; SSD okuma hızını geri getirir.", "Moves slow-reading cold files to fresh NAND cells via FSCTL_MOVE_FILE to restore read speed.", lang), true, activeDiagnosis?.recommended_mode === "magnetic_refresh" ? L("Önerilen", "Recommended", lang) : "")}
    <p class="mode-note">${icon("info")}<span>${L("SSD bozuk hücreleri kendisi değiştirir; ölü bölge kapatma gerekmez, yalnızca yavaş okunan veriler tazelenir.", "SSD controllers handle spare block retirement internally; only slow-reading stale files need refreshing.", lang)}</span></p>`
    }
    <details class="mode-more" data-details-key="surface-mode-more" ${["targeted_repair", "data_refresh"].includes(scan.mode) ? "open" : ""}><summary>${L("Diğer kontroller", "More checks", lang)} ${icon("chevron")}</summary>
    ${modeCard("targeted_repair", "tools", L("Dosya sistemi kontrolü", "Filesystem check", lang), L("Windows chkdsk ile NTFS hatalarını arar.", "Checks NTFS consistency via Windows chkdsk.", lang), true)}
    ${modeCard("data_refresh", "refresh", L("Yazma doğrulaması", "Write verify check", lang), L("Geçici dosya yazıp geri okur.", "Writes and verifies a temporary test payload.", lang))}
    </details>
    <div class="auto-budget">${icon("settings")}<div><strong>${L("Otomatik ayarlandı", "Auto-configured budget", lang)}</strong><small>${e(autoText)}</small></div></div>
    ${button("start-surface", startLabel, "play", "primary full", startDisabled)}<p class="form-hint">${e((mutatingSurface ? writeDisabledReason(d) : disabledReason(d)) || (needsAdmin && !state?.admin ? L("Bu seçenek için sağ üstten “Yönetici olarak aç”a basın.", "Click “Run as Admin” in the top bar for raw device access.", lang) : scanMapHint || L("Disk ısınırsa veya yeni hata çıkarsa işlem kendini otomatik yavaşlatır ya da durdurur.", "Built-in thermal & SMART guard pauses or halts automatically if temperature or errors spike.", lang)))}</p></article>
  <div class="surface-right-col">${
    activeDiagnosis && rs?.engine !== "pinpoint"
      ? diagnosisCard(activeDiagnosis, d, rs, fullScanJob)
      : ""
  }<article class="card block-map-card"><div class="section-title"><div><h2>${L("200 Bölgeli Yüzey &amp; LBA Haritası", "200-Zone Surface &amp; LBA Map", lang)}</h2><p>${blocks.length ? `${blocks.length} ${L("bölge ölçüldü", "zones measured", lang)}` : L("200 bölge · sonuç bekleniyor", "200 zones · awaiting scan", lang)}</p></div>${job ? button("export-surface", L("Rapor", "Export", lang), "export", "compact") : pill(L("NOKTA ATIŞI", "PINPOINT ENGINE", lang), "ok")}</div><div class="block-grid" role="group" aria-label="${L("Bölge ve dosya gecikme haritası", "Surface latency zone map", lang)}">${Array.from(
    { length: 200 },
    (_, i) => {
      const b = blocks.find((b) => b.index === i);
      return `<button type="button" data-block="${i}" aria-label="${L("Örnek", "Zone", lang)} ${i + 1}: ${b ? blockStatus(b) : L("Ölçülmedi", "Not scanned", lang)}" class="block ${b?.status ?? "pending"} ${block?.index === i ? "selected" : ""}" title="${b ? e(blockStatus(b)) : L("Ölçülmedi", "Not scanned", lang)}" ${!b ? "disabled" : ""}>${b?.status === "bad" ? "×" : ""}</button>`;
    },
  ).join("")}</div><div class="block-legend">${[
    ["fast", "<5 ms"],
    ["normal", "5–25 ms"],
    ["slow", "25–100 ms"],
    ["warning", ">100 ms"],
    ["bad", L("Okuma hatası", "Read error", lang)],
    ["pending", L("Ölçülmedi", "Pending", lang)],
  ]
    .map(([cls, label]) => `<span><i class="${cls}"></i>${label}</span>`)
    .join(
      "",
    )}</div>${jobProgress(job)}<div class="block-inspector">${block ? `<div><span class="eyebrow">${L("BÖLGE", "ZONE", lang)} #${block.index + 1}</span><h3>${blockStatus(block)}</h3><p class="mono">${e(block.file)}</p></div><dl><dt>${L("Gecikme", "Latency", lang)}</dt><dd>${fmt(block.latency_ms, 3)} ms</dd><dt>${L("Aktarım", "Speed", lang)}</dt><dd>${fmt(block.mb_s, 2)} ${L("MB/sn", "MB/s", lang)}</dd><dt>${L("Ofset", "Offset", lang)}</dt><dd>${fmtBytes(block.offset_bytes)}</dd><dt>${L("Boyut", "Size", lang)}</dt><dd>${fmtBytes(block.bytes)}</dd></dl>${block.error ? `<p class="error-text">${e(block.error)}</p>` : ""}` : `${icon("surface")}<div><h3>${L("Her bölgeyi ayrı inceleyin", "Inspect any zone", lang)}</h3><p>${L("Bir bölge veya LBA ölçüldüğünde hedefini, fiziksel ofsetini ve gecikmesini burada görebilirsiniz.", "Click any measured square to inspect its file target, byte offset, and latency.", lang)}</p></div>`}</div>${
    hasThroughput
      ? `<div class="zbr-card"><div class="section-title"><div><h3>${L("ZBR Platter Hız Eğrisi (Dış İz → İç İz)", "Surface Speed Profile (Outer → Inner Track)", lang)}</h3><p>${L("Okuma kararlılığı ve iç iz düşüş analizi", "Sequential read consistency & inner-track dropoff", lang)}</p></div>${zbr?.outer_mb_s != null ? `<div class="inline-actions">${pill(`${L("Dış", "Outer", lang)}: ${fmt(zbr.outer_mb_s, 1)} ${L("MB/sn", "MB/s", lang)}`, "ok")}${pill(`${L("İç", "Inner", lang)}: ${fmt(zbr.inner_mb_s, 1)} ${L("MB/sn", "MB/s", lang)}`)}${zbr.drop_percent != null ? pill(L(`Düşüş: %${fmt(zbr.drop_percent, 1)}`, `Drop: ${fmt(zbr.drop_percent, 1)}%`, lang), zbr.tail_degraded ? "warning" : "") : ""}</div>` : ""}</div>${chart(
          Array.from({ length: 200 }, (_, i) => blocks.find((b) => b.index === i)?.mb_s ?? null),
          "var(--accent)",
          L("Yüzey aktarım hız eğrisi", "Surface transfer speed profile", lang),
        )}</div>`
      : ""
  }${
    rs?.engine === "pinpoint"
      ? pinpointSummaryCard(rs, d, isHdd, selectedVol, volReady, qRec)
      : rs && (rs.smart_before || rs.zbr_profile || rs.isolated_clusters != null || rs.evacuated_clusters != null || rs.bad_lbas?.length || (isHdd && qRec))
        ? `<div class="heal-summary">
             <div class="section-title"><div><h3>${L("Tarama ve onarım sonucu", "Scan &amp; repair result", lang)}</h3><p>${e(rs.scope ?? L("Fiziksel tarama ve onarım özeti", "Physical scan and repair summary", lang))}</p></div>${rs.recovered_pending ? pill(L(`${rs.recovered_pending} bekleyen sektör onarıldı`, `${rs.recovered_pending} pending sectors healed`, lang), "ok") : rs.isolated_clusters ? pill(L(`${rs.isolated_clusters} küme kilitlendi`, `${rs.isolated_clusters} clusters locked`, lang), "warning") : ""}</div>
             ${
               rs.smart_before && rs.smart_after
                 ? `<div class="heal-stat-grid">
                      <div><span>${L("Bekleyen (#197)", "Pending (#197)", lang)}</span><strong>${rs.smart_before.pending_197} → ${rs.smart_after.pending_197}</strong></div>
                      <div><span>${L("Yeniden Eşlenen (#05)", "Reallocated (#05)", lang)}</span><strong>${rs.smart_before.reallocated_5} → ${rs.smart_after.reallocated_5}</strong></div>
                      <div><span>${L("Düzeltilemeyen (#198)", "Uncorrectable (#198)", lang)}</span><strong>${rs.smart_before.uncorrectable_198} → ${rs.smart_after.uncorrectable_198}</strong></div>
                      <div><span>${L("Tazelenen / Tahliye", "Refreshed / Evacuated", lang)}</span><strong>${fmt(rs.refreshed_blocks ?? rs.evacuated_clusters ?? rs.micro_probed_sectors ?? 0)}</strong></div>
                      ${rs.isolated_bytes ? `<div><span>${L("Karantina Kalkanı", "Quarantine Shield", lang)}</span><strong>${fmtBytes(rs.isolated_bytes)}</strong></div>` : ""}
                      <div><span>${L("Termal Fren", "Thermal pauses", lang)}</span><strong>${fmt(rs.thermal_pauses ?? 0)}</strong></div>
                    </div>`
                 : ""
             }
             ${
               rs.bad_lbas?.length
                 ? `<div class="bad-lba-list"><strong>${L("Kararsız / Yavaş Fiziksel LBA'lar:", "Unstable / Slow Physical LBAs:", lang)}</strong><p class="mono">${rs.bad_lbas.slice(0, 12).map((x) => `LBA ${x.lba}${x.latency_ms != null ? ` (${fmt(x.latency_ms, 0)}ms)` : " (ERR)"}`).join(" · ")}</p></div>`
                 : ""
             }
             ${
               isHdd && selectedVol
                 ? `<div class="opt-actions">
                      ${optBtn("guard_tail", `${d.id}:${selectedVol}`, String(zbr?.suggested_tail_guard_pct ?? 5), L(`Ölü Kuyruğu Bölüm Dışına Kırp (%${zbr?.suggested_tail_guard_pct ?? 5})`, `Trim Degraded Tail (${zbr?.suggested_tail_guard_pct ?? 5}%)`, lang), !volReady, zbr?.tail_degraded ? "primary" : "quiet")}
                      ${optBtn("guard_tail", `${d.id}:${selectedVol}`, "10", L("Kuyruğu %10 Kırp (Unallocated Guard)", "Guard Tail 10% (Unallocated)", lang), !volReady)}
                      ${qRec || (rs.isolated_clusters ?? 0) > 0 ? optBtn("release_quarantine", `${d.id}:${selectedVol}`, "release", L("Karantina Kalkanını Kaldır", "Release Quarantine", lang), !volReady) : ""}
                    </div>`
                 : ""
             }
           </div>`
        : ""
  }${job?.damaged_files.length ? `<div class="damaged-files"><h3>${L("Okunamayan / Hasarlı LBA ile Çakışan Dosyalar", "Files Overlapping Unreadable / Damaged LBAs", lang)}</h3>${job.damaged_files.map((v) => `<p><code>${e(v.path)}</code><span>${e(v.error)}</span></p>`).join("")}</div>` : ""}</article></div></div>${jobHistory(surfaceHistory, "surface")}`;
}
const blockStatus = (b: Block) =>
  ({
    fast: L("Hızlı", "Fast"),
    normal: L("Normal", "Normal"),
    slow: L("Yavaş", "Slow"),
    warning: L("Gecikmeli", "Delayed"),
    bad: L("Okuma hatası", "Read error"),
  })[b.status] ?? L("Ölçülmedi", "Pending");

function jobHistory(jobs: Job[], kind: string): string {
  const lang = getLang();
  return `<article class="card saved-jobs"><div class="section-title"><div><h2>${kind === "benchmark" ? L("Önceki hız testleri", "Previous benchmarks", lang) : L("Önceki doğrulamalar", "Previous surface scans", lang)}</h2><p>${L("Son 20 yerel sonuç", "Latest 20 local runs", lang)}</p></div>${button(`export-${kind}`, L("Dışa aktar", "Export", lang), "export", "compact", !jobs.length)}</div>${jobs.length ? `<div class="table-wrap"><table><thead><tr><th>${L("Tarih", "Date", lang)}</th><th>${L("Hedef", "Target", lang)}</th><th>${L("Durum", "Status", lang)}</th><th>${L("Mod", "Mode", lang)}</th><th></th></tr></thead><tbody>${jobs.map((j) => `<tr><td>${date(j.started_at)}</td><td>${e(j.letter)}: · ${e(j.model)}</td><td>${pill(statusText(j.status), j.status === "completed" ? "ok" : j.status === "failed" ? "warning" : "")}</td><td>${e(j.options?.mode)}</td><td><button class="text-button" data-view-job="${j.id}" data-kind="${kind}">${L("İncele", "Inspect", lang)} ${icon("arrow")}</button></td></tr>`).join("")}</tbody></table></div>` : `<div class="inline-empty">${icon("history")}${L("Henüz kaydedilmiş bir test yok.", "No recorded runs yet.", lang)}</div>`}</article>`;
}

function recoveryView(d: Disk): string {
  const lang = getLang();
  const job = currentJob("recovery") ?? recoveryHistory[0] ?? null;
  if (job?.id && job.id !== lastRecoveryJobId) {
    lastRecoveryJobId = job.id;
    selectedRecoveryIds.clear();
    recoveryTabManual = false;
  } else if (!job && lastRecoveryJobId) {
    lastRecoveryJobId = "";
    selectedRecoveryIds.clear();
    recoveryTabManual = false;
  }

  const rawItems: RecoveredItem[] = job?.recovery_items ?? [];
  const apps: DeletedAppBundle[] = job?.deleted_apps ?? [];
  const unopenableCount = rawItems.filter((i) => isUnopenableMedia(i)).length;
  const visiblePool = showUnopenableMedia
    ? rawItems
    : rawItems.filter((i) => !isUnopenableMedia(i));

  const activeTab: "apps" | "files" = recoveryTabManual
    ? recoveryTab
    : job?.options?.mode === "dev"
      ? "files"
      : apps.length > 0
        ? "apps"
        : rawItems.length > 0
          ? "files"
          : "apps";

  const filtered = aiNaturalFilter(visiblePool, recoverySearch, recoveryCat);
  const sorted = [...filtered].sort((a, b) => {
    if (recoverySort === "size") return b.size_bytes - a.size_bytes;
    if (recoverySort === "recent")
      return (b.deleted_at ?? "").localeCompare(a.deleted_at ?? "");
    if (recoverySort === "name") return a.name.localeCompare(b.name, localeTag(lang));
    return 0;
  });

  const canLive = canStart(state, d) && !pending;
  const needsAdmin = recConfig.mode === "deep";
  const startDisabled = !canLive || (needsAdmin && !state?.admin);
  const rs = job?.repair_summary as Record<string, unknown> | undefined;

  const catCounts = new Map<string, number>();
  for (const it of visiblePool) {
    catCounts.set(it.category, (catCounts.get(it.category) ?? 0) + 1);
  }
  const userVisibleCount = visiblePool.filter(
    (i) => i.category !== "system_lib",
  ).length;
  const filesTabCount =
    userVisibleCount > 0 || visiblePool.length === 0
      ? userVisibleCount
      : visiblePool.length;
  const categories: [string, string, number][] = [
    ["all", L("Tümü", "All", lang), userVisibleCount],
    ["exe_setup", L("EXE & Kurulumlar", "EXE & Installers", lang), catCounts.get("exe_setup") ?? 0],
    ["game_portable", L("Oyun & Taşınabilir", "Games & Portable", lang), catCounts.get("game_portable") ?? 0],
    ["document", L("Belgeler", "Documents", lang), catCounts.get("document") ?? 0],
    ["photo", L("Fotoğraf & Görsel", "Photos & Images", lang), catCounts.get("photo") ?? 0],
    ["video_audio", L("Video & Ses", "Video & Audio", lang), catCounts.get("video_audio") ?? 0],
    ["archive", L("Arşiv", "Archives", lang), catCounts.get("archive") ?? 0],
    ["database_code", L("Veritabanı & Kod", "Database & Code", lang), catCounts.get("database_code") ?? 0],
    ["system_lib", L("DLL & Sistem", "DLL & System", lang), catCounts.get("system_lib") ?? 0],
  ];

  const isGameCat = (cat?: string | null) => {
    const low = (cat ?? "").toLocaleLowerCase("tr");
    return low === "oyun" || low === "game";
  };

  const appCounts = {
    all: apps.length,
    full_restore: apps.filter(
      (a) => inferAppRecoveryStatus(a) === "full_restore",
    ).length,
    installer_found: apps.filter(
      (a) => inferAppRecoveryStatus(a) === "installer_found",
    ).length,
    games: apps.filter((a) => isGameCat(a.category)).length,
    reinstall_only: apps.filter(
      (a) => inferAppRecoveryStatus(a) === "reinstall_only",
    ).length,
  };
  const filteredApps = filterDeletedApps(apps, appSearch, appFilter);
  const appFilterTabs: [AppFilter, string, number][] = [
    ["all", L("Tümü", "All", lang), appCounts.all],
    ["full_restore", L("Tam Kurtarılabilir", "Full Restore", lang), appCounts.full_restore],
    ["installer_found", L("Kurulum Bulundu", "Installer Found", lang), appCounts.installer_found],
    ["games", L("Oyunlar", "Games", lang), appCounts.games],
    ["reinstall_only", L("Kalıntı / Yeniden Kur", "Traces / Reinstall", lang), appCounts.reinstall_only],
  ];

  const mftScanned = Number(rs?.mft_scanned ?? 0);
  const vssFound = Number(rs?.vss_found ?? 0);
  const usnDeleted = Number(rs?.usn_deleted ?? 0);
  const exeFound = Number(
    rs?.exe_found ??
      rawItems.filter(
        (i) =>
          (i.category === "exe_setup" || i.category === "game_portable") &&
          !i.trim_zeroed,
      ).length,
  );
  const highConf = Number(rs?.high_confidence ?? 0);
  const sourceLabel = (src: string) =>
    ({
      vss_shadow: L("VSS Gölge Kopya (%100)", "VSS Shadow Copy (100%)", lang),
      installer_cache: L("MSI Önbelleği", "MSI Cache", lang),
      recycle_bin: L("Geri Dönüşüm", "Recycle Bin", lang),
      mft_resident: L("$MFT Yerleşik", "$MFT Resident", lang),
      mft_nonresident: L("$MFT Küme", "$MFT Clusters", lang),
      usn_journal: L("$UsnJrnl İzi", "$UsnJrnl Trace", lang),
      indx_slack: "$I30 Slack",
      carved: L("İmza Kazıma", "Header Carving", lang),
    })[src] ?? src;

  if (recConfig.mode === "apps_only") recConfig.mode = "quick";
  const recCard = (value: string, ico: string, title: string, text: string, tag = "") =>
    `<label class="mode-card ${recConfig.mode === value ? "selected" : ""}"><input type="radio" name="rec-mode" value="${value}" ${recConfig.mode === value ? "checked" : ""}><span>${icon(ico)}<strong>${title}${tag ? ` <em class="mode-tag">${tag}</em>` : ""}</strong><small>${text}</small></span></label>`;
  const recAuto =
    recConfig.mode === "deep"
      ? deepRecoveryEstimate(d)
      : recConfig.mode === "dev"
        ? L("Geri Dönüşüm, VSS, $MFT ve $UsnJrnl yalnız .env, kaynak kod ve veritabanı dosyalarına odaklanarak taranır (salt okunur).", "Scans Recycle Bin, VSS, $MFT, and $UsnJrnl focusing on .env, source code, and database files (read-only).", lang)
        : L("Geri Dönüşüm, gölge kopyalar, dosya tablosu ve program izleri taranır. Birkaç dakika sürer.", "Scans Recycle Bin, VSS Shadow Copies, NTFS $MFT, $UsnJrnl, and app traces. Read-only.", lang);

  const validItemIds = new Set(rawItems.map((i) => i.id));
  const activeSelectedIds = Array.from(selectedRecoveryIds).filter((id) =>
    validItemIds.has(id),
  );
  const selectedCount = activeSelectedIds.length;
  const recoverableVisibleCount = sorted.filter((it) =>
    isRecoverableRow(it),
  ).length;

  const appsTabHtml = `<div class="rec-tab-panel" role="tabpanel" aria-label="${L("Silinmiş Uygulamalar ve Oyunlar", "Deleted Apps & Games", lang)}">
    <div class="ai-search-bar">
      <input type="search" id="recovery-app-search" value="${e(appSearch)}" placeholder="${L("Uygulama, oyun, yayıncı veya yol ara…", "Search app, game, publisher, or path…", lang)}" aria-label="${L("Silinmiş uygulama ara", "Search deleted apps", lang)}">
    </div>
    <div class="cat-pills app-filter-pills" role="group" aria-label="${L("Uygulama kurtarma filtresi", "App recovery filter", lang)}">
      ${appFilterTabs
        .map(
          ([k, label, count]) =>
            `<button type="button" class="button compact ${appFilter === k ? "selected" : "quiet"}" data-app-filter="${k}">${label} (${count})</button>`,
        )
        .join("")}
    </div>
    ${
      filteredApps.length
        ? `<div class="app-bundle-grid">${filteredApps
            .map((app) => {
              const st = inferAppRecoveryStatus(app);
              const isGame = isGameCat(app.category);
              const catLabel = isGame
                ? L("Oyun", "Game", lang)
                : !app.category || app.category.toLocaleLowerCase("tr") === "uygulama" || app.category.toLowerCase() === "app"
                  ? L("Uygulama", "App", lang)
                  : app.category;
              const catCls = isGame ? "info" : "";
              const statusLabel =
                st === "full_restore"
                  ? L("%100 Geri Getirilebilir", "100% Recoverable", lang)
                  : st === "installer_found"
                    ? L("Kurulum Bulundu", "Installer Found", lang)
                    : app.appdata_paths.length
                      ? L("Ayarlar Korunmuş · Yeniden Kur", "Settings Intact · Reinstall", lang)
                      : L("Kalıntı · Yeniden Kur", "Traces · Reinstall", lang);
              const statusCls =
                st === "full_restore"
                  ? "ok"
                  : st === "installer_found"
                    ? "info"
                    : "warning";
              const appDataMeta =
                app.leftover_bytes && app.leftover_bytes > 0
                  ? L(`Korunan AppData: ${fmtBytes(app.leftover_bytes)}${app.appdata_paths.length ? ` (${app.appdata_paths.length} klasör)` : ""}`, `Preserved AppData: ${fmtBytes(app.leftover_bytes)}${app.appdata_paths.length ? ` (${app.appdata_paths.length} folders)` : ""}`, lang)
                  : app.appdata_paths.length
                    ? L(`${app.appdata_paths.length} AppData ayar klasörü korundu`, `${app.appdata_paths.length} AppData config folder(s) preserved`, lang)
                    : L("AppData kalıntısı yok", "No leftover AppData folders", lang);
              const primaryAction =
                st === "full_restore" && app.recoverable_item_id && job
                  ? `<button type="button" class="button compact primary app-primary-btn" data-restore-job="${e(job.id)}" data-restore-id="${e(app.recoverable_item_id)}" ${state?.source !== "live" || pending ? "disabled" : ""}>${icon("recover")}<span>${L("VSS / Geri Dönüşüm'den Kurtar", "Restore from VSS / Recycle Bin", lang)}</span></button>`
                  : app.recoverable_item_id && job
                    ? `<button type="button" class="button compact primary app-primary-btn" data-restore-job="${e(job.id)}" data-restore-id="${e(app.recoverable_item_id)}" ${state?.source !== "live" || pending ? "disabled" : ""}>${icon("recover")}<span>${L("Kurulumu Kurtar", "Recover Installer", lang)}</span></button>`
                    : `<button type="button" class="button compact accent-outline app-primary-btn" data-copy-winget="${e(app.winget_hint)}">${icon("copy")}<span>${L("Winget Komutunu Kopyala", "Copy Winget Command", lang)}</span></button>`;
              const evKey = `app-ev:${app.name}:${app.exe_path}`;
              return `<article class="app-bundle-card app-status-${st}">
                <div class="app-card-head">
                  <div class="app-card-title">
                    <strong>${e(app.name)}</strong>
                    ${app.publisher ? `<small>${e(app.publisher)}</small>` : ""}
                  </div>
                  <div class="inline-actions">
                    ${pill(catLabel, catCls)}
                    ${app.drive_letter ? pill(`${app.drive_letter}:`) : ""}
                  </div>
                </div>
                <div class="app-status-row">
                  <span class="pill app-status-badge ${statusCls}">${e(statusLabel)}</span>
                  <span class="app-conf-meta">${L(`%${app.confidence} güven`, `${app.confidence}% confidence`, lang)}</span>
                </div>
                <div class="app-card-meta">
                  ${
                    app.run_count || app.last_run_at
                      ? `<span>${icon("clock")}${app.run_count ? L(`${app.run_count} kez çalıştırıldı`, `Launched ${app.run_count}x`, lang) : ""}${app.run_count && app.last_run_at ? " · " : ""}${app.last_run_at ? `${L("Son", "Last", lang)}: ${date(app.last_run_at)}` : ""}</span>`
                      : ""
                  }
                  <span>${icon("folder")}${e(appDataMeta)}</span>
                </div>
                <div class="app-card-action">${primaryAction}</div>
                <details class="app-evidence-details" data-details-key="${e(evKey)}">
                  <summary><span>${L(`Adli kanıtlar (${app.sources?.length ?? 0}) &amp; dosya yolu`, `Forensic artifacts (${app.sources?.length ?? 0}) &amp; paths`, lang)}</span>${icon("chevron")}</summary>
                  <div class="app-evidence-body">
                    ${
                      app.sources?.length
                        ? `<div class="inline-actions">${app.sources.map((s) => pill(s, /VSS|Geri Dönüşüm|Recycle/i.test(s) ? "ok" : "")).join("")}</div>`
                        : ""
                    }
                    <div class="app-ev-line"><span>${L("Çalıştırılabilir yol", "Executable path", lang)}</span><code class="mono">${e(app.exe_path)}</code></div>
                    ${app.install_dir ? `<div class="app-ev-line"><span>${L("Kurulum klasörü", "Install folder", lang)}</span><code class="mono">${e(app.install_dir)}</code></div>` : ""}
                    ${
                      app.appdata_paths.length
                        ? `<div class="app-ev-line"><span>${L("Korunan ayarlar", "Preserved settings", lang)}</span><code class="mono">${e(app.appdata_paths.join(" · "))}</code></div>`
                        : ""
                    }
                    <div class="app-ev-winget">
                      <code class="mono">${e(app.winget_hint)}</code>
                      <button type="button" class="button compact quiet" data-copy-winget="${e(app.winget_hint)}">${icon("copy")}<span>${L("Kopyala", "Copy", lang)}</span></button>
                    </div>
                  </div>
                </details>
              </article>`;
            })
            .join("")}</div>`
        : empty(
            apps.length
              ? L("Filtreyle eşleşen uygulama yok", "No matching apps found", lang)
              : L("Henüz silinmiş uygulama listelenmedi", "No deleted apps listed yet", lang),
            apps.length
              ? L("Arama metnini temizleyin veya üstten başka bir filtre seçin.", "Clear search text or select another filter tab above.", lang)
              : L("Silinmiş program ve oyunları tespit etmek için soldan taramayı başlatın.", "Start a recovery scan on the left to detect uninstalled apps and games.", lang),
            "recover",
          )
    }
  </div>`;

  const filesTabHtml = `<div class="rec-tab-panel" role="tabpanel" aria-label="${L("Kurtarılabilir Dosyalar", "Recoverable Files", lang)}">
    <div class="ai-search-bar">
      <input type="search" id="recovery-search" value="${e(recoverySearch)}" placeholder="${L("Ara: 'exe', 'oyun', 'pdf', 'foto', 'sağlam', 'rufus', 'dll'…", "Filter: 'exe', 'game', 'pdf', 'photo', 'healthy', 'dll'…", lang)}" aria-label="${L("Kurtarma dosyası arama ve akıllı filtre", "Filter recoverable files", lang)}">
      <select id="recovery-sort" aria-label="${L("Sonuç sıralama", "Sort recovery results", lang)}">
        <option value="best" ${recoverySort === "best" ? "selected" : ""}>${L("Öncelik: Önce VSS, EXE &amp; Sağlam", "Priority: VSS, EXE &amp; High Integrity first", lang)}</option>
        <option value="size" ${recoverySort === "size" ? "selected" : ""}>${L("Boyut (Büyükten Küçüğe)", "Size (Largest first)", lang)}</option>
        <option value="recent" ${recoverySort === "recent" ? "selected" : ""}>${L("Silinme Tarihi (En Yeni)", "Deleted time (Newest)", lang)}</option>
        <option value="name" ${recoverySort === "name" ? "selected" : ""}>${L("Dosya Adı (A–Z)", "Filename (A–Z)", lang)}</option>
      </select>
    </div>
    <div class="cat-pills-row">
      <div class="cat-pills" role="group" aria-label="${L("Kategori filtresi", "Category filter", lang)}">
        ${categories
          .map(
            ([cat, label, count]) =>
              `<button type="button" class="button compact ${recoveryCat === cat ? "selected" : "quiet"}" data-rec-cat="${cat}">${label} (${count})</button>`,
          )
          .join("")}
      </div>
      ${
        unopenableCount > 0
          ? `<label class="unopenable-toggle"><input type="checkbox" id="show-unopenable-media" ${showUnopenableMedia ? "checked" : ""}><span>${L(`Bozuk / açılamayanları da göster (${unopenableCount})`, `Show corrupted / unopenable (${unopenableCount})`, lang)}</span></label>`
          : ""
      }
    </div>
    ${
      sorted.length
        ? `<div class="batch-recovery-bar ${selectedCount > 0 ? "has-selection" : ""}" role="region" aria-label="${L("Toplu kurtarma çubuğu", "Batch recovery bar", lang)}">
            <div class="batch-bar-info">
              <strong>${selectedCount > 0 ? L(`${selectedCount} dosya seçildi`, `${selectedCount} item(s) selected`, lang) : L("Toplu kurtarma", "Batch recovery", lang)}</strong>
              <small>${selectedCount > 0 ? L("Seçilenler tek seferde seçeceğiniz güvenli klasöre kurtarılır.", "Selected items are recovered safely into your chosen destination folder.", lang) : L(`${recoverableVisibleCount} kurtarılabilir dosya · birden fazla seçip tek tıkla kaydedin.`, `${recoverableVisibleCount} recoverable files · select multiple to save together.`, lang)}</small>
            </div>
            <div class="batch-bar-actions">
              <button type="button" class="button compact quiet" data-action="rec-select-all" ${recoverableVisibleCount === 0 ? "disabled" : ""}>${L("Tümünü Seç", "Select All", lang)}</button>
              <button type="button" class="button compact quiet" data-action="rec-clear-selection" ${selectedCount === 0 ? "disabled" : ""}>${L("Temizle", "Clear", lang)}</button>
              <button type="button" class="button compact primary" data-action="rec-restore-batch" ${selectedCount === 0 || state?.source !== "live" || pending ? "disabled" : ""}>${icon("folder")}<span>${L("Seçilenleri Klasöre Kurtar", "Restore Selected to Folder", lang)}${selectedCount > 0 ? ` (${selectedCount})` : ""}</span></button>
            </div>
          </div>`
        : ""
    }
    ${
      sorted.length
        ? `<div class="recovery-list">${sorted
            .map((it) => {
              const metaOnly =
                it.source === "usn_journal" || it.source === "indx_slack";
              const unopenable = isUnopenableMedia(it);
              const recoverable = isRecoverableRow(it);
              const checked = selectedRecoveryIds.has(it.id);
              const mBadge = mediaStatusBadge(it, lang);
              return `<div class="recovery-row ${checked ? "row-selected" : ""} ${unopenable ? "row-unopenable" : ""}">
                <label class="rec-check-wrap" title="${recoverable ? L("Toplu kurtarma için seç", "Select for batch restore", lang) : L("Bu öğe doğrudan kurtarılamaz", "Not directly recoverable", lang)}">
                  <input type="checkbox" class="rec-checkbox" data-rec-select="${e(it.id)}" ${checked ? "checked" : ""} ${!recoverable ? "disabled" : ""} aria-label="${e(it.name)}">
                </label>
                <div class="recovery-row-main">
                  <div class="inline-actions">
                    <strong>${e(it.name)}</strong>
                    ${pill(L(`%${it.confidence}`, `${it.confidence}%`, lang), it.trim_zeroed || unopenable ? "critical" : it.confidence >= 75 ? "ok" : "warning")}
                    ${mBadge ? pill(mBadge.label, mBadge.level) : ""}
                    ${it.sub_type ? pill(it.sub_type) : ""}
                    ${it.publisher ? pill(it.publisher) : ""}
                    ${pill(sourceLabel(it.source), it.source === "vss_shadow" ? "ok" : "")}
                  </div>
                  <small class="mono">${e(it.original_path)}${it.size_bytes ? ` · ${fmtBytes(it.size_bytes)}` : ""}${it.deleted_at ? ` · ${date(it.deleted_at)}` : ""}</small>
                  <p class="ai-note">${e(it.ai_summary)}</p>
                  ${it.repair_note ? `<p class="repair-note ${unopenable ? "repair-note-bad" : ""}">${icon(unopenable ? "alert" : "check")}<span>${e(it.repair_note)}</span></p>` : ""}
                </div>
                <div class="recovery-row-actions">
                  <button type="button" class="button compact quiet" data-preview-job="${e(job!.id)}" data-preview-id="${e(it.id)}" data-preview-name="${e(it.name)}" data-preview-recoverable="${recoverable ? "1" : "0"}" ${pending ? "disabled" : ""}>${icon("eye")}<span>${L("Önizle", "Preview", lang)}</span></button>
                  <button type="button" class="button compact primary" data-restore-job="${e(job!.id)}" data-restore-id="${e(it.id)}" ${state?.source !== "live" || !recoverable || pending ? "disabled" : ""}>${metaOnly ? L("Yalnız İz", "Trace Only", lang) : unopenable ? L("Açılamaz", "Corrupt", lang) : L("Kurtar", "Recover", lang)}</button>
                </div>
              </div>`;
            })
            .join("")}</div>`
        : empty(
            L("Kurtarılacak dosya listelenmedi", "No recoverable files listed", lang),
            job
              ? unopenableCount > 0 && !showUnopenableMedia
                ? L(`Mevcut filtrede açılabilir dosya yok (${unopenableCount} bozuk/açılamayan medya gizlendi).`, `No openable items match this filter (${unopenableCount} corrupted media hidden).`, lang)
                : L("Mevcut kategori veya arama filtresiyle eşleşen kullanıcı dosyası yok.", "No user files match the current category or query.", lang)
              : L("Silinmiş EXE, uygulama, fotoğraf ve kullanıcı dosyalarını tespit etmek için soldan taramayı başlatın.", "Start a scan on the left to find deleted EXEs, apps, photos, and documents.", lang),
            "recover",
          )
    }
  </div>`;

  return `${heading(d, L("DOSYA KURTARMA", "FILE RECOVERY", lang), L("Silinen dosya ve programları geri getirin", "Recover deleted files &amp; uninstalled apps", lang), L("Önce hızlı tarama yapın. Aradığınızı bulamazsanız derin taramayı deneyin.", "Start with Quick Scan; switch to Deep Carve if items were wiped longer ago.", lang))}
    <div class="scope-note">${icon("shield")}<div><strong>${L("Kurtardığınız dosyaları başka bir diske kaydedin.", "Save recovered files to a different drive.", lang)}</strong> ${L("Tarama diske hiç yazmaz; aynı diske kaydetmek diğer silinmiş dosyaların üzerine yazabilir. Bozuk fotoğraf ve videolar kurtarılırken otomatik onarılır.", "Scanning is strictly read-only. Writing output back to the source volume can overwrite deleted clusters. Corrupt JPEG/PNG/MP4 headers are repaired on restore.", lang)}</div></div>
    <div class="recovery-layout">
      <article class="card scan-settings">
        <div class="section-title"><div><h2>${L("Nasıl tarayalım?", "Scan method", lang)}</h2><p>${L("Birini seçin, gerisini uygulama ayarlar", "Select mode — safe parameters are automatic", lang)}</p></div></div>
        ${volumeSelect(d)}
        ${recCard("quick", "search", L("Hızlı tarama", "Quick scan", lang), L("Yakın zamanda silinen dosya ve programları bulur.", "Finds recently deleted files, Recycle Bin, VSS copies, and app traces.", lang), L("Önce bunu", "Start here", lang))}
        ${recCard("dev", "code", L("Geliştirici & Proje", "Developer & Project", lang), L(".env, kaynak kod, SQLite ve yapılandırma dosyalarını öncelikli tarar (salt okunur).", "Prioritizes .env, source code, SQLite DBs, and project configs (read-only).", lang), L("Vibe", "Dev", lang))}
        ${recCard("deep", "cpu", L("Derin tarama", "Deep signature carve", lang), `${L("Tüm boş alanı sektör sektör kazır; formatlanmış veya eski dosyaları da bulur.", "Carves unallocated space sector-by-sector for formatted or older files.", lang)}${!state?.admin ? L(" · Yönetici gerekir", " · Admin required", lang) : ""}`)}
        <div class="auto-budget">${icon("settings")}<div><strong>${L("Otomatik ayarlandı", "Auto-configured", lang)}</strong><small>${e(recAuto)}</small></div></div>
        ${button("start-recovery", recConfig.mode === "deep" ? L("Derin taramayı başlat", "Start Deep Scan", lang) : recConfig.mode === "dev" ? L("Geliştirici taramasını başlat", "Start Developer Scan", lang) : L("Hızlı taramayı başlat", "Start Quick Scan", lang), "recover", "primary full", startDisabled)}
        <p class="form-hint">${e(disabledReason(d) || (needsAdmin && !state?.admin ? L("Derin tarama için sağ üstten “Yönetici olarak aç”a basın.", "Click “Run as Admin” in the top bar for raw deep carving.", lang) : L("Tarama salt okunurdur; istediğiniz an durdurabilirsiniz.", "Scanning is read-only; stop anytime.", lang)))}</p>
      </article>

      <article class="card recovery-results-card">
        <div class="section-title"><div><h2>${L("Tespit edilen dosya ve uygulamalar", "Detected files &amp; applications", lang)}</h2><p>${job ? `${rawItems.length} ${L("öğe", "items", lang)} · ${apps.length} ${L("silinmiş uygulama", "deleted apps", lang)}${vssFound ? ` · ${vssFound} VSS %100` : ""}${usnDeleted ? ` · ${usnDeleted} $UsnJrnl` : ""}` : L("Henüz kurtarma taraması yapılmadı", "No recovery scan run yet", lang)}</p></div>${job && rs ? pill(`${highConf} ${L("Sağlam", "Intact", lang)}`, "ok") : ""}</div>
        ${
          job
            ? `<div class="rec-telemetry">
                 <div><span>${L("Okunan Sektör", "Bytes Scanned", lang)}</span><strong>${fmtBytes(job.read_bytes)}</strong></div>
                 <div><span>${L("İncelenen $MFT", "$MFT Parsed", lang)}</span><strong>${fmt(mftScanned)} ${L("kayıt", "records", lang)}</strong></div>
                 <div><span>${L("Bulunan EXE / VSS", "EXEs / VSS", lang)}</span><strong>${exeFound} EXE · ${vssFound} VSS</strong></div>
                 <div><span>${L("Yüksek Bütünlük", "High Integrity", lang)}</span><strong>${highConf}</strong></div>
               </div>`
            : ""
        }
        ${jobProgress(job)}
        <div class="recovery-tabs" role="tablist" aria-label="${L("Kurtarma sonucu sekmeleri", "Recovery result tabs", lang)}">
          <button type="button" role="tab" aria-selected="${activeTab === "apps"}" class="rec-tab ${activeTab === "apps" ? "selected" : ""}" data-rec-tab="apps">${icon("cpu")}<span>${L("Silinmiş Uygulamalar &amp; Oyunlar", "Deleted Apps &amp; Games", lang)}</span><span class="rec-tab-count">${apps.length}</span></button>
          <button type="button" role="tab" aria-selected="${activeTab === "files"}" class="rec-tab ${activeTab === "files" ? "selected" : ""}" data-rec-tab="files">${icon("recover")}<span>${L("Kurtarılabilir Dosyalar", "Recoverable Files", lang)}</span><span class="rec-tab-count">${filesTabCount}</span></button>
        </div>
        ${activeTab === "apps" ? appsTabHtml : filesTabHtml}
      </article>
    </div>`;
}

function historyView(d: Disk): string {
  const lang = getLang();
  return `${heading(d, L("ZAMAN İÇİNDE", "OVER TIME", lang), L("Disk geçmişi ve olaylar", "Drive history &amp; events", lang), L("İlk gözlem başlangıç değeridir. Sonraki sayaç artışları ayrıca işaretlenir.", "Initial readings set the baseline; subsequent counter changes are logged as events.", lang))}
    <div class="two-columns"><article class="card"><div class="section-title"><div><h2>${L("Sağlık / dayanım", "Health / endurance", lang)}</h2><p>${L("Gerçek yerel ölçümler", "Local recorded samples", lang)}</p></div>${pill(`${history.length} ${L("ölçüm", "samples", lang)}`)}</div>${chart(
      history.map((h) => h.health_percent),
      "var(--accent)",
      L("Sağlık yüzdesi geçmişi", "Health percentage history", lang),
    )}<div class="chart-footer"><span>${date(history[0]?.time)}</span><span>${date(history.at(-1)?.time)}</span></div></article><article class="card"><div class="section-title"><div><h2>${L("Sıcaklık", "Temperature", lang)}</h2><p>${L("Aygıtın bildirdiği değerler", "Vendor sensor history", lang)}</p></div>${pill("°C")}</div>${chart(
      history.map((h) => h.temperature_c),
      "var(--blue)",
      L("Sıcaklık geçmişi", "Temperature history", lang),
    )}<div class="chart-footer"><span>${history.length < 2 ? L("Eğilim için daha fazla örnek gerekli", "Collect more samples for trends", lang) : L("Kaydedilmiş ölçüm zamanları", "Recorded sample timestamps", lang)}</span></div></article></div>
    <article class="card event-card"><div class="section-title"><div><h2>${L("Olay günlüğü", "Event log", lang)}</h2><p>${L("Son 200 olay", "Latest 200 events", lang)} · ${state?.source === "replay" ? L("kayıt kaynağı", "snapshot replay", lang) : L("canlı kaynak", "live telemetry", lang)}</p></div><span class="small-count">${events.length}</span></div>${events.length ? events.map((ev) => `<div class="event-row"><span class="event-icon ${ev.level}">${icon(ev.level === "info" ? "info" : "alert")}</span><div><strong>${e(ev.message)}</strong><small>${e(ev.code)}</small></div><time>${date(ev.time)}</time></div>`).join("") : empty(L("Henüz yeni bir olay yok", "No events recorded yet", lang), L("İlk okuma geçmiş sayacınızı başlangıç kabul eder; eski hatalar yeni olay olarak bildirilmez.", "First scan captures your baseline counters; only new threshold changes appear here.", lang), "history")}</article>`;
}

function toolsView(d: Disk): string {
  const lang = getLang();
  const live = canMaintain(state, d) && !pending;
  const autostart = !!state?.windows?.system_tuning?.autostart_enabled;
  const healthMode = state?.health_mode === "strict" ? "strict" : "default";
  const autoUpdateOn = state?.auto_check_updates !== false;
  const updateMsg = updateStatus
    ? translateUpdateMessage(updateStatus, lang)
    : autoUpdateOn
      ? L(
          `Yüklü sürüm v${state?.version ?? "1.10.2"} · canlı modda açılışta yeni sürümler kontrol edilir.`,
          `Installed v${state?.version ?? "1.10.2"} · checks for new releases once on launch in live mode.`,
          lang,
        )
      : L(
          `Yüklü sürüm v${state?.version ?? "1.10.2"} · otomatik denetim kapalı (yalnız elle kontrol).`,
          `Installed v${state?.version ?? "1.10.2"} · automatic checks off (manual check only).`,
          lang,
        );
  return `${heading(d, L("BAKIM & AYARLAR", "TOOLS & SETTINGS", lang), L("Bakım araçları ve uygulama ayarları", "Maintenance tools &amp; preferences", lang), L("Her işlem başlamadan önce onay ister.", "Every write or firmware command prompts for confirmation before running.", lang))}
    <div class="scope-note">${icon("shield")}<div><strong>${L("Yedeğiniz her zaman ilk adım.", "Keep backups first.", lang)}</strong> ${L("SMART testleri ve dosya sistemi kontrolleri fiziksel hasarı iyileştirme garantisi vermez.", "Hardware self-tests and filesystem scans help diagnose drives but cannot fix failing flash or platters.", lang)}</div></div>
    <div class="tool-grid">${toolCard("smart", L("SMART günlükleri", "SMART device logs", lang), L("Aygıtın kaydettiği hata ve dahili test sonuçlarını okuyun. Test başlatmaz.", "Inspect hardware error & self-test logs read-only without starting a new test.", lang), "logs", L("Günlükleri oku", "Read logs", lang), pending)}${toolCard("activity", L("Kısa dahili test", "Short self-test", lang), L("Disk firmware’inin desteklediği kısa öz testi başlatır. İlerleme ve sonuç günlükte izlenir.", "Triggers the firmware short self-test (typically 1–2 min). Results appear in logs.", lang), "short_test", L("Kısa testi başlat", "Run short test", lang), !live || !state?.admin || !d.capabilities.self_test)}${toolCard("clock", L("Uzun dahili test", "Extended self-test", lang), L("Kapsamlı firmware öz testi. İş yükünü etkileyebilir ve uzun sürebilir.", "Runs a full surface firmware self-test. May affect I/O throughput while active.", lang), "long_test", L("Uzun testi başlat", "Run extended test", lang), !live || !state?.admin || !d.capabilities.self_test)}${toolCard("refresh", "SSD ReTrim", L("Windows’a boş blokları yeniden bildirmesini söyler. Seçilen NTFS / ReFS bölümü doğrulanır.", "Asks Windows to re-trim free blocks on the selected NTFS / ReFS volume.", lang), "retrim", L("ReTrim iste", "Run ReTrim", lang), !live || !state?.admin || d.type === "HDD" || !canStart(state, d))}</div>
    <article class="card settings-card"><div class="section-title"><div><h2>${L("Uygulama tercihleri", "Application preferences", lang)}</h2><p>${L("Windows masaüstü entegrasyonu ve hesaplama ayarları", "Desktop integration, language, and health calculation settings", lang)}</p></div>${pill("RUST + TAURI 2", "ok")}</div>
      <div class="setting-row"><span>${icon("settings")}<span><strong>${L("Uygulama dili", "Application language", lang)}</strong><small>${L("Arayüz ve sistem tepsisi bildirim dili (TR / EN).", "UI and system tray notification language (TR / EN).", lang)}</small></span></span><select id="language-setting" aria-label="${L("Uygulama dili", "Application language", lang)}"><option value="tr" ${lang === "tr" ? "selected" : ""}>Türkçe (TR)</option><option value="en" ${lang === "en" ? "selected" : ""}>English (EN)</option></select></div>
      <label class="setting-row"><span>${icon("disk")}<span><strong>${L("Kapatınca sistem tepsisine küçült", "Minimize to tray on close", lang)}</strong><small>${L("İzleme arka planda devam eder. Tam çıkış için tepsi menüsünü kullanın.", "Background SMART & temperature monitoring stays active; use tray menu to quit.", lang)}</small></span></span><input type="checkbox" role="switch" id="minimise-setting" ${state?.minimise_to_tray ? "checked" : ""}></label>
      <label class="setting-row"><span>${icon("bolt")}<span><strong>${L("Windows açılışında tepside başlat", "Launch minimized on Windows startup", lang)}</strong><small>${L("Oturum açılışında sessizce sistem tepsisinde başlar (HKCU\\Run).", "Starts silently in the system tray at login via HKCU\\Run.", lang)}</small></span></span><input type="checkbox" role="switch" id="autostart-setting" ${autostart ? "checked" : ""} ${state?.source !== "live" || state?.stale || pending ? "disabled" : ""}></label>
      <div class="setting-row"><span>${icon("shield")}<span><strong>${L("Sağlık hesaplama modu", "Health calculation mode", lang)}</strong><small>${L("Sıkı modda HDD/SATA hata sayaçları daha yüksek ağırlık ve kuyruk cezası alır.", "Strict mode applies heavier penalties to ATA/SATA sector reallocate & pending error counters.", lang)}</small></span></span><select id="health-mode-setting" aria-label="${L("Sağlık hesaplama modu", "Health calculation mode", lang)}"><option value="default" ${healthMode === "default" ? "selected" : ""}>${L("Varsayılan (Standart)", "Default (Standard)", lang)}</option><option value="strict" ${healthMode === "strict" ? "selected" : ""}>${L("Sıkı Mod (Erken Uyarı)", "Strict Mode (Early Warning)", lang)}</option></select></div>
      <label class="setting-row"><span>${icon("lock")}<span><strong>${L("Salt-okunur koruma kilidi (Write Guard)", "Read-Only Guard (Write Protection)", lang)}</strong><small>${L("Etkin olduğunda disklere yazan hız testi, onarım, optimize ve taşıma işlemleri Rust çekirdeğinde engellenir.", "When enabled, mutating operations (benchmarks, repairs, tuning, tiering) are blocked at the Rust core boundary.", lang)}</small></span></span><input type="checkbox" role="switch" id="write-guard-setting" ${isWriteLocked(state) ? "checked" : ""} ${state?.source !== "live" || pending ? "disabled" : ""}></label>
      <div class="setting-row"><span>${icon("temperature")}<span><strong>${L("Yüksek sıcaklık uyarısı", "Thermal alert threshold", lang)}</strong><small>${L("55 °C’de tek bildirim; 50 °C’ye düşünce yeniden etkinleşir.", "Single notification at 55 °C with hysteresis reset below 50 °C.", lang)}</small></span></span>${pill(state?.source === "replay" ? L("Kayıt modunda kapalı", "Off in replay", lang) : L("Etkin", "Active", lang), state?.source === "replay" ? "" : "ok")}</div>
      <div class="setting-row"><span>${icon("lock")}<span><strong>${L("Çalışma yetkisi", "Privilege level", lang)}</strong><small>${L("Bakım ve optimizasyon işlemleri için yönetici olarak açmak gerekir.", "Raw SMART I/O, benchmarks, and tuning require administrator elevation.", lang)}</small></span></span><div class="inline-actions">${pill(state?.admin ? L("Yönetici", "Administrator", lang) : L("Standart kullanıcı", "Standard user", lang))}${!state?.admin && state?.source === "live" ? button("elevate", L("Yönetici olarak aç", "Run as Admin", lang), "lock", "compact quiet", pending) : ""}</div></div>
      <label class="setting-row"><span>${icon("refresh")}<span><strong>${L("Açılışta otomatik güncelleme denetimi", "Check for updates automatically on startup", lang)}</strong><small>${L("Canlı modda açılışta GitHub Releases bir kez kontrol edilir; indirme ve kurulum her zaman SHA-256 ve onayınızla başlar.", "Checks GitHub Releases once on launch in live mode; download and install always require SHA-256 verification and your confirmation.", lang)}</small></span></span><input type="checkbox" role="switch" id="auto-update-setting" ${autoUpdateOn ? "checked" : ""} ${pending ? "disabled" : ""}></label>
      <div class="setting-row"><span>${icon("info")}<span><strong>${L("Sürüm ve güncelleme", "Version &amp; updates", lang)}</strong><small>${e(updateMsg)}</small></span></span><div class="inline-actions">${updateStatus?.status === "up_to_date" && updateStatus.url ? button("open-release", L("GitHub Sürümleri", "GitHub Releases", lang), "arrow", "compact quiet", pending) : ""}${button("check-updates", L("Güncellemeleri kontrol et", "Check for updates", lang), "refresh", "compact quiet", pending)}</div></div>
      ${
        updateStatus?.status === "update_available"
          ? `<div class="update-panel" role="region" aria-label="${L("Yeni sürüm bilgisi", "Update details", lang)}">
               <div class="update-panel-head">
                 <div class="inline-actions">${icon("info")}<strong>${L(`Yeni sürüm hazır: ${e(updateStatus.latest ?? "")}`, `New release available: ${e(updateStatus.latest ?? "")}`, lang)}</strong>${updateStatus.asset_size ? pill(fmtBytes(updateStatus.asset_size), "ok") : ""}</div>
                 <div class="inline-actions">
                   ${updateStatus.download_url && updateStatus.asset_name ? button("install-update", L("İndir ve Güncelle (SHA-256)", "Download &amp; Update (SHA-256)", lang), "export", "compact primary", pending) : ""}
                   ${updateStatus.url ? button("open-release", L("GitHub Sürüm Notları", "Release Notes", lang), "arrow", "compact quiet", pending) : ""}
                 </div>
               </div>
               ${updateStatus.notes ? `<p class="update-notes">${e(updateStatus.notes)}</p>` : ""}
             </div>`
          : ""
      }
    </article><div class="maintenance-footer">${button("abort_test", L("Dahili testi durdur", "Abort self-test", lang), "stop", "quiet", !live || !state?.admin || !d.capabilities.self_test)}<p>${e(writeDisabledReason(d) || L("Dahili testler uygulama dışında disk firmware’inde sürer; sonuç için günlükleri kontrol edin.", "Firmware self-tests continue inside the drive even if you close this screen; check SMART logs for completion.", lang))}</p></div>`;
}
function toolCard(
  ico: string,
  title: string,
  detail: string,
  action: string,
  label: string,
  disabled: boolean,
): string {
  return `<article class="card tool-card"><div class="tool-icon">${icon(ico)}</div><h2>${title}</h2><p>${detail}</p>${button(action, label, "arrow", "quiet", disabled)}</article>`;
}

function currentPollSignature(): string {
  const detailCounts = `${history.length}:${events.length}:${benchmarkHistory.length}:${surfaceHistory.length}:${defragHistory.length}:${smartTierHistory.length}:${autopilotHistory.length}:${recoveryHistory.length}`;
  return computePollSignature(state, {
    page,
    selected,
    letter,
    connectionError,
    lang: getLang(),
    detailCounts,
    forecast: healthForecast,
    checkpoint: surfaceCheckpoint,
    tierAnalysis,
    tierMoves,
    tierBusy: tierAnalyzing || tierTransferring,
    devAnalysis,
    devBusy: devScanning || devCleaning,
    devFilter,
    devCustomRoot,
  });
}

function render() {
  const lang = getLang();
  const focus = document.activeElement as HTMLInputElement | null;
  const id = focus?.id,
    start = focus?.selectionStart,
    end = focus?.selectionEnd;
  const scroll = document.querySelector("#main")?.scrollTop ?? 0;
  const openKeys = new Set<string>();
  const openUnkeyed: boolean[] = [];
  document.querySelectorAll<HTMLDetailsElement>("details").forEach((el) => {
    const k = el.dataset.detailsKey;
    if (k) {
      if (el.open) openKeys.add(k);
    } else {
      openUnkeyed.push(el.open);
    }
  });
  const d = disk();
  const partialWarnings = state?.partial_warnings ?? [];
  let content = d
    ? {
        overview,
        smart: smartView,
        performance: performanceView,
        optimize: optimizeView,
        surface: surfaceView,
        recovery: recoveryView,
        history: historyView,
        tools: toolsView,
      }[page](d)
    : empty(
        state?.updating ? L("Diskler okunuyor…", "Scanning drives…", lang) : L("Disk bilgisi bulunamadı", "No drives detected", lang),
        state?.error ??
          L("smartctl bağlantısını ve yönetici erişimini kontrol edin.", "Verify smartctl bundle path and administrator permissions.", lang),
        "disk",
      );
  const partialBanner = partialWarnings.length
    ? `<div class="warning-banner" role="status"><div class="warning-banner-main">${icon("alert")}<div class="warning-banner-text"><strong>${L("Kısmi SMART okuma uyarısı", "Partial SMART read warning", lang)}</strong><ul>${partialWarnings.map((w) => `<li>${e(w)}</li>`).join("")}</ul></div></div>${!state?.admin && state?.source === "live" ? button("elevate", L("Yönetici olarak aç", "Run as Admin", lang), "lock", "compact primary warning-elevate", pending) : ""}</div>`
    : "";
  const updateBanner =
    updateStatus?.status === "update_available" &&
    !updateBannerDismissed &&
    page !== "tools"
      ? `<div class="update-banner" role="status"><div class="update-banner-main">${icon("info")}<div><strong>${L(`Yeni sürüm hazır: ${e(updateStatus.latest ?? "")}`, `New release available: ${e(updateStatus.latest ?? "")}`, lang)}</strong><p>${L("Resmi GitHub sürümü SHA-256 doğrulaması ile tek tıkla kurulabilir.", "Official GitHub release verified with SHA-256 — one click to install.", lang)}</p></div></div><div class="inline-actions">${updateStatus.download_url && updateStatus.asset_name ? button("install-update", L("İndir ve Güncelle (SHA-256)", "Download &amp; Update (SHA-256)", lang), "export", "compact primary", pending) : ""}${updateStatus.url ? button("open-release", L("Sürüm Notları", "Release Notes", lang), "arrow", "compact quiet", pending) : ""}${button("dismiss-update-banner", L("Daha sonra", "Later", lang), "stop", "compact quiet", false)}</div></div>`
      : "";
  root.innerHTML = `${navigation()}<div class="workspace">${header(d)}<main id="main" tabindex="-1">${connectionError ? `<div class="error-banner">${icon("alert")}${e(connectionError)}</div>` : ""}${state?.error ? `<div class="error-banner">${icon("alert")}${e(state.error)}</div>` : ""}${partialBanner}${updateBanner}${state?.source === "replay" ? `<div class="replay-banner">${icon("history")}${L("Kaydedilmiş SMART ölçümleri görüntüleniyor. Canlı disk işlemleri devre dışı.", "Snapshot replay active. Live disk operations are disabled.", lang)}</div>` : ""}${content}<footer class="page-footer"><span>${icon("shield")}SSD Sentinel <b>v${e(state?.version ?? "1.10.2")}</b></span><span>${L("Yerel ölçüm. Açık veri. Bilinçli karar.", "Local telemetry. Transparent data. Informed action.", lang)}</span><span class="rust-badge">Rust <i>+</i> Tauri</span></footer></main>${state?.job && active(state.job) ? `<div class="active-job-bar"><span class="live-dot"></span><div><strong>${e(state.job.model)} · ${e(state.job.letter)}:</strong><small>${e(state.job.phase)}</small></div><progress max="100" value="${state.job.progress}"></progress><b>${state.job.progress}%</b>${button("cancel-job", L("Durdur", "Stop", lang), "stop", "compact danger", state.job.status === "cancelling")}</div>` : ""}</div>`;
  document.querySelector("#main")!.scrollTop = scroll;
  if (id) {
    const input = document.getElementById(id) as HTMLInputElement | null;
    input?.focus({ preventScroll: true });
    if (input?.tagName === "INPUT" && start !== null && start !== undefined) {
      try {
        input.setSelectionRange(start, end ?? start);
      } catch {}
    }
  }
  let unkeyedIdx = 0;
  document.querySelectorAll<HTMLDetailsElement>("details").forEach((el) => {
    const k = el.dataset.detailsKey;
    if (k) {
      if (openKeys.has(k)) el.open = true;
    } else {
      if (openUnkeyed[unkeyedIdx]) el.open = true;
      unkeyedIdx++;
    }
  });
  lastPollSignature = currentPollSignature();
}

function applyLanguage(next: Lang) {
  setLang(next);
  syncSkipLink();
  lastSyncedBackendLang = next;
  const activeToast = document.querySelector<HTMLDivElement>("#toast");
  if (activeToast) {
    activeToast.className = "";
    activeToast.textContent = "";
  }
  if (state) {
    state.language = next;
    void invoke("set_preferences", {
      minimiseToTray: state.minimise_to_tray,
      theme: document.documentElement.dataset.theme,
      healthMode: state.health_mode,
      language: next,
      readOnlyGuard: state.read_only_guard ?? true,
    }).catch(() => {});
  }
  render();
}

async function loadDetails(force = false) {
  const d = disk();
  if (!d || !state) return;
  const key = `${d.id}:${state.updated_at}:${state.job?.status ?? ""}:${state.job?.finished_at ?? ""}`;
  if (!force && detailKey === key) return;
  detailKey = key;
  const seq = ++loadSequence;
  try {
    const [h, ev, b, s, df, st, ap, rec, fc, cp, tm] = await Promise.all([
      invoke<History[]>("get_history", { diskId: d.id }),
      invoke<Event[]>("get_events", { diskId: d.id }),
      invoke<Job[]>("get_jobs", { kind: "benchmark", diskId: d.id }),
      invoke<Job[]>("get_jobs", { kind: "surface", diskId: d.id }),
      invoke<Job[]>("get_jobs", { kind: "defrag", diskId: d.id }).catch(() => []),
      invoke<Job[]>("get_jobs", { kind: "smart_tier", diskId: d.id }).catch(() => []),
      invoke<Job[]>("get_jobs", { kind: "autopilot", diskId: d.id }).catch(() => []),
      invoke<Job[]>("get_jobs", { kind: "recovery", diskId: d.id }).catch(() => []),
      invoke<HealthForecast | null>("get_health_forecast", { diskId: d.id }).catch(() => null),
      invoke<SurfaceCheckpoint | null>("get_surface_checkpoint", { diskId: d.id }).catch(() => null),
      invoke<TierMoveRecord[]>("list_tier_moves").catch(() => state?.tier_moves ?? []),
    ]);
    if (seq !== loadSequence || selected !== d.id) return;
    history = h;
    events = ev;
    benchmarkHistory = b;
    surfaceHistory = s;
    defragHistory = df;
    smartTierHistory = st;
    autopilotHistory = ap;
    recoveryHistory = rec;
    healthForecast = fc;
    surfaceCheckpoint = cp;
    tierMoves = Array.isArray(tm) ? tm : [];
    render();
  } catch (error) {
    detailKey = "";
    toast(L(`Geçmiş okunamadı: ${String(error)}`, `Failed to load history: ${String(error)}`), true);
  }
}

async function poll() {
  if (polling) return;
  polling = true;
  try {
    state = await invoke<AppState>("get_state");
    connectionError = "";
    const backendLang =
      state.language === "en" ? "en" : state.language === "tr" ? "tr" : "";
    if (backendLang) {
      if (!lastSyncedBackendLang) {
        if (!hadStoredLang && backendLang !== getLang()) {
          setLang(backendLang);
          syncSkipLink();
        } else if (hadStoredLang && backendLang !== getLang()) {
          void invoke("set_preferences", {
            minimiseToTray: state.minimise_to_tray,
            theme: document.documentElement.dataset.theme,
            healthMode: state.health_mode,
            language: getLang(),
            readOnlyGuard: state.read_only_guard ?? true,
          }).catch(() => {});
        }
      } else if (backendLang !== lastSyncedBackendLang && backendLang !== getLang()) {
        setLang(backendLang);
        syncSkipLink();
      }
      lastSyncedBackendLang = backendLang;
    }
    if (!state.disks.some((d) => d.id === selected)) {
      selected = state.disks[0]?.id ?? "";
      detailKey = "";
      history = [];
      events = [];
      smartTierHistory = [];
      healthForecast = null;
      surfaceCheckpoint = null;
      tierAnalysis = null;
      viewedJob = null;
    }
    const d = disk();
    if (d && !d.volumes.some((v) => v.letter === letter))
      letter = d.volumes[0]?.letter ?? "";
    const p = state.performance.disks.find(
      (p) => p.number === d?.os_disk_number,
    );
    if (d && p) {
      const points = livePoints.get(d.id) ?? [];
      points.push({ read: p.read_bps, write: p.write_bps });
      livePoints.set(d.id, points.slice(-40));
    }
    const sig = currentPollSignature();
    // Preserve an open native select and modal while fresh data remains in memory,
    // and skip redundant full-DOM redraws when visible state has not changed.
    if (
      sig !== lastPollSignature &&
      document.activeElement?.tagName !== "SELECT" &&
      !dialog.open
    ) {
      render();
    }
    void maybeAutoCheckUpdates();
    await loadDetails();
  } catch (error) {
    connectionError = L(
      `Rust çekirdeğine erişilemedi: ${String(error)}`,
      `Failed to reach Rust core: ${String(error)}`,
    );
    if (state) {
      state.stale = true;
      state.performance = { disks: [], error: connectionError };
    }
    render();
  } finally {
    polling = false;
  }
}

async function maybeAutoCheckUpdates() {
  if (autoUpdateChecked || !state || state.source !== "live") return;
  if (state.auto_check_updates === false) return;
  autoUpdateChecked = true;
  try {
    const res = await invoke<typeof updateStatus>("check_updates");
    if (res?.status === "update_available") {
      updateStatus = res;
      updateBannerDismissed = false;
      render();
    }
  } catch {
    // Stay silent on offline/startup network failure.
  }
}

async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {}
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  } catch {
    return false;
  }
}

async function openRecoveryPreview(
  jobId: string,
  itemId: string,
  itemName: string,
  recoverable: boolean,
) {
  const lang = getLang();
  dialog.innerHTML = `<div class="preview-modal"><div class="preview-head"><div><span class="eyebrow">${L("SALT OKUNUR ÖNİZLEME", "READ-ONLY PREVIEW", lang)}</span><h2 id="confirm-title">${e(itemName)}</h2></div></div><p class="muted">${L("Önizleme hazırlanıyor…", "Loading preview…", lang)}</p></div>`;
  dialog.showModal();
  try {
    const res = await invoke<RecoveryPreview>("preview_recovered_item", {
      jobId,
      itemId,
    });
    const repairBanner =
      res.repaired || res.repair_note
        ? `<div class="preview-repair-banner">${icon("check")}<div><strong>${res.repaired ? L("Otomatik Onarıldı", "Auto-Repaired", lang) : L("Medya Durumu", "Media Status", lang)}</strong>${res.repair_note ? `<p>${e(res.repair_note)}</p>` : ""}</div></div>`
        : "";
    let body = "";
    if (res.kind === "image") {
      const dims =
        res.width && res.height ? ` · ${res.width}×${res.height}` : "";
      body = `${repairBanner}<div class="preview-image-wrap"><img class="preview-image" src="${e(res.data_url)}" alt="${e(itemName)}"></div><p class="preview-meta">${e(res.mime)}${dims} · ${fmtBytes(res.size_bytes)}</p>`;
    } else if (res.kind === "text") {
      const maskedCount = Number(res.secrets_masked ?? 0);
      const hasSecretMask = maskedCount > 0 && typeof res.raw_text === "string";
      const secretBanner = hasSecretMask
        ? `<div class="preview-secret-banner">${icon("lock")}<div><strong>${L(`${maskedCount} hassas satır (.env / API anahtarı) maskelendi`, `${maskedCount} sensitive line(s) (.env / API keys) masked`, lang)}</strong><p>${L("Kurtar dediğinizde orijinal içerik olduğu gibi kaydedilir.", "Restoring saves the original unmasked file bytes.", lang)}</p></div><button type="button" class="button compact quiet" id="toggle-secret-mask">${L("Gizli Değerleri Göster", "Reveal Secrets", lang)}</button></div>`
        : "";
      body = `${repairBanner}${secretBanner}<pre class="preview-text" id="preview-text-pre">${e(res.text)}</pre><p class="preview-meta">${fmtBytes(res.size_bytes)}${res.truncated ? ` · ${L("İlk 16 KiB gösteriliyor", "Showing first 16 KiB", lang)}` : ""}</p>`;
    } else if (res.kind === "binary") {
      const m = res.metadata ?? {};
      body = `<dl class="preview-kv">
        <dt>${L("Ürün Adı", "Product Name", lang)}</dt><dd>${e(m.product_name || "—")}</dd>
        <dt>${L("Yayıncı", "Publisher", lang)}</dt><dd>${e(m.publisher || "—")}</dd>
        <dt>${L("Dosya Türü", "File Type", lang)}</dt><dd>${e(m.sub_type || L("PE / Çalıştırılabilir", "PE / Executable", lang))}</dd>
        <dt>${L("Boyut", "Size", lang)}</dt><dd>${m.size_bytes != null ? fmtBytes(m.size_bytes) : "—"}</dd>
        <dt>${L("Kaynak", "Source", lang)}</dt><dd>${e(m.source || "—")}</dd>
        <dt>${L("Durum", "Status", lang)}</dt><dd>${e(m.status_label || "—")}</dd>
      </dl>`;
    } else {
      body = `${repairBanner}<div class="preview-unavailable">${icon("info")}<div><strong>${L("Önizleme kullanılamıyor", "Preview unavailable", lang)}</strong><p>${e(res.reason)}</p></div></div>`;
    }
    const kindBadge =
      res.kind === "image"
        ? L("Görsel", "Image", lang)
        : res.kind === "text"
          ? L("Metin", "Text", lang)
          : res.kind === "binary"
            ? L("PE / İkili", "PE / Binary", lang)
            : L("Bilgi", "Info", lang);
    const canRestore = state?.source === "live" && recoverable && !pending;
    dialog.innerHTML = `<div class="preview-modal">
      <div class="preview-head"><div><span class="eyebrow">${L("SALT OKUNUR ÖNİZLEME", "READ-ONLY PREVIEW", lang)}</span><h2 id="confirm-title">${e(itemName)}</h2></div>${pill(kindBadge, res.kind === "unavailable" ? "warning" : "ok")}</div>
      ${body}
      <div class="dialog-actions">
        ${canRestore ? `<button type="button" class="button primary" id="preview-restore">${icon("recover")}<span>${L("Dosyayı Kurtar", "Restore File", lang)}</span></button>` : ""}
        <button type="button" class="button quiet" id="close-result">${L("Kapat", "Close", lang)}</button>
      </div>
    </div>`;
    dialog
      .querySelector("#close-result")!
      .addEventListener("click", () => dialog.close());
    if (res.kind === "text" && res.secrets_masked && typeof res.raw_text === "string") {
      let revealed = false;
      const preEl = dialog.querySelector<HTMLPreElement>("#preview-text-pre");
      const toggleBtn = dialog.querySelector<HTMLButtonElement>("#toggle-secret-mask");
      toggleBtn?.addEventListener("click", () => {
        revealed = !revealed;
        if (preEl) preEl.textContent = revealed ? res.raw_text! : res.text;
        toggleBtn.textContent = revealed
          ? L("Gizli Değerleri Maskele", "Mask Secrets", lang)
          : L("Gizli Değerleri Göster", "Reveal Secrets", lang);
      });
    }
    dialog.querySelector("#preview-restore")?.addEventListener("click", () => {
      dialog.close();
      void run(async () => {
        const out = await invoke<{
          saved: boolean;
          path?: string;
          repaired?: boolean;
        }>("restore_recovered_item", { jobId, itemId });
        if (out.saved) {
          toast(
            L(
              `Dosya kurtarıldı${out.repaired ? " (otomatik onarıldı)" : ""}: ${out.path ?? ""}`,
              `File restored${out.repaired ? " (auto-repaired)" : ""}: ${out.path ?? ""}`,
            ),
          );
        }
      });
    });
  } catch (err) {
    dialog.innerHTML = `<div class="preview-modal">
      <div class="preview-head"><h2 id="confirm-title">${e(itemName)}</h2>${pill(L("Hata", "Error", lang), "critical")}</div>
      <p class="error-text">${e(String(err))}</p>
      <div class="dialog-actions"><button type="button" class="button primary" id="close-result">${L("Kapat", "Close", lang)}</button></div>
    </div>`;
    dialog
      .querySelector("#close-result")!
      .addEventListener("click", () => dialog.close());
  }
}

interface DryRunSpec {
  mode: "read_only" | "mutating" | "system";
  target: string;
  apis: string;
  writeBudget?: string;
  rollback: string;
  confirmationToken?: string;
  steps?: string[];
}

function renderDryRunBox(dryRun?: DryRunSpec): string {
  if (!dryRun) return "";
  const isReadOnly = dryRun.mode === "read_only";
  const badge = isReadOnly
    ? pill(L("Salt-Okunur · 0 Yazma", "Read-Only · 0 Writes"), "ok")
    : pill(L("Etki Önizlemesi (Dry-Run)", "Dry-Run Impact Preview"), "warning");
  return `<div class="dry-run-box" role="region" aria-label="${L("İşlem etki önizlemesi", "Operation dry-run preview")}">
    <div class="dry-run-head">
      <span class="eyebrow">${L("YÜRÜTME PLANI", "EXECUTION MANIFEST")}</span>
      ${badge}
    </div>
    <dl class="dry-run-kv">
      <dt>${L("Hedef", "Target")}</dt><dd class="mono">${e(dryRun.target)}</dd>
      <dt>${L("API / Çağrı", "API / Path")}</dt><dd class="mono">${e(dryRun.apis)}</dd>
      ${dryRun.writeBudget ? `<dt>${L("Yazma Bütçesi", "Write Scope")}</dt><dd>${e(dryRun.writeBudget)}</dd>` : ""}
      <dt>${L("Geri Alma", "Rollback")}</dt><dd>${e(dryRun.rollback)}</dd>
      ${dryRun.confirmationToken ? `<dt>${L("IPC Token", "IPC Token")}</dt><dd><code class="mono">${e(dryRun.confirmationToken)}</code></dd>` : ""}
    </dl>
    ${
      dryRun.steps?.length
        ? `<ul class="dry-run-steps">${dryRun.steps.map((s) => `<li>${e(s)}</li>`).join("")}</ul>`
        : ""
    }
  </div>`;
}

function confirm(
  title: string,
  body: string,
  action: () => Promise<void>,
  dryRun?: DryRunSpec,
) {
  dialog.innerHTML = `<div class="dialog-icon">${icon(dryRun?.mode === "read_only" ? "shield" : "alert")}</div><h2 id="confirm-title">${e(title)}</h2><p>${e(body)}</p>${renderDryRunBox(dryRun)}<div class="dialog-actions"><button class="button quiet" id="confirm-cancel">${L("Vazgeç", "Cancel")}</button><button class="button primary" id="confirm-run">${L("Onayla ve başlat", "Confirm &amp; run")} ${icon("arrow")}</button></div>`;
  dialog.showModal();
  document
    .querySelector("#confirm-cancel")!
    .addEventListener("click", () => dialog.close());
  document.querySelector("#confirm-run")!.addEventListener("click", () => {
    dialog.close();
    void run(action);
  });
}

function toggleWriteGuard(unlock: boolean) {
  if (!state) return;
  const persistGuard = async (readOnlyGuard: boolean) => {
    await invoke("set_preferences", {
      minimiseToTray: state!.minimise_to_tray,
      theme: document.documentElement.dataset.theme,
      healthMode: state!.health_mode,
      language: getLang(),
      readOnlyGuard,
    });
    state!.read_only_guard = readOnlyGuard;
    toast(
      readOnlyGuard
        ? L(
            "Salt-okunur koruma kilidi yeniden etkinleştirildi.",
            "Read-Only Guard re-enabled.",
          )
        : L(
            "Yazma modu açıldı. İşleminiz bittiğinde kilidi kapatabilirsiniz.",
            "Write mode unlocked. Re-lock Read-Only Guard when finished.",
          ),
    );
  };
  if (!unlock) {
    void run(() => persistGuard(true));
    return;
  }
  confirm(
    L("Yazma modu kilidi açılsın mı?", "Unlock Write Mode?"),
    L(
      "SSD Sentinel varsayılan olarak diske yazan işlemleri (hız testi, yüzey onarımı, sistem ayarları ve NTFS katmanlama) Rust çekirdeğinde engeller. Kilidi açtığınızda bile her işlem ayrı etki önizlemesi ve onay isteyecektir.",
      "SSD Sentinel defaults to Read-Only Guard, blocking all mutating operations inside the Rust core. Even when unlocked, every mutating action still requires a dry-run preview and explicit confirmation.",
    ),
    () => persistGuard(false),
    {
      mode: "system",
      target: L("Tüm canlı diskler ve politika kontrolleri", "All live drives & tuning controls"),
      apis: "Core::read_only_guard + preferences.json",
      writeBudget: L(
        "Yalnız tercih bayrağı güncellenir; diske henüz veri yazılmaz",
        "Updates guard flag only; zero disk writes occur now",
      ),
      rollback: L(
        "Üst çubuktaki veya Ayarlar'daki anahtarla tek tıkla Salt-Okunur moda dönülebilir",
        "Re-lock Read-Only Guard in one click from the top bar or Settings",
      ),
    },
  );
}

async function run(fn: () => Promise<void>) {
  if (pending) return;
  pending = true;
  render();
  try {
    await fn();
    await poll();
  } catch (error) {
    toast(String(error), true);
  } finally {
    pending = false;
    render();
  }
}

function applyBenchPreset(preset: "quick" | "full") {
  const d = disk();
  const isNvme = /nvme/i.test(`${d?.type ?? ""} ${d?.interface ?? ""}`);
  benchPreset = preset;
  if (preset === "quick") {
    Object.assign(bench, {
      profile: "standard",
      size_mib: 256,
      repeats: 1,
      mode: "read",
      seconds: 5,
      interval: 0,
      data: "random",
      row: "all",
    });
  } else {
    Object.assign(bench, {
      profile: isNvme ? "nvme" : "standard",
      size_mib: 1024,
      repeats: 3,
      mode: "both",
      seconds: 10,
      interval: 0,
      data: "random",
      row: "all",
    });
  }
  render();
}

function start(kind: "benchmark" | "surface", row = "all", resume = false) {
  const d = disk();
  if (!d || !canStart(state, d)) return;
  if (kind === "surface" && resume) {
    scan.mode = "physical_scan";
  }
  const activeMode = resume ? "physical_scan" : scan.mode;
  const isReadOnlySurface =
    kind === "surface" && ["read_scan", "physical_scan"].includes(activeMode);
  if ((kind === "benchmark" || (kind === "surface" && !isReadOnlySurface)) && isWriteLocked(state)) {
    toast(writeDisabledReason(d), true);
    return;
  }
  const opts =
    kind === "benchmark"
      ? { ...bench, row }
      : resume
        ? { ...scan, mode: "physical_scan", resume: true }
        : { ...scan };
  const selectedLetter = letter;
  const cpInfo = resume && surfaceCheckpoint ? checkpointProgress(surfaceCheckpoint) : null;
  const warning =
    kind === "benchmark"
      ? L(
          `${d.model} · ${selectedLetter}: üzerinde ${bench.size_mib} MiB geçici dosya hazırlanacak. Salt okuma seçimi bile dosya hazırlığında yazma yapar. Süre tabanlı testte toplam yazma, dosya boyutundan çok daha fazla olabilir. Kullanıcı dosyaları değiştirilmez.`,
          `A temporary ${bench.size_mib} MiB benchmark file will be allocated on ${d.model} (${selectedLetter}:). Duration-based write runs may write more than the file size. Existing user files are untouched.`,
        )
      : resume
        ? L(
            `${d.model} · ${selectedLetter}: için kaydedilen tam yüzey taraması kaldığı yerden (%${cpInfo?.percent ?? 0}) devam ettirilecek. Diskin kalan bölgeleri ham LBA olarak salt okunur taranır; istediğiniz an durdurabilirsiniz.`,
            `Resumes the saved full surface scan on ${d.model} (${selectedLetter}:) from ${cpInfo?.percent ?? 0}%. Remaining LBAs are scanned read-only; stop anytime.`,
          )
        : scan.mode === "data_refresh"
          ? L(
              `${d.model} · ${selectedLetter}: bölümünde ${autoSurfaceMib(scan.mode, d)} MiB geçici alana yazılıp geri okunacak ve karşılaştırılacak. Bu işlem eski kullanıcı verilerini yenilemez ve fiziksel sektör onarımı yapmaz.`,
              `Writes and verifies ${autoSurfaceMib(scan.mode, d)} MiB temporary data on ${d.model} (${selectedLetter}:). Does not modify user files or perform physical sector repair.`,
            )
          : scan.mode === "targeted_repair"
            ? L(
                `${d.model} · ${selectedLetter}: için NTFS çevrimiçi chkdsk /scan ve sınırlı dosya örneklemesi yapılacak. Windows taraması dosya sistemi etkinliğine ve yük artışına yol açabilir. Güncel yedeğinizi doğrulayın.`,
                `Runs online NTFS chkdsk /scan and sampled file verification on ${d.model} (${selectedLetter}:). Ensure backups are up to date.`,
              )
            : scan.mode === "physical_scan"
              ? L(
                  `${d.model} · ${selectedLetter}: sürücüsünün tamamı ham LBA olarak baştan sona okunacak; yavaş veya bozuk parçalar 4K sektör sektör incelenip dosyalarla eşlenecek. ${fullScanEstimate(d)}`,
                  `Reads ${d.model} (${selectedLetter}:) sequentially from start to end via raw LBA; maps slow or unreadable chunks to 4K sectors and files. ${fullScanEstimate(d)}`,
                )
              : scan.mode === "magnetic_refresh"
                ? d.type.includes("HDD")
                  ? L(
                      `${d.model} · ${selectedLetter}: HDD'sinde son tam taramanın bulduğu bozuk ve zayıf sektörlerdeki kullanıcı dosyaları önce güvenli boş alana taşınacak (yerine yazma yapılmaz), ardından sektörler yerinde tazelenip yeniden ölçülecek ve düzelmeyen ölü alanlar kapatılacak. Güncel yedeğinizi doğrulayın.`,
                      `Moves user files off weak/bad sectors identified in the latest full scan on ${d.model} (${selectedLetter}:), safely refreshes magnetic tracks, verifies latency, and isolates severe defects. Verify backups first.`,
                    )
                  : L(
                      `${d.model} · ${selectedLetter}: SSD'sinde son tam taramanın bulduğu yavaş okunan eski veri bölgelerindeki dosyalar FSCTL_MOVE_FILE ile taze NAND hücrelerine taşınacak; okuma hızı önce/sonra doğrulanacak.`,
                      `Relocates files in slow-reading stale regions on ${d.model} (${selectedLetter}:) to fresh NAND cells via FSCTL_MOVE_FILE and verifies read latency before vs after.`,
                    )
                : scan.mode === "bad_zone_isolate"
                  ? L(
                      `${d.model} · ${selectedLetter}: NTFS bölümünde chkdsk /scan sonrası tam taramanın bulduğu hasarlı bölgelerdeki dosyalar güvenli dış izlere taşınacak ve ölü alanlar .ssd-sentinel-quarantine kasasına kalıcı olarak kilitlenecek.`,
                      `Moves files off damaged regions detected by the full scan on ${d.model} (${selectedLetter}:) and permanently locks bad clusters under .ssd-sentinel-quarantine.`,
                    )
                  : L(
                      `${d.model} · ${selectedLetter}: içindeki erişilebilir dosyalardan en fazla ${autoSurfaceMib(scan.mode, d)} MiB örnek okunacak. Tam fiziksel sektör taraması değildir. Önbellek ölçümü etkileyebilir.`,
                      `Reads up to ${autoSurfaceMib(scan.mode, d)} MiB of accessible files on ${d.model} (${selectedLetter}:). Not a full raw sector scan.`,
                    );
  const confirmationToken =
    kind === "benchmark"
      ? `benchmark:${d.id}:${selectedLetter}`
      : `surface:${d.id}:${selectedLetter}:${activeMode}`;
  const dryRun: DryRunSpec =
    kind === "benchmark"
      ? {
          mode: "mutating",
          target: `${d.model} (${selectedLetter}:)`,
          apis: "Win32 IOCP Direct-I/O (FILE_FLAG_NO_BUFFERING | OVERLAPPED)",
          writeBudget: `${bench.size_mib} MiB · ${bench.mode} · ${bench.repeats}×${bench.seconds}s`,
          rollback: L(
            "Geçici dosya iş bitiminde veya Durdur basıldığında otomatik silinir",
            "Temporary payload deleted automatically on completion or stop",
          ),
          confirmationToken,
        }
      : {
          mode: isReadOnlySurface ? "read_only" : "mutating",
          target:
            activeMode === "physical_scan"
              ? `\\\\.\\PhysicalDrive${d.os_disk_number ?? "?"} (${selectedLetter}:)`
              : `${d.model} (${selectedLetter}:)`,
          apis:
            activeMode === "physical_scan"
              ? "CreateFileW(GENERIC_READ | FILE_FLAG_NO_BUFFERING) + 4K Probe + Checkpoint"
              : activeMode === "read_scan"
                ? "ReadFile (salt-okunur dosya örnekleme)"
                : activeMode === "magnetic_refresh"
                  ? "FSCTL_GET_RETRIEVAL_POINTERS + FSCTL_MOVE_FILE + Raw Sector Verify"
                  : activeMode === "bad_zone_isolate"
                    ? "chkdsk.exe /scan + FSCTL_MOVE_FILE + .ssd-sentinel-quarantine"
                    : activeMode === "targeted_repair"
                      ? "chkdsk.exe /scan + ReadFile"
                      : "WriteFile / ReadFile + SHA-256",
          writeBudget: isReadOnlySurface
            ? L("0 B · diske yazma yapılmaz", "0 B · strictly read-only")
            : L(
                `En fazla ${autoSurfaceMib(activeMode, d)} MiB (termal/SMART koruma etkin)`,
                `Up to ${autoSurfaceMib(activeMode, d)} MiB (thermal & SMART guard active)`,
              ),
          rollback: isReadOnlySurface
            ? L(
                "Gerekmez (salt-okunur); tarama noktası SQLite'da saklanır",
                "Not needed (read-only); resume checkpoint stored in SQLite",
              )
            : activeMode === "bad_zone_isolate"
              ? L(
                  "Karantina dosyaları Bakım/Optimizasyon bölümünden kaldırılabilir",
                  "Quarantine markers can be released from the Optimize view",
                )
              : L(
                  "Kullanıcı dosyaları yazmadan önce boş alana taşınır",
                  "User files are evacuated via FSCTL_MOVE_FILE before sector refresh",
                ),
          confirmationToken,
        };
  confirm(
    kind === "benchmark"
      ? L("Hız testini başlat?", "Start benchmark?")
      : resume
        ? L("Taramaya kaldığın yerden devam et?", "Resume surface scan?")
        : L("Disk doğrulamasını başlat?", "Start surface verification?"),
    warning,
    async () => {
      viewedJob = null;
      await invoke("start_job", {
        request: {
          kind,
          disk_id: d.id,
          letter: selectedLetter,
          confirmation: confirmationToken,
          options: opts,
        },
      });
      toast(
        resume
          ? L("Yüzey taraması kaldığı yerden devam ediyor.", "Surface scan resumed.")
          : L("İş başlatıldı. İlerlemeyi alttaki durum çubuğundan izleyebilirsiniz.", "Job started. Monitor progress in the bottom bar."),
      );
    },
    dryRun,
  );
}

function startDefrag(mode: "analyze" | "defrag" | "consolidate") {
  const d = disk();
  if (!d || !canStart(state, d)) return;
  if (mode !== "analyze" && !canMaintain(state, d)) return;
  const selectedLetter = letter;
  const label =
    mode === "analyze"
      ? L("Parçalanma analizi (/A)", "Fragmentation analysis (/A)")
      : mode === "consolidate"
        ? L("Boş alan birleştirme (/X)", "Free space consolidation (/X)")
        : L("Akıllı HDD birleştirme (/O)", "Smart HDD optimization (/O)");
  const confirmationToken = `defrag:${d.id}:${selectedLetter}:${mode}`;
  confirm(
    L(`${label} başlatılsın mı?`, `Start ${label}?`),
    L(
      `${d.model} · ${selectedLetter}: HDD bölümü üzerinde Windows defrag.exe (${label}) çalıştırılacak. Yalnız mekanik HDD'lerde desteklenir.`,
      `Runs Windows defrag.exe (${label}) on HDD volume ${d.model} (${selectedLetter}:). Mechanical HDDs only.`,
    ),
    async () => {
      viewedJob = null;
      await invoke("start_job", {
        request: {
          kind: "defrag",
          disk_id: d.id,
          letter: selectedLetter,
          confirmation: confirmationToken,
          options: { mode },
        },
      });
      toast(L("HDD birleştirme/analiz işi başlatıldı.", "HDD optimization job started."));
    },
    {
      mode: mode === "analyze" ? "read_only" : "mutating",
      target: `${d.model} (${selectedLetter}:)`,
      apis: `defrag.exe ${selectedLetter}: ${mode === "analyze" ? "/A" : mode === "consolidate" ? "/X" : "/O"}`,
      writeBudget:
        mode === "analyze"
          ? L("0 B · yalnızca okuma ve analiz", "0 B · read-only fragmentation analysis")
          : L("Windows defrag.exe blok taşıma (SSD'de kilitli)", "Windows defrag.exe cluster move (blocked on SSDs)"),
      rollback: L("İptal edilebilir; NTFS dosya bütünlüğü korunur", "Cancellable; NTFS consistency preserved"),
      confirmationToken,
    },
  );
}

async function analyzeStorageTiering() {
  const d = disk();
  if (!d || tierAnalyzing || tierTransferring || pending) return;
  const selectedLetter = letter || d.volumes[0]?.letter || "";
  if (state?.source === "live" && !selectedLetter) {
    toast(
      L("Analiz için seçili diskte NTFS bölüm eşlemesi gerekli.", "An NTFS volume mapping is required for tiering analysis."),
      true,
    );
    return;
  }
  const diskId = d.id;
  tierAnalyzing = true;
  render();
  try {
    const res = await invoke<TierAnalysis>("analyze_storage_tiering", {
      diskId,
      letter: selectedLetter,
    });
    if (selected === diskId) {
      tierAnalysis = res;
      if (Array.isArray(res.active_moves) && res.active_moves.length) {
        const merged = new Map<string, TierMoveRecord>();
        for (const m of [...res.active_moves, ...tierMoves]) {
          merged.set(m.id, m);
        }
        tierMoves = Array.from(merged.values());
      }
    }
    const translated = translateTierAnalysis(res, fmtBytes, getLang());
    toast(
      translated.summary ||
        L("Depolama katmanlama analizi tamamlandı.", "Storage tiering analysis completed."),
    );
  } catch (error) {
    toast(String(error), true);
  } finally {
    tierAnalyzing = false;
    render();
  }
}

async function analyzeDevWorkspace() {
  const d = disk();
  if (!d || devScanning || devCleaning || tierTransferring || pending) return;
  const selectedLetter = letter || d.volumes[0]?.letter || "";
  if (state?.source === "live" && !selectedLetter) {
    toast(
      L(
        "Geliştirici taraması için seçili diskte NTFS bölüm eşlemesi gerekli.",
        "An NTFS volume mapping is required for developer workspace analysis.",
      ),
      true,
    );
    return;
  }
  const diskId = d.id;
  devScanning = true;
  render();
  try {
    const res = await invoke<DevWorkspaceAnalysis>("analyze_dev_workspace", {
      diskId,
      letter: selectedLetter,
      customRoot: devCustomRoot || null,
    });
    if (selected === diskId) {
      devAnalysis = res;
    }
    const lang = getLang();
    toast(
      (lang === "en" ? res.summary_en : res.summary_tr) ||
        L("Geliştirici alan analizi tamamlandı.", "Developer workspace scan completed."),
    );
  } catch (error) {
    toast(String(error), true);
  } finally {
    devScanning = false;
    render();
  }
}

async function pickDevFolder() {
  if (devScanning || devCleaning || pending) return;
  try {
    const picked = await invoke<string | null>("pick_dev_folder");
    if (picked) {
      devCustomRoot = picked;
      render();
      await analyzeDevWorkspace();
    }
  } catch (error) {
    toast(String(error), true);
  }
}

function cleanDevArtifact(
  artifactPath: string,
  confirmationToken: string,
  label: string,
  sizeBytes: number,
) {
  const d = disk();
  if (!d || !canWrite(state) || !canStart(state, d) || devCleaning || tierTransferring) {
    return;
  }
  const diskId = d.id;
  confirm(
    L(`"${label}" güvenle temizlensin mi?`, `Safely clean "${label}"?`),
    L(
      `${artifactPath} (${fmtBytes(sizeBytes)}) yalnızca yeniden üretilebilir derleme / önbellek dosyalarını içerir. Kaynak kod, .git ve .env dosyalarına dokunulmaz.`,
      `${artifactPath} (${fmtBytes(sizeBytes)}) contains only rebuildable artifacts/caches. Source code, .git, and .env files remain untouched.`,
    ),
    async () => {
      devCleaning = true;
      render();
      try {
        const res = await invoke<{
          freed_bytes?: number;
          message_tr?: string;
          message?: string;
        }>("clean_dev_artifact", {
          diskId,
          artifactPath,
          confirmation: confirmationToken,
        });
        if (devAnalysis && devAnalysis.disk_id === diskId) {
          const normTarget = artifactPath.toLowerCase();
          const projects = devAnalysis.projects.map((p) => {
            const artifacts = p.artifacts.filter(
              (a) => a.path.toLowerCase() !== normTarget,
            );
            return {
              ...p,
              artifacts,
              artifact_bytes: artifacts.reduce((s, a) => s + a.size_bytes, 0),
            };
          });
          const global_caches = devAnalysis.global_caches.filter(
            (c) => c.path.toLowerCase() !== normTarget,
          );
          const total_artifact_bytes = projects.reduce(
            (s, p) => s + p.artifact_bytes,
            0,
          );
          const stale_artifact_bytes = projects
            .filter((p) => p.stale)
            .reduce((s, p) => s + p.artifact_bytes, 0);
          const total_cache_bytes = global_caches
            .filter((c) => !c.is_junction)
            .reduce((s, c) => s + c.size_bytes, 0);
          const reclaimable_cache = global_caches
            .filter((c) => c.can_clean)
            .reduce((s, c) => s + c.size_bytes, 0);
          devAnalysis = {
            ...devAnalysis,
            projects,
            global_caches,
            total_artifact_bytes,
            stale_artifact_bytes,
            total_cache_bytes,
            total_reclaimable_bytes: stale_artifact_bytes + reclaimable_cache,
          };
        }
        const msg =
          getLang() === "en"
            ? res?.message ?? `${label} cleaned.`
            : res?.message_tr ?? res?.message ?? `${label} temizlendi.`;
        toast(msg);
      } finally {
        devCleaning = false;
      }
    },
    {
      mode: "mutating",
      target: artifactPath,
      apis: "Dev Allowlist Guard + Recursive Unlink (No Junction Traversal)",
      writeBudget: `${fmtBytes(sizeBytes)} · ${L("Kaynak kod ve .env korunur", "Source code & .env protected")}`,
      rollback: L(
        "Silinen derleme önbelleği npm / cargo / pip / uv ile yeniden üretilebilir",
        "Deleted cache/artifacts can be regenerated by npm / cargo / pip / uv",
      ),
      confirmationToken,
    },
  );
}

function offloadDevCache(cacheId: string) {
  const d = disk();
  if (!d || !devAnalysis || devAnalysis.disk_id !== d.id || tierTransferring || devCleaning) {
    return;
  }
  const target = devAnalysis.offload_target;
  const cache = devAnalysis.global_caches.find((c) => c.id === cacheId);
  if (!cache || !target || !canMaintain(state, d) || !canStart(state, d)) return;
  const diskId = d.id;
  const targetLetter = target.letter;
  const direction = target.direction || "to_archive";
  const label = getLang() === "en" ? cache.name_en : cache.name;
  const confirmationToken = `tier_relocate:${diskId}:${targetLetter}`;
  confirm(
    L(
      `"${label}" ${targetLetter}: sürücüsüne Junction ile taşınsın mı?`,
      `Offload "${label}" to ${targetLetter}: via NTFS Junction?`,
    ),
    L(
      `${cache.path} (${fmtBytes(cache.size_bytes)}) → ${targetLetter}: sürücüsüne SHA-256 doğrulaması ile kopyalanacak ve orijinal yolda şeffaf NTFS Junction bırakılacak. Ollama / Cargo / Hugging Face yolları bozulmaz ve tek tıkla geri alınabilir.`,
      `${cache.path} (${fmtBytes(cache.size_bytes)}) will be SHA-256 verified and moved to ${targetLetter}: with a transparent NTFS Junction at the original path. Ollama / Cargo / Hugging Face continue working seamlessly.`,
    ),
    async () => {
      tierTransferring = true;
      render();
      try {
        const rec = await invoke<TierMoveRecord>("relocate_tier_item", {
          diskId,
          sourcePath: cache.path,
          targetLetter,
          direction,
          confirmation: confirmationToken,
        });
        tierMoves = [rec, ...tierMoves.filter((m) => m.id !== rec.id)];
        if (devAnalysis && devAnalysis.disk_id === diskId) {
          const global_caches = devAnalysis.global_caches.map((c) =>
            c.id === cache.id
              ? { ...c, is_junction: true, can_offload: false, can_clean: false }
              : c,
          );
          devAnalysis = {
            ...devAnalysis,
            global_caches,
            total_cache_bytes: global_caches
              .filter((c) => !c.is_junction)
              .reduce((s, c) => s + c.size_bytes, 0),
          };
        }
        detailKey = "";
        toast(
          L(
            `"${label}" ${targetLetter}: sürücüsüne taşındı (NTFS Junction aktif).`,
            `"${label}" offloaded to ${targetLetter}: (NTFS Junction active).`,
          ),
        );
      } finally {
        tierTransferring = false;
      }
    },
    {
      mode: "mutating",
      target: `${cache.path} → ${targetLetter}:\\SSD-Sentinel-Arsiv`,
      apis: "SHA-256 Chunked Copy + Rename + FSCTL_SET_REPARSE_POINT (NTFS Junction)",
      writeBudget: `${fmtBytes(cache.size_bytes)} · ≥10 GiB ${L("hedef boş alan rezervi", "target free reserve")}`,
      rollback: L(
        "Aktif Taşınmış Öğeler listesinden tek tıkla SHA-256 ile geri alınır",
        "One-click SHA-256 verified rollback from Active Relocated Items",
      ),
      confirmationToken,
    },
  );
}

function startSmartTier(mode: "ssd_extent_optimize" | "hdd_outer_optimize") {
  const d = disk();
  if (!d || !canMaintain(state, d) || !canStart(state, d) || tierTransferring) return;
  const selectedLetter = letter || d.volumes[0]?.letter || "";
  if (!selectedLetter) return;
  const isSsdOpt = mode === "ssd_extent_optimize";
  const confirmationToken = `smart_tier:${d.id}:${selectedLetter}:${mode}`;
  confirm(
    isSsdOpt
      ? L("SSD Akıllı Parça Temizliği + ReTrim başlatılsın mı?", "Start SSD Smart Extent Consolidation + ReTrim?")
      : L("HDD Dış-İz Hızlı Yerleşim başlatılsın mı?", "Start HDD Outer-Track Placement?"),
    isSsdOpt
      ? L(
          `${d.model} · ${selectedLetter}: SSD bölümünde klasik defrag yapılmaz; yalnızca aşırı parçalanmış (>50 parça) kullanıcı dosyaları en fazla 2 GiB yazma bütçesiyle toparlanır ve ardından Windows ReTrim (/L) uygulanır.`,
          `Avoids traditional defrag on ${d.model} (${selectedLetter}:); consolidates only heavily fragmented (>50 extents) files within a 2 GiB write cap, then runs ReTrim (/L).`,
        )
      : L(
          `${d.model} · ${selectedLetter}: HDD bölümünde önce SMART sektör güvenliği doğrulanır; ardından sık kullanılan parçalanmış dosyalar en hızlı dış ZBR izlerine (en fazla 4 GiB bütçe) yerleştirilir.`,
          `Checks SMART safety first on ${d.model} (${selectedLetter}:), then places hot fragmented files onto faster outer ZBR tracks within a 4 GiB cap.`,
        ),
    async () => {
      viewedJob = null;
      await invoke("start_job", {
        request: {
          kind: "smart_tier",
          disk_id: d.id,
          letter: selectedLetter,
          confirmation: confirmationToken,
          options: { mode },
        },
      });
      toast(
        isSsdOpt
          ? L("SSD Akıllı Parça Temizliği + ReTrim başlatıldı.", "SSD Extent Consolidation + ReTrim started.")
          : L("HDD Dış-İz Hızlı Yerleşim başlatıldı.", "HDD Outer-Track Placement started."),
      );
    },
    {
      mode: "mutating",
      target: `${d.model} (${selectedLetter}:)`,
      apis: isSsdOpt
        ? "FSCTL_GET_RETRIEVAL_POINTERS + FSCTL_MOVE_FILE + defrag.exe /L"
        : "SMART Guard + FSCTL_GET_VOLUME_BITMAP + FSCTL_MOVE_FILE",
      writeBudget: isSsdOpt
        ? L("Maks. 2 GiB (>50 parça dosyalar); klasik defrag yok", "Max 2 GiB (>50 extent files); no full defrag")
        : L("Maks. 4 GiB (sıcak parçalı dosyalar)", "Max 4 GiB (hot fragmented files)"),
      rollback: L(
        "İptal edilebilir; yalnızca NTFS küme yerleşimi iyileştirilir",
        "Cancellable anytime; only optimizes NTFS extent layout",
      ),
      confirmationToken,
    },
  );
}

function relocateTierCandidate(candidateId: string) {
  const d = disk();
  if (!d || !tierAnalysis || tierAnalysis.disk_id !== d.id || tierTransferring) return;
  const companion = tierAnalysis.companion;
  const cand = tierAnalysis.candidates.find((c) => c.id === candidateId);
  if (!cand || !companion || !canMaintain(state, d) || !canStart(state, d)) {
    return;
  }
  const diskId = d.id;
  const targetLetter = companion.letter;
  const direction =
    cand.recommended_direction || (d.type.includes("HDD") ? "to_ssd" : "to_hdd");
  const isPromote = direction === "to_ssd";
  const confirmationToken = `tier_relocate:${diskId}:${targetLetter}`;
  confirm(
    isPromote
      ? L(`"${cand.name}" SSD'ye (${targetLetter}:) terfi ettirilsin mi?`, `Promote "${cand.name}" to SSD (${targetLetter}:)?`)
      : L(`"${cand.name}" HDD'ye (${targetLetter}:) taşınsın mı?`, `Offload "${cand.name}" to HDD (${targetLetter}:)?`),
    L(
      `${cand.path} (${fmtBytes(cand.size_bytes)}) → ${targetLetter}: sürücüsüne SHA-256 doğrulaması ile kopyalanacak.${cand.is_dir ? " Orijinal konumda şeffaf NTFS Directory Junction oluşturulacağı için kısayol, oyun ve program yolları bozulmaz." : " Doğrulama tamamlanmadan kaynak dosya silinmez."} İstediğiniz zaman tek tıkla geri alabilirsiniz.`,
      `${cand.path} (${fmtBytes(cand.size_bytes)}) will be SHA-256 verified and moved to ${targetLetter}:.${cand.is_dir ? " An NTFS Directory Junction keeps original paths and shortcuts working." : " Source is removed only after SHA-256 verification."} You can revert anytime in one click.`,
    ),
    async () => {
      tierTransferring = true;
      render();
      try {
        const rec = await invoke<TierMoveRecord>("relocate_tier_item", {
          diskId,
          sourcePath: cand.path,
          targetLetter,
          direction,
          confirmation: confirmationToken,
        });
        tierMoves = [rec, ...tierMoves.filter((m) => m.id !== rec.id)];
        if (tierAnalysis && tierAnalysis.disk_id === diskId) {
          tierAnalysis = {
            ...tierAnalysis,
            candidates: tierAnalysis.candidates.filter((c) => c.id !== cand.id),
            total_reclaimable_bytes: Math.max(
              0,
              (tierAnalysis.total_reclaimable_bytes ?? 0) - cand.size_bytes,
            ),
            active_moves: [
              rec,
              ...(tierAnalysis.active_moves ?? []).filter((m) => m.id !== rec.id),
            ],
          };
        }
        detailKey = "";
        const msg = rec.junction_created
          ? L(
              `"${cand.name}" ${targetLetter}: sürücüsüne taşındı (NTFS Junction aktif · kısayollar korundu).`,
              `"${cand.name}" moved to ${targetLetter}: (NTFS Junction active · paths preserved).`,
            )
          : L(
              `"${cand.name}" SHA-256 doğrulamasıyla ${targetLetter}: sürücüsüne taşındı.`,
              `"${cand.name}" moved to ${targetLetter}: with SHA-256 verification.`,
            );
        toast(rec.cleanup_warning ? `${msg} (${rec.cleanup_warning})` : msg);
      } finally {
        tierTransferring = false;
      }
    },
    {
      mode: "mutating",
      target: `${cand.path} → ${targetLetter}:\\SSD-Sentinel-Arsiv`,
      apis: cand.is_dir
        ? "SHA-256 Chunked Copy + Rename + FSCTL_SET_REPARSE_POINT (NTFS Junction)"
        : "SHA-256 Chunked Copy + Verified Atomic Swap",
      writeBudget: `${fmtBytes(cand.size_bytes)} · ≥10 GiB ${L("hedef boş alan rezervi", "target free reserve")}`,
      rollback: L(
        "Aktif Taşınmış Öğeler listesinden tek tıkla SHA-256 ile geri alınır",
        "One-click SHA-256 verified rollback from Active Relocated Items",
      ),
      confirmationToken,
    },
  );
}

function rollbackTierMove(moveId: string) {
  const rec = mergedTierMoves(tierAnalysis).find((m) => m.id === moveId);
  if (!rec || !canOptimizeSystem(state) || tierTransferring) return;
  const isFromSsd = rec.direction === "to_ssd";
  const confirmationToken = `tier_rollback:${rec.id}`;
  confirm(
    isFromSsd
      ? L("Öğe HDD'ye geri alınsın mı?", "Restore item back to HDD?")
      : L("Öğe SSD'ye geri alınsın mı?", "Restore item back to SSD?"),
    L(
      `${rec.target_path} (${fmtBytes(rec.size_bytes)}) SHA-256 doğrulaması ile orijinal konumuna (${rec.source_path}) geri taşınacak${rec.junction_created ? " ve geçici NTFS Junction güvenli biçimde kaldırılacak" : ""}.`,
      `${rec.target_path} (${fmtBytes(rec.size_bytes)}) will be SHA-256 verified and restored to ${rec.source_path}${rec.junction_created ? " with the NTFS Junction removed cleanly" : ""}.`,
    ),
    async () => {
      tierTransferring = true;
      render();
      try {
        const res = await invoke<{
          message_tr?: string;
          message?: string;
          cleanup_warning?: string | null;
        }>("rollback_tier_move", {
          moveId: rec.id,
          confirmation: confirmationToken,
        });
        tierMoves = tierMoves.filter((m) => m.id !== rec.id);
        if (state?.tier_moves) {
          state.tier_moves = state.tier_moves.filter((m) => m.id !== rec.id);
        }
        if (tierAnalysis) {
          tierAnalysis = {
            ...tierAnalysis,
            active_moves: (tierAnalysis.active_moves ?? []).filter(
              (m) => m.id !== rec.id,
            ),
          };
        }
        detailKey = "";
        const baseMsg = L(
          res?.message_tr ?? "Taşınan öğe orijinal konumuna geri alındı.",
          res?.message ?? "Relocated item restored to its original path.",
        );
        toast(res?.cleanup_warning ? `${baseMsg} (${res.cleanup_warning})` : baseMsg);
      } finally {
        tierTransferring = false;
      }
    },
    {
      mode: "mutating",
      target: `${rec.target_path} → ${rec.source_path}`,
      apis: rec.junction_created
        ? "SHA-256 Chunked Restore + Junction Unlink + Directory Swap"
        : "SHA-256 Chunked Restore",
      writeBudget: `${fmtBytes(rec.size_bytes)}`,
      rollback: L(
        "Hedef kopya ancak kaynak bütünlüğü SHA-256 ile doğrulandıktan sonra temizlenir",
        "Archived target is removed only after SHA-256 verification succeeds",
      ),
      confirmationToken,
    },
  );
}

function startAutopilot() {
  const d = disk();
  if (!d || !canMaintain(state, d) || !canStart(state, d)) return;
  const selectedLetter = letter;
  const isHdd = d.type === "HDD";
  const confirmationToken = `autopilot:${d.id}:${selectedLetter}`;
  confirm(
    isHdd
      ? L("HDD Politika Profili Uygulansın mı?", "Apply HDD Policy Profile?")
      : L("SSD Dayanım & Performans Profili Uygulansın mı?", "Apply SSD Endurance & Policy Profile?"),
    isHdd
      ? L(
          `${d.model} · ${selectedLetter}: üzerinde donanım yazma önbelleği + APM 254 + NTFS chkdsk /scan + akıllı defrag /O sırasıyla uygulanacak. Sistem ayarları SQLite geri alma listesine kaydedilir.`,
          `Applies hardware write caching + APM 254 + NTFS chkdsk /scan + smart defrag /O in order on ${d.model} (${selectedLetter}:). Settings are archived in SQLite for rollback.`,
        )
      : L(
          `${d.model} · ${selectedLetter}: üzerinde Windows TRIM + düşük gecikmeli PCIe ASPM + donanım önbelleği + ReTrim (/L) uygulanacak. Tüm değişiklikler geri alınabilir.`,
          `Applies Windows TRIM + low-latency PCIe ASPM + write cache + ReTrim (/L) on ${d.model} (${selectedLetter}:). All setting changes can be rolled back.`,
        ),
    async () => {
      viewedJob = null;
      await invoke("start_job", {
        request: {
          kind: "autopilot",
          disk_id: d.id,
          letter: selectedLetter,
          confirmation: confirmationToken,
          options: { profile: "auto" },
        },
      });
      toast(L("Depolama politikası profili başlatıldı.", "Storage policy profile started."));
    },
    {
      mode: "mutating",
      target: `${d.model} (${selectedLetter}:)`,
      apis: isHdd
        ? "IOCTL_DISK_SET_CACHE_INFORMATION + smartctl APM + chkdsk /scan + defrag /O"
        : "fsutil DisableDeleteNotify + powercfg ASPM + IOCTL Cache + defrag /L",
      rollback: L(
        "Önceki ayarlar SQLite optimizations tablosundan geri alınabilir",
        "Previous settings archived in SQLite optimizations table for rollback",
      ),
      confirmationToken,
      steps: isHdd
        ? [
            L("1. Donanım yazma önbelleğini etkinleştir", "1. Enable hardware write cache"),
            L("2. smartctl üzerinden APM 254 kafa park korumasını ayarla", "2. Set ATA APM 254 via smartctl"),
            L("3. chkdsk /scan ile çevrimiçi NTFS bütünlük kontrolü", "3. Online NTFS chkdsk /scan check"),
            L("4. SMART sağlıklıysa defrag /O uygula", "4. Run defrag /O if SMART is healthy"),
          ]
        : [
            L("1. Windows NTFS TRIM (DisableDeleteNotify=0) doğrula", "1. Enable Windows NTFS TRIM"),
            L("2. PCIe ASPM düşük gecikme profilini ayarla", "2. Configure low-latency PCIe ASPM"),
            L("3. Donanım yazma önbelleğini etkinleştir", "3. Enable controller write cache"),
            L("4. defrag /L ile SSD ReTrim uygula", "4. Run SSD ReTrim via defrag /L"),
          ],
    },
  );
}

function startRecovery() {
  const d = disk();
  if (!d || !canStart(state, d)) return;
  const selectedLetter = letter;
  const confirmationToken = `recovery:${d.id}:${selectedLetter}:${recConfig.mode}`;
  confirm(
    L("Kurtarma taraması başlatılsın mı?", "Start recovery scan?"),
    L(
      `${d.model} · ${selectedLetter}: üzerinde ${
        recConfig.mode === "deep"
          ? "derin (tüm boş alan)"
          : recConfig.mode === "dev"
            ? "geliştirici (.env, kod ve veritabanı odaklı)"
            : "hızlı"
      } silinmiş dosya ve program taraması çalıştırılacak. Diske HİÇ yazma yapılmaz (salt okunur). Kurtarılan dosyaları her zaman farklı bir sürücüye kaydetmeniz gerekir.`,
      `Runs a strictly read-only ${
        recConfig.mode === "deep"
          ? "deep signature carve"
          : recConfig.mode === "dev"
            ? "developer (.env, source code & DB) recovery"
            : "quick recovery"
      } scan on ${d.model} (${selectedLetter}:). Save any recovered output to a different physical drive.`,
    ),
    async () => {
      viewedJob = null;
      await invoke("start_job", {
        request: {
          kind: "recovery",
          disk_id: d.id,
          letter: selectedLetter,
          confirmation: confirmationToken,
          options: { ...recConfig },
        },
      });
      toast(L("Veri ve uygulama kurtarma taraması başlatıldı.", "File and app recovery scan started."));
    },
    {
      mode: "read_only",
      target: `${d.model} (${selectedLetter}:)`,
      apis:
        recConfig.mode === "deep"
          ? `CreateFileW(\\\\.\\${selectedLetter}:, GENERIC_READ) + $MFT/$UsnJrnl/$I30 + VSS + Carving`
          : "Recycle Bin + VSS Shadow Copies + $MFT + App Traces (GENERIC_READ)",
      writeBudget: L(
        "0 B kaynak sürücü yazması (salt okunur)",
        "0 B written to source volume (strictly read-only)",
      ),
      rollback: L(
        "Gerekmez; kurtarılan dosyalar kullanıcı seçimiyle başka bir sürücüye kaydedilir",
        "Not needed; recovered files save only to user-picked external location",
      ),
      confirmationToken,
    },
  );
}

function describeOptimization(
  action: string,
  target: string,
  value: string,
): [string, string] {
  if (action === "write_cache")
    return [
      value === "rollback"
        ? L("Donanım yazma önbelleğini geri al?", "Revert hardware write cache?")
        : value === "enable"
          ? L("Donanım yazma önbelleğini etkinleştir?", "Enable hardware write cache?")
          : L("Donanım yazma önbelleğini kapat?", "Disable hardware write cache?"),
      L(
        "Disk denetleyicisi yazma önbelleği IOCTL komutu ile güncellenecek. Etkin durum hızı artırır ancak ani elektrik kesintisinde önbellekteki veri kaybolabilir; önceki ayar SQLite kaydından geri alınabilir.",
        "Updates the storage controller write cache policy via Win32 IOCTL. Previous state is archived in SQLite for rollback.",
      ),
    ];
  if (action === "ntfs_trim")
    return [
      value === "rollback" ? L("TRIM ayarını geri al?", "Revert TRIM setting?") : L("Windows TRIM etkinleştirilsin mi?", "Enable Windows TRIM?"),
      L(
        "NTFS DisableDeleteNotify kayıt defteri/fsutil ayarı güncellenecek ve önceki değer geri alma kaydına eklenecek.",
        "Updates Windows NTFS DisableDeleteNotify via fsutil and logs the prior state for rollback.",
      ),
    ];
  if (action === "pcie_aspm")
    return [
      value === "rollback"
        ? L("PCIe ASPM güç ayarını geri al?", "Revert PCIe ASPM power setting?")
        : L("PCIe bağlantı güç tasarrufu (ASPM) güncellensin mi?", "Update PCIe Link Power Management (ASPM)?"),
      L(
        "Etkin Windows güç planında PCI Express ASPM politikası powercfg ile güncellenecek. Tam performans düşük gecikme sağlar ancak dizüstü pil süresini azaltabilir.",
        "Updates active power plan PCIe ASPM setting via powercfg. Maximum Performance improves NVMe wake latency at modest battery cost.",
      ),
    ];
  if (action === "hibernation")
    return [
      value === "rollback"
        ? L("Hazırda bekletme ayarını geri al?", "Revert hibernation setting?")
        : value === "off"
          ? L("Hazırda bekletme (hiberfil.sys) kapatılsın mı?", "Disable hibernation (hiberfil.sys)?")
          : L("Hazırda bekletme etkinleştirilsin mi?", "Enable hibernation?"),
      L(
        "Windows hiberfil.sys dosyası kapatılırsa disk alanı kazanılır ve uyku yazma aşınması azalır; ancak Windows 'Hazırda Beklet' seçeneği devre dışı kalır.",
        "Disabling hiberfil.sys frees drive capacity and reduces SSD hibernation writes, while turning off Windows Hibernate.",
      ),
    ];
  if (action === "hdd_apm")
    return [
      value === "rollback"
        ? L("HDD APM seviyesini geri al?", "Revert HDD APM level?")
        : L("HDD Gelişmiş Güç Yönetimi (APM) ayarlansın mı?", "Set HDD Advanced Power Management (APM)?"),
      L(
        `Seçilen HDD (${target}) için smartctl üzerinden APM seviyesi '${value}' gönderilecek. 254 kafa park etmeyi ve tıklama sesini azaltır.`,
        `Applies APM level '${value}' to HDD ${target} via smartctl. Level 254 reduces frequent head parking.`,
      ),
    ];
  if (action === "apply_op")
    return [
      L(`SSD Over-Provisioning (%${value}) ayrılsın mı?`, `Reserve ${value}% SSD Over-Provisioning?`),
      L(
        `${target.split(":")[1] ?? ""}: bölümü güvenli miktarda küçültülerek disk sonunda ayrılmamış yedek alan bırakılacak ve TRIM uygulanacak. Küçültme sonrası en az 10 GiB boş alan korunur. Önce güncel yedeğinizi doğrulayın.`,
        `Safely shrinks volume ${target.split(":")[1] ?? ""}: to leave unallocated spare space and trims it (preserving >=10 GiB free). Verify backups first.`,
      ),
    ];
  if (action === "reclaim_op")
    return [
      L("Ayrılmamış OP alanını bölüme geri kat?", "Reclaim Over-Provisioning space?"),
      L(
        `${target.split(":")[1] ?? ""}: bölümü arkasındaki bitişik ayrılmamış alan kadar genişletilecek.`,
        `Extends volume ${target.split(":")[1] ?? ""}: back into contiguous unallocated space.`,
      ),
    ];
  if (action === "guard_tail")
    return [
      L(`Ölü kuyruk bölgesi (%${value}) bölüm dışına çıkarılsın mı?`, `Trim degraded tail zone (${value}%) out of partition?`),
      L(
        `${target.split(":")[1] ?? ""}: bölümü sondan %${value} küçültülerek bozuk veya yavaş LBA izleri ayrılmamış koruma alanına bırakılacak. İşlem sonrası en az 10 GiB boş alan korunur ve geri alınabilir.`,
        `Shrinks volume ${target.split(":")[1] ?? ""}: by ${value}% at the tail so degraded LBAs stay unallocated (preserves >=10 GiB free, reversible).`,
      ),
    ];
  if (action === "expand_tail")
    return [
      L("Kırpılan kuyruk alanını bölüme geri kat?", "Reclaim guarded tail space?"),
      L(
        `${target.split(":")[1] ?? ""}: bölümü arkasındaki koruma alanı kadar yeniden genişletilecek.`,
        `Extends volume ${target.split(":")[1] ?? ""}: back into guarded trailing space.`,
      ),
    ];
  if (action === "release_quarantine")
    return [
      L("HDD karantina kalkanı kaldırılsın mı?", "Remove HDD quarantine shield?"),
      L(
        `${target.split(":")[1] ?? ""}: üzerindeki .ssd-sentinel-quarantine koruma dosyaları silinecek ve bu bozuk sektör bölgesi yeniden NTFS boş alanına açılacak.`,
        `Removes .ssd-sentinel-quarantine marker files on ${target.split(":")[1] ?? ""}: so those clusters become allocatable again.`,
      ),
    ];
  if (action === "long_paths")
    return [
      value === "rollback"
        ? L("Windows uzun yol ayarı geri alınsın mı?", "Revert Windows Long Paths setting?")
        : value === "enable"
          ? L("Windows Uzun Yol Desteği (>260 karakter) açılsın mı?", "Enable Windows Long Paths (>260 chars)?")
          : L("Windows Uzun Yol Desteği kapatılsın mı?", "Disable Windows Long Paths?"),
      L(
        "HKLM\\SYSTEM\\CurrentControlSet\\Control\\FileSystem\\LongPathsEnabled kayıt değeri güncellenecek. Derin node_modules ve Cargo/Rust yollarındaki 260 karakter hatalarını önler; önceki değer SQLite kaydından geri alınabilir.",
        "Updates HKLM\\...\\FileSystem\\LongPathsEnabled to prevent 260-character path failures in deep node_modules and Cargo trees; archived in SQLite for rollback.",
      ),
    ];
  if (action === "ntfs_8dot3")
    return [
      value === "rollback"
        ? L("NTFS 8.3 kısa dosya adı ayarı geri alınsın mı?", "Revert NTFS 8.3 short-name setting?")
        : value === "disable"
          ? L("NTFS 8.3 kısa dosya adı üretimi kapatılsın mı?", "Disable NTFS 8.3 short-name creation?")
          : L("NTFS 8.3 ayarı birim bazlı varsayılana alınsın mı?", "Restore NTFS 8.3 default (per-volume)?"),
      L(
        "fsutil behavior set disable8dot3 komutu uygulanacak. Küçük dosya yoğun npm/pip/Cargo kurulumlarında metadata I/O yükünü azaltır; önceki ayar geri alınabilir.",
        "Runs fsutil behavior set disable8dot3 to cut metadata I/O overhead during npm/pip/Cargo installs; reversible via SQLite rollback.",
      ),
    ];
  if (action === "compact_wsl_vhdx")
    return [
      L("WSL2 / Docker VHDX sıkıştırılsın mı?", "Compact WSL2 / Docker VHDX?"),
      L(
        `Önce çalışan WSL/Docker oturumları kapatılacak (wsl --shutdown), ardından ${target} üzerinde salt-okunur bağlama + diskpart compact vdisk çalıştırılarak kullanılmayan boş alan geri kazanılacak. Çalışan konteyner veya açık WSL terminallerinizi kaydedin.`,
        `Stops active WSL/Docker instances (wsl --shutdown), then attaches ${target} read-only and runs diskpart compact vdisk to reclaim sparse blocks. Save open WSL work first.`,
      ),
    ];
  return [
    L("Optimizasyon onaylansın mı?", "Confirm optimization?"),
    L(
      `Hedef ${target} üzerinde ${action}=${value} işlemi uygulanacak.`,
      `Applies ${action}=${value} on target ${target}.`,
    ),
  ];
}

function optimizeApiFor(action: string): string {
  switch (action) {
    case "write_cache":
      return "IOCTL_DISK_GET/SET_CACHE_INFORMATION";
    case "ntfs_trim":
      return "fsutil.exe behavior set DisableDeleteNotify";
    case "pcie_aspm":
      return "powercfg.exe /setacvalueindex (PCIe ASPM)";
    case "hibernation":
      return "powercfg.exe /hibernate";
    case "hdd_apm":
      return "smartctl.exe -s apm";
    case "apply_op":
    case "guard_tail":
      return "Resize-Partition (shrink >=10 GiB reserve) + defrag /L";
    case "reclaim_op":
    case "expand_tail":
      return "Resize-Partition (extend)";
    case "release_quarantine":
      return "Remove .ssd-sentinel-quarantine markers";
    case "long_paths":
      return "RegSetKeyValueW (HKLM\\...\\FileSystem\\LongPathsEnabled)";
    case "ntfs_8dot3":
      return "fsutil.exe behavior set disable8dot3";
    case "compact_wsl_vhdx":
      return "wsl.exe --shutdown + diskpart.exe /s (attach vdisk readonly + compact vdisk)";
    default:
      return action;
  }
}

function runOptimize(action: string, target: string, value: string) {
  if (action === "query_op_bounds") {
    void run(async () => {
      const res = await invoke<{
        size_min: number;
        size_max: number;
        current_size: number;
        free_bytes: number;
        message?: string;
      }>("optimize_action", {
        request: {
          action,
          target,
          value,
          confirmation: `optimize:${action}:${target}`,
        },
      });
      const [disk_id, l] = target.split(":");
      opBounds = {
        disk_id,
        letter: l ?? "",
        size_min: res.size_min,
        size_max: res.size_max,
        current_size: res.current_size,
        free_bytes: res.free_bytes,
      };
      toast(res.message ?? L("Bölüm küçültme sınırları okundu.", "Volume shrink bounds retrieved."));
    });
    return;
  }
  const [title, body] = describeOptimization(action, target, value);
  const confirmationToken = `optimize:${action}:${target}`;
  confirm(
    title,
    body,
    async () => {
      const res = await invoke<{ message?: string }>("optimize_action", {
        request: {
          action,
          target,
          value,
          confirmation: confirmationToken,
        },
      });
      opBounds = null;
      toast(res.message ?? L("Optimizasyon uygulandı.", "Optimization applied."));
    },
    {
      mode: "mutating",
      target,
      apis: optimizeApiFor(action),
      rollback: L(
        "Önceki durum SQLite optimizations tablosunda saklanır",
        "Prior value archived in SQLite optimizations table",
      ),
      confirmationToken,
    },
  );
}

root.addEventListener("click", (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>("button");
  if (!target || target.hasAttribute("disabled")) return;
  if (target.dataset.lang === "tr" || target.dataset.lang === "en") {
    applyLanguage(target.dataset.lang);
    return;
  }
  if (target.dataset.page || target.dataset.goto) {
    page = (target.dataset.page ?? target.dataset.goto) as Page;
    render();
    document.querySelector("#main")!.scrollTop = 0;
    return;
  }
  if (target.dataset.disk) {
    selected = target.dataset.disk;
    field = "";
    viewedJob = null;
    opBounds = null;
    detailKey = "";
    history = [];
    events = [];
    benchmarkHistory = [];
    surfaceHistory = [];
    defragHistory = [];
    smartTierHistory = [];
    autopilotHistory = [];
    recoveryHistory = [];
    healthForecast = null;
    surfaceCheckpoint = null;
    tierAnalysis = null;
    devAnalysis = null;
    devCustomRoot = "";
    devFilter = "all";
    selectedRecoveryIds.clear();
    recoveryTabManual = false;
    lastRecoveryJobId = "";
    letter = disk()?.volumes[0]?.letter ?? "";
    render();
    void loadDetails(true);
    return;
  }
  if (target.dataset.field) {
    field = target.dataset.field;
    render();
    return;
  }
  if (target.dataset.block) {
    blockIndex = Number(target.dataset.block);
    render();
    return;
  }
  if (target.dataset.unit) {
    unit = target.dataset.unit as typeof unit;
    render();
    return;
  }
  if (target.dataset.benchPreset) {
    applyBenchPreset(target.dataset.benchPreset as "quick" | "full");
    return;
  }
  if (target.dataset.recTab) {
    recoveryTab = target.dataset.recTab as "apps" | "files";
    recoveryTabManual = true;
    render();
    return;
  }
  if (target.dataset.appFilter) {
    appFilter = target.dataset.appFilter as AppFilter;
    render();
    return;
  }
  if (target.dataset.copyWinget) {
    const cmd = target.dataset.copyWinget;
    void copyText(cmd).then(() => {
      toast(L(`Komut panoya kopyalandı: ${cmd}`, `Command copied to clipboard: ${cmd}`));
    });
    return;
  }
  if (target.dataset.copyText) {
    const txt = target.dataset.copyText;
    void copyText(txt).then(() => {
      toast(L("Komutlar panoya kopyalandı.", "Commands copied to clipboard."));
    });
    return;
  }
  if (target.dataset.devFilter === "all" || target.dataset.devFilter === "stale") {
    devFilter = target.dataset.devFilter;
    render();
    return;
  }
  if (target.dataset.devCleanPath && target.dataset.devCleanToken) {
    cleanDevArtifact(
      target.dataset.devCleanPath,
      target.dataset.devCleanToken,
      target.dataset.devCleanName ?? target.dataset.devCleanPath,
      Number(target.dataset.devCleanBytes ?? 0),
    );
    return;
  }
  if (target.dataset.devOffloadId) {
    offloadDevCache(target.dataset.devOffloadId);
    return;
  }
  if (target.dataset.recCat) {
    recoveryCat = target.dataset.recCat;
    render();
    return;
  }
  if (target.dataset.previewId) {
    const jobId = target.dataset.previewJob ?? "";
    const itemId = target.dataset.previewId;
    const itemName = target.dataset.previewName ?? L("Dosya", "File");
    const recoverable = target.dataset.previewRecoverable === "1";
    void openRecoveryPreview(jobId, itemId, itemName, recoverable);
    return;
  }
  if (target.dataset.restoreId) {
    const jobId = target.dataset.restoreJob ?? "";
    const itemId = target.dataset.restoreId;
    void run(async () => {
      const res = await invoke<{
        saved: boolean;
        path?: string;
        repaired?: boolean;
      }>("restore_recovered_item", { jobId, itemId });
      if (res.saved) {
        toast(
          L(
            `Dosya kurtarıldı${res.repaired ? " (otomatik onarıldı)" : ""}: ${res.path ?? ""}`,
            `File restored${res.repaired ? " (auto-repaired)" : ""}: ${res.path ?? ""}`,
          ),
        );
      }
    });
    return;
  }
  if (target.dataset.benchRow) {
    start("benchmark", target.dataset.benchRow);
    return;
  }
  if (target.dataset.defragMode) {
    startDefrag(
      target.dataset.defragMode as "analyze" | "defrag" | "consolidate",
    );
    return;
  }
  if (target.dataset.tierJob) {
    startSmartTier(
      target.dataset.tierJob as "ssd_extent_optimize" | "hdd_outer_optimize",
    );
    return;
  }
  if (target.dataset.tierRelocate) {
    relocateTierCandidate(target.dataset.tierRelocate);
    return;
  }
  if (target.dataset.tierRollback) {
    rollbackTierMove(target.dataset.tierRollback);
    return;
  }
  if (target.dataset.recommendedSurface) {
    scan.mode = target.dataset.recommendedSurface;
    render();
    start("surface");
    return;
  }
  if (target.dataset.optAction) {
    runOptimize(
      target.dataset.optAction,
      target.dataset.optTarget ?? "system",
      target.dataset.optValue ?? "",
    );
    return;
  }
  if (target.dataset.rollbackKey) {
    const rKey = target.dataset.rollbackKey;
    const rTarget = target.dataset.rollbackTarget ?? "system";
    if (rKey.startsWith("op:")) {
      runOptimize("reclaim_op", rTarget, "reclaim");
    } else if (rKey.startsWith("tail:")) {
      runOptimize("expand_tail", rTarget, "reclaim");
    } else if (rKey.startsWith("quarantine:")) {
      runOptimize("release_quarantine", rTarget, "release");
    } else {
      runOptimize(rKey.split(":")[0], rTarget, "rollback");
    }
    return;
  }
  if (target.dataset.viewJob) {
    viewedJob =
      [
        ...benchmarkHistory,
        ...surfaceHistory,
        ...defragHistory,
        ...smartTierHistory,
      ].find((j) => j.id === target.dataset.viewJob) ?? null;
    const profile = viewedJob?.options.profile;
    if (
      viewedJob?.kind === "benchmark" &&
      typeof profile === "string" &&
      state?.profiles[profile]
    )
      bench.profile = profile;
    blockIndex = 0;
    render();
    document.querySelector("#main")!.scrollTop = 0;
    return;
  }
  const action = target.dataset.action;
  const d = disk();
  if (action === "theme") {
    document.documentElement.dataset.theme =
      document.documentElement.dataset.theme === "dark" ? "light" : "dark";
    localStorage.setItem(
      "sentinel-theme",
      document.documentElement.dataset.theme!,
    );
    if (state)
      void invoke("set_preferences", {
        minimiseToTray: state.minimise_to_tray,
        theme: document.documentElement.dataset.theme,
        healthMode: state.health_mode,
        language: getLang(),
        readOnlyGuard: state.read_only_guard ?? true,
      }).catch((error) => toast(String(error), true));
    render();
  } else if (action === "toggle-write-guard") {
    toggleWriteGuard(isWriteLocked(state));
  } else if (action === "refresh")
    void run(async () => {
      await invoke("refresh");
      toast(L("Disk bilgileri yeniden okunuyor.", "Refreshing drive telemetry."));
    });
  else if (action === "elevate")
    void run(async () => {
      const res = await invoke<{
        relaunching: boolean;
        already_admin?: boolean;
      }>("restart_as_admin");
      if (res.already_admin)
        toast(L("Uygulama zaten yönetici olarak çalışıyor.", "Already running as Administrator."));
      else if (res.relaunching)
        toast(
          L(
            "Yönetici onayı verildi; uygulama yükseltilmiş yetkiyle yeniden başlatılıyor…",
            "Elevation granted; restarting with Administrator privileges…",
          ),
        );
      else toast(L("Yönetici yükseltme isteği iptal edildi.", "Elevation prompt was cancelled."), true);
    });
  else if (action === "check-updates") {
    updateBannerDismissed = false;
    void run(async () => {
      updateStatus = await invoke<typeof updateStatus>("check_updates");
      toast(
        updateStatus
          ? translateUpdateMessage(updateStatus, getLang())
          : L("Sürüm durumu kontrol edildi.", "Version check finished."),
      );
    });
  } else if (action === "dismiss-update-banner") {
    updateBannerDismissed = true;
    render();
  } else if (action === "open-release" && updateStatus?.url)
    void invoke("open_release_url", { url: updateStatus.url }).catch((error) =>
      toast(String(error), true),
    );
  else if (
    action === "install-update" &&
    updateStatus?.download_url &&
    updateStatus?.asset_name
  ) {
    const dlUrl = updateStatus.download_url;
    const aName = updateStatus.asset_name;
    const shaUrl = updateStatus.sha256_url ?? null;
    const latest = updateStatus.latest ?? "";
    const sizeText = updateStatus.asset_size
      ? ` (${fmtBytes(updateStatus.asset_size)})`
      : "";
    confirm(
      L(
        `Güncelleme indirilsin ve kurulsun mu? (${latest})`,
        `Download & install update (${latest})?`,
      ),
      L(
        `${aName}${sizeText} resmi GitHub deposundan HTTPS ile indirilecek, SHA-256 doğrulaması yapılacak ve kurulum başlatılacaktır.`,
        `${aName}${sizeText} will be downloaded over HTTPS from GitHub, verified against SHA-256, and launched.`,
      ),
      async () => {
        const res = await invoke<{
          launched: boolean;
          path?: string;
          sha256?: string;
          verified?: boolean;
        }>("download_and_launch_update", {
          downloadUrl: dlUrl,
          assetName: aName,
          sha256Url: shaUrl,
        });
        toast(
          res.verified
            ? L(
                `Kurulum SHA-256 doğrulaması ile başlatıldı: ${aName}`,
                `Verified installer (SHA-256) launched: ${aName}`,
              )
            : L(
                `Kurulum başlatıldı: ${aName}`,
                `Installer launched: ${aName}`,
              ),
        );
      },
      {
        mode: "system",
        target: aName,
        apis: "WinHTTP HTTPS Allowlist + SHA256SUMS.txt + ShellExecuteW",
        rollback: L(
          "SHA-256 eşleşmezse indirilen dosya çalıştırılmadan silinir",
          "Aborted and deleted before launch on any SHA-256 mismatch",
        ),
      },
    );
  }
  else if (action === "advanced") {
    advanced = !advanced;
    render();
  } else if (action === "sort-raw") {
    sort = sort === "raw-desc" ? "raw-asc" : "raw-desc";
    render();
  } else if (action === "start-benchmark") start("benchmark");
  else if (action === "start-surface") start("surface");
  else if (action === "resume-surface") start("surface", "all", true);
  else if (action === "clear-surface-checkpoint" && d)
    void run(async () => {
      await invoke("clear_surface_checkpoint", { diskId: d.id });
      surfaceCheckpoint = null;
      toast(L("Kayıtlı tarama noktası sıfırlandı.", "Saved scan checkpoint cleared."));
    });
  else if (action === "start-autopilot") startAutopilot();
  else if (action === "analyze-tiering") void analyzeStorageTiering();
  else if (action === "analyze-dev") void analyzeDevWorkspace();
  else if (action === "pick-dev-folder") void pickDevFolder();
  else if (action === "clear-dev-root") {
    devCustomRoot = "";
    render();
    void analyzeDevWorkspace();
  }
  else if (action === "start-recovery") startRecovery();
  else if (action === "rec-select-all") {
    const job = currentJob("recovery") ?? recoveryHistory[0] ?? null;
    const rawItems = job?.recovery_items ?? [];
    const pool = showUnopenableMedia
      ? rawItems
      : rawItems.filter((i) => !isUnopenableMedia(i));
    for (const it of aiNaturalFilter(pool, recoverySearch, recoveryCat)) {
      if (isRecoverableRow(it)) selectedRecoveryIds.add(it.id);
    }
    render();
  } else if (action === "rec-clear-selection") {
    selectedRecoveryIds.clear();
    render();
  } else if (action === "rec-restore-batch") {
    const job = currentJob("recovery") ?? recoveryHistory[0] ?? null;
    const validIds = new Set((job?.recovery_items ?? []).map((i) => i.id));
    const itemIds = Array.from(selectedRecoveryIds).filter((id) =>
      validIds.has(id),
    );
    if (job && itemIds.length) {
      void run(async () => {
        const res = await invoke<BatchRestoreResult>("restore_recovered_batch", {
          jobId: job.id,
          itemIds,
        });
        if (res.cancelled) return;
        if (res.restored > 0) {
          selectedRecoveryIds.clear();
          const repairedN = Number(res.repaired_count ?? 0);
          const extra = [
            repairedN > 0
              ? L(`${repairedN} onarıldı`, `${repairedN} repaired`)
              : res.repaired
                ? L("otomatik onarıldı", "auto-repaired")
                : "",
            res.failed ? L(`${res.failed} başarısız`, `${res.failed} failed`) : "",
            res.skipped ? L(`${res.skipped} atlandı`, `${res.skipped} skipped`) : "",
          ]
            .filter(Boolean)
            .join(", ");
          toast(
            L(
              `${res.restored} dosya kurtarıldı → ${res.dest_dir}${extra ? ` (${extra})` : ""}`,
              `${res.restored} file(s) restored → ${res.dest_dir}${extra ? ` (${extra})` : ""}`,
            ),
          );
        } else if (res.errors?.length) {
          toast(
            L(`Toplu kurtarma tamamlanamadı: ${res.errors[0]}`, `Batch restore failed: ${res.errors[0]}`),
            true,
          );
        } else {
          toast(L("Seçilen dosyalar kurtarılamadı.", "Selected files could not be restored."), true);
        }
      });
    }
  } else if (action === "cancel-job" && state?.job) {
    const jobId = state.job.id;
    void (async () => {
      try {
        await invoke("cancel_job", { id: jobId });
        toast(L("İş durduruluyor…", "Stopping active job…"));
        await poll();
      } catch (error) {
        toast(String(error), true);
      }
    })();
  } else if (action?.startsWith("export"))
    void run(async () => {
      const isHtml = action === "export-html";
      const result = await invoke<{ saved: boolean }>("save_report", {
        kind:
          action === "export" || isHtml
            ? "snapshot"
            : action.replace("export-", ""),
        diskId: d?.id ?? null,
        format: isHtml ? "html" : "json",
        redact: true,
      });
      if (result.saved) toast(L("Rapor kaydedildi.", "Report saved."));
    });
  else if (
    d &&
    action &&
    ["logs", "short_test", "long_test", "abort_test", "retrim"].includes(action)
  ) {
    const selectedLetter = letter || d.volumes[0]?.letter || null;
    const execute = async () => {
      const result = await invoke<unknown>("disk_action", {
        diskId: d.id,
        action,
        confirmation: `${action}:${d.id}`,
        letter: selectedLetter,
      });
      dialog.innerHTML = `<h2 id="confirm-title">${action === "logs" ? L("SMART günlükleri", "SMART device logs") : L("İşlem sonucu", "Action result")}</h2><pre>${e(JSON.stringify(result, null, 2))}</pre><button class="button primary" id="close-result">${L("Kapat", "Close")}</button>`;
      dialog.showModal();
      document
        .querySelector("#close-result")!
        .addEventListener("click", () => dialog.close());
    };
    if (action === "logs") void run(execute);
    else {
      const actText =
        action === "retrim"
          ? "Windows ReTrim"
          : action === "short_test"
            ? L("kısa SMART testi", "short SMART self-test")
            : action === "long_test"
              ? L("uzun SMART testi", "extended SMART self-test")
              : L("SMART testi durdurma", "abort SMART self-test");
      confirm(
        L("Disk bakım işlemini onaylayın", "Confirm drive maintenance action"),
        L(
          `${d.model}${action === "retrim" ? ` · ${selectedLetter}:` : ""} üzerinde ${actText} isteği gönderilecek. Disk yükü ve performansı etkilenebilir. Güncel yedeğinizi doğrulayın.`,
          `Sends ${actText} to ${d.model}${action === "retrim" ? ` (${selectedLetter}:)` : ""}. Drive I/O may be affected while running. Verify backups first.`,
        ),
        execute,
        {
          mode: "mutating",
          target: `${d.model}${action === "retrim" ? ` (${selectedLetter}:)` : ""}`,
          apis:
            action === "retrim"
              ? "Optimize-Volume -ReTrim"
              : `smartctl.exe ${action === "abort_test" ? "-X" : "-t"}`,
          rollback: L(
            "Firmware testleri smartctl -X ile durdurulabilir",
            "Firmware self-tests can be aborted via smartctl -X",
          ),
          confirmationToken: `${action}:${d.id}`,
        },
      );
    }
  }
});
root.addEventListener("input", (event) => {
  const t = event.target as HTMLInputElement;
  if (t.id === "smart-search") {
    search = t.value;
    render();
  } else if (t.id === "recovery-search") {
    recoverySearch = t.value;
    render();
  } else if (t.id === "recovery-app-search") {
    appSearch = t.value;
    render();
  }
});
root.addEventListener("change", (event) => {
  const t = event.target as HTMLInputElement;
  if (t.id === "smart-filter") {
    filter = t.value;
    render();
  } else if (t.id === "target-volume" || t.dataset.volumeSelect) {
    letter = t.value;
    opBounds = null;
    if (tierAnalysis && tierAnalysis.letter !== letter) {
      tierAnalysis = null;
    }
    if (devAnalysis && devAnalysis.letter !== letter) {
      devAnalysis = null;
    }
    render();
  } else if (t.dataset.recSelect) {
    const id = t.dataset.recSelect;
    if (t.checked) selectedRecoveryIds.add(id);
    else selectedRecoveryIds.delete(id);
    render();
  } else if (t.id === "show-unopenable-media") {
    showUnopenableMedia = t.checked;
    render();
  } else if (t.id.startsWith("bench-")) {
    const key = t.id.slice(6) as keyof typeof bench;
    Object.assign(bench, {
      [key]: ["size_mib", "repeats", "seconds", "interval"].includes(key)
        ? Number(t.value)
        : t.value,
    });
    bench.row = "all";
    benchPreset = "custom";
    render();
  } else if (t.name === "scan-mode") {
    scan.mode = t.value;
    render();
  } else if (t.name === "rec-mode") {
    recConfig.mode = t.value;
    render();
  } else if (t.id === "recovery-sort") {
    recoverySort = t.value as typeof recoverySort;
    render();
  } else if (t.id === "language-setting") {
    applyLanguage(t.value === "en" ? "en" : "tr");
  } else if (t.id === "minimise-setting" && state)
    void run(async () => {
      await invoke("set_preferences", {
        minimiseToTray: t.checked,
        theme: document.documentElement.dataset.theme,
        healthMode: state!.health_mode,
        language: getLang(),
        readOnlyGuard: state!.read_only_guard ?? true,
      });
      state!.minimise_to_tray = t.checked;
    });
  else if (t.id === "health-mode-setting" && state)
    void run(async () => {
      await invoke("set_preferences", {
        minimiseToTray: state!.minimise_to_tray,
        theme: document.documentElement.dataset.theme,
        healthMode: t.value,
        language: getLang(),
        readOnlyGuard: state!.read_only_guard ?? true,
      });
      toast(
        t.value === "strict"
          ? L("Sıkı sağlık hesaplama modu etkinleştirildi.", "Strict health calculation mode enabled.")
          : L("Varsayılan (Standart) sağlık moduna geçildi.", "Switched to Default (Standard) health calculation mode."),
      );
    });
  else if (t.id === "write-guard-setting" && state) {
    if (!t.checked) {
      t.checked = true;
      toggleWriteGuard(true);
    } else {
      toggleWriteGuard(false);
    }
  } else if (t.id === "auto-update-setting" && state)
    void run(async () => {
      await invoke("set_preferences", {
        minimiseToTray: state!.minimise_to_tray,
        theme: document.documentElement.dataset.theme,
        healthMode: state!.health_mode,
        language: getLang(),
        readOnlyGuard: state!.read_only_guard ?? true,
        autoCheckUpdates: t.checked,
      });
      state!.auto_check_updates = t.checked;
      if (t.checked && state!.source === "live") {
        autoUpdateChecked = false;
        void maybeAutoCheckUpdates();
      }
      toast(
        t.checked
          ? L(
              "Açılışta otomatik güncelleme denetimi etkinleştirildi.",
              "Automatic startup update check enabled.",
            )
          : L(
              "Açılışta otomatik güncelleme denetimi kapatıldı.",
              "Automatic startup update check disabled.",
            ),
      );
    });
  else if (t.id === "autostart-setting" && state)
    void run(async () => {
      const res = await invoke<{ message?: string }>("optimize_action", {
        request: {
          action: "autostart",
          target: "system",
          value: t.checked ? "enable" : "disable",
          confirmation: "optimize:autostart:system",
        },
      });
      await invoke("refresh");
      toast(res.message ?? L("Windows başlangıç tercihi güncellendi.", "Windows startup preference updated."));
    });
});
void poll();
window.setInterval(() => {
  if (!document.hidden || active(state?.job)) void poll();
}, 2000);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) void poll();
});
