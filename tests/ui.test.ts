import { test } from "node:test";
import assert from "node:assert/strict";
import {
  aiNaturalFilter,
  alignmentDescription,
  bytes,
  checkpointProgress,
  compareRaw,
  computePollSignature,
  computeSurfaceStepper,
  canStart,
  canMaintain,
  canOptimizeSystem,
  canWrite,
  devCacheKindBadge,
  devEnvSnippet,
  devStackBadge,
  escape,
  evaluateBenchmarkSpeed,
  filterDeletedApps,
  findCompanionDrive,
  format,
  formatInterface,
  formatUntouchedDays,
  hasNormalizedSmart,
  inferAppRecoveryStatus,
  isRecoverableRow,
  isUnopenableMedia,
  isWriteLocked,
  maskSerial,
  mediaStatusBadge,
  ntfs8dot3Label,
  slcPressureBadge,
  tierCategoryBadge,
  tierScoreBadge,
} from "../src/ui.ts";

test("missing numbers stay unknown while known zero remains visible", () => {
  assert.equal(format(null), "—");
  assert.equal(format(0), "0");
  assert.equal(bytes(null), "—");
  assert.equal(bytes(1_000_000_000_000), "1 TB");
});
test("raw sorting preserves precision and escapes device text", () => {
  assert.equal(compareRaw("9007199254740993", "9007199254740992"), 1);
  assert.equal(
    escape('<img src=x onerror="boom">'),
    "&lt;img src=x onerror=&quot;boom&quot;&gt;",
  );
});
test("real writes never become available for replay stale busy or unmapped drives", () => {
  const disk = {
    stable_identity: true,
    os_disk_number: 1,
    os_serial: "s",
    os_model: "m",
    volumes: [{ letter: "C" }],
  };
  const state = { source: "live", stale: false, busy: false };
  assert.equal(canStart(state, disk), true);
  for (const changed of [{ source: "replay" }, { stale: true }, { busy: true }])
    assert.equal(canStart({ ...state, ...changed }, disk), false);
  assert.equal(canStart(state, { ...disk, os_serial: null }), false);
});

test("firmware mutations require unique Windows identity without needing a formatted volume", () => {
  const s = { source: "live", stale: false, busy: false, admin: true };
  const d = {
    stable_identity: true,
    os_disk_number: 1,
    os_serial: "s",
    os_model: "m",
    volumes: [],
  };
  assert.equal(canMaintain(s, d), true);
  assert.equal(canMaintain(s, { ...d, os_disk_number: null }), false);
  assert.equal(canMaintain({ ...s, admin: false }, d), false);
});

test("system optimization requires live non-stale idle admin session and maskSerial hides middle characters", () => {
  const s = { source: "live", stale: false, busy: false, admin: true };
  assert.equal(canOptimizeSystem(s), true);
  for (const changed of [
    { source: "replay" },
    { stale: true },
    { busy: true },
    { admin: false },
  ]) {
    assert.equal(canOptimizeSystem({ ...s, ...changed }), false);
  }
  assert.equal(maskSerial(null), "—");
  assert.equal(maskSerial("ABCD"), "••••");
  assert.equal(maskSerial("SN1234567890"), "SN••••••••90");
});

test("aiNaturalFilter separates EXE/apps from DLL noise and filters Turkish categories", () => {
  const sample = [
    { name: "SpotifySetup.exe", category: "exe_setup", confidence: 92, extension: "exe", original_path: "D:\\Downloads\\SpotifySetup.exe", sub_type: "64-bit NSIS Kurulum Paketi", product_name: "Spotify", publisher: "Spotify AB", ai_summary: "Tam PE" },
    { name: "Fatura.pdf", category: "document", confidence: 88, extension: "pdf", original_path: "D:\\Belgeler\\Fatura.pdf", ai_summary: "PDF" },
    { name: "vcruntime140.dll", category: "system_lib", confidence: 85, extension: "dll", original_path: "D:\\Temp\\vcruntime140.dll", ai_summary: "DLL" },
    { name: "broken.exe", category: "exe_setup", confidence: 15, extension: "exe", original_path: "D:\\Temp\\broken.exe", ai_summary: "Ezilmiş" },
  ];
  assert.deepEqual(
    aiNaturalFilter(sample, "", "all").map((i) => i.name),
    ["SpotifySetup.exe", "Fatura.pdf", "broken.exe"],
  );
  assert.deepEqual(
    aiNaturalFilter(sample, "sağlam uygulama").map((i) => i.name),
    ["SpotifySetup.exe"],
  );
  assert.deepEqual(
    aiNaturalFilter(sample, "belge fatura").map((i) => i.name),
    ["Fatura.pdf"],
  );
  assert.deepEqual(
    aiNaturalFilter(sample, "", "system_lib").map((i) => i.name),
    ["vcruntime140.dll"],
  );
});

test("formatInterface prefixes bare NVMe version numbers without mangling SATA or existing prefixes", () => {
  assert.equal(formatInterface("1.3", "NVMe SSD"), "NVMe 1.3");
  assert.equal(formatInterface("1.4", "NVMe SSD"), "NVMe 1.4");
  assert.equal(formatInterface("2.0", ""), "NVMe 2.0");
  assert.equal(formatInterface("NVMe 1.3", "NVMe SSD"), "NVMe 1.3");
  assert.equal(formatInterface("6.0 Gb/s", "HDD"), "6.0 Gb/s");
  assert.equal(formatInterface("1.3", "HDD"), "1.3");
  assert.equal(formatInterface(null, "NVMe SSD"), "—");
});

test("alignmentDescription never claims 4K alignment is verified when mapping is missing", () => {
  assert.equal(
    alignmentDescription(null),
    "Windows bölüm eşlemesi olmadan 4K hizalama doğrulanamaz.",
  );
  assert.equal(
    alignmentDescription(undefined),
    "Windows bölüm eşlemesi olmadan 4K hizalama doğrulanamaz.",
  );
  assert.match(alignmentDescription(true), /4K sektör sınırında/);
  assert.match(alignmentDescription(false), /Hizasız/);
});

test("hasNormalizedSmart distinguishes SATA normalized SMART from NVMe raw-only rows", () => {
  assert.equal(
    hasNormalizedSmart([
      { value: null, worst: null, threshold: null },
      { value: null, worst: null, threshold: null },
    ]),
    false,
  );
  assert.equal(
    hasNormalizedSmart([
      { value: 200, worst: 200, threshold: 51 },
    ]),
    true,
  );
});

test("filterDeletedApps and inferAppRecoveryStatus handle v1.7 fields and legacy fixtures cleanly", () => {
  const apps = [
    {
      name: "Spotify",
      publisher: "Spotify AB",
      exe_path: "D:\\Downloads\\SpotifySetup.exe",
      category: "Uygulama",
      recovery_status: "full_restore",
      vss_recoverable: true,
      installer_found: true,
      sources: ["Prefetch", "VSS Gölge Kopya"],
    },
    {
      name: "Valorant",
      publisher: "Riot Games",
      exe_path: "D:\\Riot Games\\VALORANT\\valorant.exe",
      category: "Oyun",
      recovery_status: "reinstall_only",
      vss_recoverable: false,
      installer_found: false,
      sources: ["GameConfigStore", "Firewall"],
    },
    {
      name: "Rufus",
      publisher: "Akeo",
      exe_path: "D:\\Downloads\\rufus-4.5.exe",
      category: "Araç",
      installer_found: true,
      recoverable_item_id: "rec-9",
      sources: ["Prefetch"],
    },
  ];
  assert.equal(inferAppRecoveryStatus(apps[0]), "full_restore");
  assert.equal(inferAppRecoveryStatus(apps[1]), "reinstall_only");
  assert.equal(inferAppRecoveryStatus(apps[2]), "installer_found");
  assert.deepEqual(
    filterDeletedApps(apps, "", "full_restore").map((a) => a.name),
    ["Spotify"],
  );
  assert.deepEqual(
    filterDeletedApps(apps, "", "installer_found").map((a) => a.name),
    ["Rufus"],
  );
  assert.deepEqual(
    filterDeletedApps(apps, "", "games").map((a) => a.name),
    ["Valorant"],
  );
  assert.deepEqual(
    filterDeletedApps(apps, "", "reinstall_only").map((a) => a.name),
    ["Valorant"],
  );
  assert.deepEqual(
    filterDeletedApps(apps, "riot firewall", "all").map((a) => a.name),
    ["Valorant"],
  );
});

test("isRecoverableRow blocks trimmed, unopenable media, and metadata-only entries", () => {
  assert.equal(isRecoverableRow({ source: "vss_shadow", trim_zeroed: false }), true);
  assert.equal(isRecoverableRow({ source: "mft_nonresident", trim_zeroed: false }), true);
  assert.equal(isRecoverableRow({ source: "mft_nonresident", trim_zeroed: true }), false);
  assert.equal(isRecoverableRow({ source: "carved", trim_zeroed: false, openable: false, media_status: "unrecoverable" }), false);
  assert.equal(isRecoverableRow({ source: "carved", trim_zeroed: false, openable: true, media_status: "repaired" }), true);
  assert.equal(isRecoverableRow({ source: "usn_journal", trim_zeroed: false }), false);
  assert.equal(isRecoverableRow({ source: "indx_slack", trim_zeroed: false }), false);
  assert.equal(isUnopenableMedia({ openable: false }), true);
  assert.equal(isUnopenableMedia({ media_status: "unrecoverable" }), true);
  assert.equal(isUnopenableMedia({ openable: true, media_status: "repaired" }), false);
  assert.deepEqual(mediaStatusBadge({ media_status: "repaired", openable: true }), {
    label: "Otomatik Onarılacak",
    level: "ok",
  });
  assert.deepEqual(mediaStatusBadge({ media_status: "thumbnail_rescued", openable: true }), {
    label: "Önizleme Kurtarıldı",
    level: "warning",
  });
  assert.deepEqual(mediaStatusBadge({ media_status: "unrecoverable", openable: false }), {
    label: "Açılamaz",
    level: "critical",
  });
  assert.equal(mediaStatusBadge({}), null);
});

test("evaluateBenchmarkSpeed grades SEQ read against NVMe, SATA SSD, and HDD bands", () => {
  assert.equal(evaluateBenchmarkSpeed(null, "NVMe SSD"), null);
  assert.equal(evaluateBenchmarkSpeed(0, "HDD"), null);
  const nvmeFast = evaluateBenchmarkSpeed(3500, "NVMe SSD", "NVMe 1.3");
  assert.equal(nvmeFast?.band, "NVMe");
  assert.equal(nvmeFast?.level, "ok");
  const nvmeSlow = evaluateBenchmarkSpeed(450, "NVMe SSD", "NVMe 1.3");
  assert.equal(nvmeSlow?.level, "critical");
  const sataGood = evaluateBenchmarkSpeed(520, "SATA SSD", "6.0 Gb/s");
  assert.equal(sataGood?.band, "SATA SSD");
  assert.equal(sataGood?.level, "ok");
  const hddNormal = evaluateBenchmarkSpeed(140, "HDD", "6.0 Gb/s");
  assert.equal(hddNormal?.band, "HDD");
  assert.equal(hddNormal?.level, "ok");
  const hddSlow = evaluateBenchmarkSpeed(50, "HDD", "6.0 Gb/s");
  assert.equal(hddSlow?.level, "warning");
});

test("computeSurfaceStepper and checkpointProgress reflect active scan, diagnosis, and pinpoint states", () => {
  assert.deepEqual(
    computeSurfaceStepper({
      hasFullScan: false,
      hasDiagnosis: false,
      hasPinpoint: false,
    }),
    { activeStep: 1, states: ["active", "pending", "pending"] },
  );
  assert.deepEqual(
    computeSurfaceStepper({
      hasFullScan: true,
      hasDiagnosis: true,
      hasPinpoint: false,
      hasTargets: true,
    }),
    { activeStep: 2, states: ["done", "active", "pending"] },
  );
  assert.deepEqual(
    computeSurfaceStepper({
      hasFullScan: true,
      hasDiagnosis: true,
      hasPinpoint: false,
      runningMode: "magnetic_refresh",
    }),
    { activeStep: 3, states: ["done", "done", "active"] },
  );
  assert.deepEqual(
    computeSurfaceStepper({
      hasFullScan: true,
      hasDiagnosis: true,
      hasPinpoint: true,
    }),
    { activeStep: 3, states: ["done", "done", "done"] },
  );
  assert.deepEqual(
    checkpointProgress({ next_zone: 60, total_zones: 200 }),
    { completed: 60, total: 200, percent: 30 },
  );
  assert.deepEqual(
    checkpointProgress({ done_bytes: 500, disk_bytes: 1000, next_zone: 100 }),
    { completed: 100, total: 200, percent: 50 },
  );
});

test("computePollSignature ignores live performance jitter off the performance page", () => {
  const base = {
    source: "live",
    admin: true,
    updating: false,
    stale: false,
    busy: false,
    updated_at: "2026-11-21T10:00:00Z",
    error: null,
    disks: [{ id: "d1", health: { percent: 98 } }],
    performance: { disks: [{ number: 0, read_bps: 1000 }], error: null },
    job: null,
  };
  const nextTick = {
    ...base,
    performance: { disks: [{ number: 0, read_bps: 999999 }], error: null },
  };
  const ctx = {
    page: "recovery",
    selected: "d1",
    letter: "C",
    connectionError: "",
  };
  assert.equal(
    computePollSignature(base, ctx),
    computePollSignature(nextTick, ctx),
  );
  assert.notEqual(
    computePollSignature(base, { ...ctx, page: "performance" }),
    computePollSignature(nextTick, { ...ctx, page: "performance" }),
  );
  assert.notEqual(
    computePollSignature(base, { ...ctx, page: "optimize", tierAnalysis: null }),
    computePollSignature(base, {
      ...ctx,
      page: "optimize",
      tierAnalysis: { disk_id: "d1", extreme_fragmented_count: 3 },
    }),
  );
  assert.notEqual(
    computePollSignature(base, { ...ctx, page: "optimize", tierMoves: [] }),
    computePollSignature(base, {
      ...ctx,
      page: "optimize",
      tierMoves: [{ id: "m1" }],
    }),
  );
});

test("v1.8 smart tiering badges format SLC pressure, categories, cold/hot scores, and untouched days", () => {
  assert.deepEqual(slcPressureBadge("low"), { label: "Düşük", level: "ok" });
  assert.deepEqual(slcPressureBadge("moderate"), {
    label: "Orta",
    level: "warning",
  });
  assert.deepEqual(slcPressureBadge("high"), {
    label: "Yüksek — SLC Önbellek Daralıyor",
    level: "critical",
  });
  assert.deepEqual(slcPressureBadge("high", false), {
    label: "Yüksek Doluluk",
    level: "critical",
  });

  assert.deepEqual(tierCategoryBadge("video"), { label: "Video", level: "info" });
  assert.deepEqual(tierCategoryBadge("archive"), {
    label: "Arşiv",
    level: "warning",
  });
  assert.deepEqual(tierCategoryBadge("iso"), { label: "ISO", level: "warning" });
  assert.deepEqual(tierCategoryBadge("installer"), {
    label: "Kurulum",
    level: "info",
  });
  assert.deepEqual(tierCategoryBadge("folder", true), {
    label: "Klasör",
    level: "",
  });
  assert.deepEqual(tierCategoryBadge("other", true), {
    label: "Klasör",
    level: "",
  });
  assert.deepEqual(tierCategoryBadge("other", false), {
    label: "Dosya",
    level: "",
  });

  assert.deepEqual(tierScoreBadge(92, "to_hdd"), {
    label: "%92 Soğuk",
    level: "info",
  });
  assert.deepEqual(tierScoreBadge(88, "to_ssd"), {
    label: "%88 Sıcak",
    level: "ok",
  });
  assert.deepEqual(tierScoreBadge(120, "to_hdd"), {
    label: "%100 Soğuk",
    level: "info",
  });

  assert.equal(formatUntouchedDays(0), "Bugün kullanıldı");
  assert.equal(formatUntouchedDays(45), "45 gün dokunulmadı");
  assert.equal(formatUntouchedDays(null), "—");
});

test("findCompanionDrive selects best NTFS HDD for SSD and >=20 GiB NTFS SSD for HDD", () => {
  const disks = [
    {
      id: "nvme-1",
      model: "Samsung 980",
      type: "NVMe SSD",
      volumes: [
        { letter: "C", filesystem: "NTFS", free_bytes: 50 * 1024 ** 3 },
      ],
    },
    {
      id: "hdd-1",
      model: "WD Blue",
      type: "HDD",
      volumes: [
        { letter: "D", filesystem: "NTFS", free_bytes: 400 * 1024 ** 3 },
        { letter: "F", filesystem: "exFAT", free_bytes: 900 * 1024 ** 3 },
      ],
    },
    {
      id: "ssd-small",
      model: "Small SSD",
      type: "SATA SSD",
      volumes: [
        { letter: "G", filesystem: "NTFS", free_bytes: 10 * 1024 ** 3 },
      ],
    },
  ];
  const forSsd = findCompanionDrive(disks, disks[0]);
  assert.equal(forSsd?.letter, "D");
  assert.equal(forSsd?.role, "hdd_archive");

  const forHdd = findCompanionDrive(disks, disks[1]);
  assert.equal(forHdd?.letter, "C");
  assert.equal(forHdd?.role, "ssd_boost");

  // Only small SSD (< 20 GiB free) -> returns null honestly for HDD promotion
  assert.equal(findCompanionDrive([disks[1], disks[2]], disks[1]), null);
  // No HDD attached -> returns null honestly for SSD offloading
  assert.equal(findCompanionDrive([disks[0], disks[2]], disks[0]), null);
});

test("English locale formatting and helper translations", () => {
  assert.equal(format(1234.5, 1, "en"), "1,234.5");
  assert.equal(format(1234.5, 1, "tr"), "1.234,5");
  assert.ok(bytes(2 * 1024 ** 3, "en").includes("GB"));
  assert.equal(
    alignmentDescription(true, "en"),
    "All partitions start on a physical 4K sector boundary.",
  );
  assert.match(alignmentDescription(false, "en"), /misaligned/i);

  assert.equal(
    mediaStatusBadge({ media_status: "repaired", openable: true }, "en")?.label,
    "Auto-Repaired",
  );
  assert.equal(
    mediaStatusBadge({ media_status: "intact", openable: true }, "en")?.label,
    "Openable · Intact",
  );
  assert.equal(
    mediaStatusBadge({ media_status: "unrecoverable", openable: false }, "en")?.label,
    "Unopenable",
  );

  const benchEn = evaluateBenchmarkSpeed(3250, "NVMe SSD", "NVMe 1.3", null, "en");
  assert.equal(benchEn?.level, "ok");
  assert.match(benchEn?.label ?? "", /Normal|Fast|Typical/i);
  assert.match(benchEn?.detail ?? "", /NVMe|PCIe/i);

  assert.equal(slcPressureBadge("high", true, "en").label, "High — SLC Cache Shrinking");
  assert.equal(slcPressureBadge("low", false, "en").label, "Low Usage");
  assert.equal(tierCategoryBadge("video", false, "en").label, "Video");
  assert.equal(tierCategoryBadge("folder", true, "en").label, "Folder");
  assert.equal(tierScoreBadge(82, "to_ssd", "en").label, "82% Hot");
  assert.equal(tierScoreBadge(90, "to_hdd", "en").label, "90% Cold");
  assert.equal(formatUntouchedDays(0, "en"), "Used today");
  assert.equal(formatUntouchedDays(45, "en"), "45 days untouched");
});

test("aiNaturalFilter recognizes English semantic tokens", () => {
  const items = [
    {
      id: "1",
      name: "report.pdf",
      category: "document",
      source: "vss_shadow",
      confidence: 98,
      trim_zeroed: false,
      ai_summary: "PDF document intact",
    },
    {
      id: "2",
      name: "holiday.jpg",
      category: "photo",
      source: "recycle_bin",
      confidence: 88,
      trim_zeroed: false,
      ai_summary: "JPEG photo recoverable",
    },
    {
      id: "3",
      name: "elden.exe",
      category: "game_portable",
      source: "mft_resident",
      confidence: 42,
      trim_zeroed: true,
      ai_summary: "Zeroed blocks",
    },
  ];
  assert.deepEqual(
    aiNaturalFilter(items, "healthy", "all").map((x) => x.id),
    ["1", "2"],
  );
  assert.deepEqual(
    aiNaturalFilter(items, "photo", "all").map((x) => x.id),
    ["2"],
  );
  assert.deepEqual(
    aiNaturalFilter(items, "game", "all").map((x) => x.id),
    ["3"],
  );
});

test("canStart and canMaintain block disks with read_incomplete true", () => {
  const liveState = {
    source: "live",
    admin: true,
    busy: false,
    stale: false,
  };
  const incompleteDisk = {
    id: "incomplete-1",
    stable_identity: true,
    os_disk_number: 1,
    os_serial: "SN123456",
    os_model: "Demo NVMe",
    read_incomplete: true,
    volumes: [{ letter: "C", filesystem: "NTFS", read_only: false }],
  };
  const completeDisk = {
    id: "complete-1",
    stable_identity: true,
    os_disk_number: 1,
    os_serial: "SN123456",
    os_model: "Demo NVMe",
    read_incomplete: false,
    volumes: [{ letter: "C", filesystem: "NTFS", read_only: false }],
  };
  assert.equal(canStart(liveState, incompleteDisk), false);
  assert.equal(canMaintain(liveState, incompleteDisk), false);
  assert.equal(canStart(liveState, completeDisk), true);
  assert.equal(canMaintain(liveState, completeDisk), true);
});

test("computePollSignature changes when UI language toggles", () => {
  const sampleState = {
    source: "replay",
    admin: false,
    updating: false,
    stale: false,
    busy: false,
    error: null,
    updated_at: "2026-04-19T12:00:00Z",
    disks: [],
  };
  const sigTr = computePollSignature(sampleState, {
    page: "overview",
    selected: "disk-1",
    letter: "C",
    connectionError: "",
    lang: "tr",
    detailCounts: "0",
  });
  const sigEn = computePollSignature(sampleState, {
    page: "overview",
    selected: "disk-1",
    letter: "C",
    connectionError: "",
    lang: "en",
    detailCounts: "0",
  });
  assert.notEqual(sigTr, sigEn);
});

test("read_only_guard locks mutating controls while leaving read-only discovery available", () => {
  const lockedState = {
    source: "live",
    admin: true,
    busy: false,
    stale: false,
    read_only_guard: true,
  };
  const unlockedState = { ...lockedState, read_only_guard: false };
  const disk = {
    id: "nvme-1",
    stable_identity: true,
    os_disk_number: 1,
    os_serial: "SN987654",
    os_model: "NVMe Test",
    read_incomplete: false,
    volumes: [{ letter: "C", filesystem: "NTFS", read_only: false }],
  };
  assert.equal(isWriteLocked(lockedState), true);
  assert.equal(isWriteLocked(unlockedState), false);
  assert.equal(canWrite(lockedState), false);
  assert.equal(canWrite(unlockedState), true);
  assert.equal(canMaintain(lockedState, disk), false);
  assert.equal(canOptimizeSystem(lockedState), false);
  assert.equal(canStart(lockedState, disk), true);

  assert.equal(canMaintain(unlockedState, disk), true);
  assert.equal(canOptimizeSystem(unlockedState), true);

  const sigLocked = computePollSignature(lockedState, {
    page: "overview",
    selected: "nvme-1",
    letter: "C",
    connectionError: "",
  });
  const sigUnlocked = computePollSignature(unlockedState, {
    page: "overview",
    selected: "nvme-1",
    letter: "C",
    connectionError: "",
  });
  assert.notEqual(sigLocked, sigUnlocked);
});

test("dev workspace helpers format stacks, cache kinds, 8.3 status, env snippet, and aiNaturalFilter dev tokens", () => {
  assert.deepEqual(devStackBadge("Rust"), { label: "Rust", level: "warning" });
  assert.deepEqual(devStackBadge("Node / JS"), { label: "Node / JS", level: "info" });
  assert.deepEqual(devCacheKindBadge("ai_model", "tr"), {
    label: "Yerel LLM",
    level: "warning",
  });
  assert.deepEqual(devCacheKindBadge("pkg_cache", "en"), {
    label: "Pkg Cache",
    level: "info",
  });
  assert.equal(ntfs8dot3Label(1, "tr"), "Kapalı (Hızlı)");
  assert.equal(ntfs8dot3Label(2, "en"), "Per-volume (Default)");
  const snippet = devEnvSnippet("e:");
  assert.match(snippet, /E:\\DevCaches\\cargo/);
  assert.match(snippet, /OLLAMA_MODELS/);

  const devRows = [
    {
      name: ".env.local",
      category: "database_code",
      confidence: 100,
      extension: "local",
      original_path: "D:\\Projects\\vibe\\.env.local",
      ai_summary: "Ortam değişkenleri",
    },
    {
      name: "App.tsx",
      category: "database_code",
      confidence: 92,
      extension: "tsx",
      original_path: "D:\\Projects\\vibe\\src\\App.tsx",
      ai_summary: "React bileşeni",
    },
    {
      name: "tatil.jpg",
      category: "photo",
      confidence: 88,
      extension: "jpg",
      original_path: "D:\\Photos\\tatil.jpg",
      ai_summary: "Fotoğraf",
    },
  ];
  assert.deepEqual(
    aiNaturalFilter(devRows, "vibe").map((r) => r.name),
    [".env.local", "App.tsx"],
  );
  assert.deepEqual(
    aiNaturalFilter(devRows, ".env").map((r) => r.name),
    [".env.local"],
  );
  assert.deepEqual(
    aiNaturalFilter(devRows, "tsx").map((r) => r.name),
    ["App.tsx"],
  );
});
