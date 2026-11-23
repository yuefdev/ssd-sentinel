/** Tests production frontend bytes against a Rust-exported recording. No disk workloads. */
import { chromium, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import net from "node:net";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import path from "node:path";

async function freePort() {
  return await new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address();
      const port = typeof addr === "object" && addr ? addr.port : 0;
      srv.close(() => resolve(port));
    });
  });
}

const root = path.resolve(import.meta.dirname, "..");
const output = path.resolve(root, process.argv[2] || "results/tauri-ui");
await mkdir(output, { recursive: true });
const fixture = JSON.parse(
  await readFile(path.join(root, "results/rust-fixture.json"), "utf8"),
);
const measurement = {
  mb_s: 128,
  iops: 31250,
  latency_us: 32,
  seconds: 5,
  read_bytes: "640000000",
  write_bytes: "0",
};
const seqMeasurement = {
  mb_s: 3250,
  iops: 3250,
  latency_us: 2460,
  seconds: 5,
  read_bytes: "16250000000",
  write_bytes: "0",
};
fixture.state.admin = false;
fixture.forecasts = {};
fixture.checkpoints = {};
fixture.tierMoves = [];
fixture.tierAnalyses = {};
fixture.jobs = [
  {
    id: "saved-nvme-simulation",
    kind: "benchmark",
    disk_id: fixture.state.disks[1].id,
    model: "SIMULATION ONLY",
    letter: "E",
    status: "completed",
    phase: "Simulated saved result",
    progress: 100,
    completed: 2,
    total: 2,
    started_at: "2026-10-21T09:00:00Z",
    finished_at: "2026-10-21T09:00:10Z",
    error: null,
    cleanup_error: null,
    read_bytes: 16890000000,
    write_bytes: 0,
    prepared_bytes: 268435456,
    options: {
      profile: "nvme",
      mode: "read",
      size_mib: 256,
      repeats: 1,
      seconds: 5,
      interval: 0,
      data: "random",
      row: "all",
    },
    results: [
      {
        row: fixture.state.profiles.nvme.find(
          (row) => row.id === "SEQ1MiB-Q8T1",
        ),
        direction: "read",
        best: seqMeasurement,
        samples: [seqMeasurement],
      },
      {
        row: fixture.state.profiles.nvme.find(
          (row) => row.id === "RND4KiB-Q32T16",
        ),
        direction: "read",
        best: measurement,
        samples: [measurement],
      },
    ],
    blocks: [],
    damaged_files: [],
    summary: {},
  },
];
const previewPort = await freePort();
const previewUrl = `http://127.0.0.1:${previewPort}`;
const server = spawn(
  process.execPath,
  [
    path.join(root, "node_modules/vite/bin/vite.js"),
    "preview",
    "--host",
    "127.0.0.1",
    "--port",
    String(previewPort),
    "--strictPort",
  ],
  { cwd: root, stdio: "pipe" },
);
let logs = "";
let serverExited = null;
server.stdout.on("data", (b) => (logs += b));
server.stderr.on("data", (b) => (logs += b));
server.on("exit", (code) => {
  serverExited = code;
});
const errors = [],
  checks = [];
let browser;
try {
  for (let i = 0; i < 80; i++) {
    if (serverExited !== null)
      throw new Error(`Preview server exited (${serverExited}): ${logs}`);
    try {
      const r = await fetch(previewUrl);
      if (r.ok) break;
    } catch {}
    if (i === 79) throw new Error(logs);
    await new Promise((r) => setTimeout(r, 100));
  }
  browser = await chromium.launch({ channel: "msedge", headless: true });
  const context = await browser.newContext({
    viewport: { width: 1400, height: 1000 },
    deviceScaleFactor: 1,
    reducedMotion: "reduce",
    locale: "tr-TR",
  });
  await context.addInitScript((data) => {
    window.__fixture = data;
    window.__calls = [];
    window.__TAURI_INTERNALS__ = {
      invoke: async (cmd, args = {}) => {
        window.__calls.push({ cmd, args });
        if (cmd === "get_state") return structuredClone(window.__fixture.state);
        if (cmd === "get_history")
          return structuredClone(window.__fixture.histories[args.diskId] || []);
        if (cmd === "get_events") return [];
        if (cmd === "get_jobs")
          return structuredClone(
            window.__fixture.jobs.filter(
              (j) => j.kind === args.kind && j.disk_id === args.diskId,
            ),
          );
        if (cmd === "refresh") return null;
        if (cmd === "set_preferences") {
          if (args.language) window.__fixture.state.language = args.language;
          if (args.healthMode)
            window.__fixture.state.health_mode = args.healthMode;
          if (typeof args.minimiseToTray === "boolean")
            window.__fixture.state.minimise_to_tray = args.minimiseToTray;
          if (typeof args.readOnlyGuard === "boolean")
            window.__fixture.state.read_only_guard = args.readOnlyGuard;
          return null;
        }
        if (cmd === "save_report") return { saved: true };
        if (cmd === "get_optimizations")
          return structuredClone(window.__fixture.optimizations || []);
        if (cmd === "optimize_action")
          return { status: "completed", message: "Simulated only" };
        if (cmd === "restart_as_admin")
          return { relaunching: false, simulated: true };
        if (cmd === "check_updates")
          return structuredClone(
            window.__fixture.mockUpdate || {
              repo: "yuefdev/ssd-sentinel",
              current: "1.10.0",
              status: "up_to_date",
              latest: "v1.10.0",
              url: "https://github.com/yuefdev/ssd-sentinel/releases/tag/v1.10.0",
              download_url: null,
              asset_name: null,
              asset_size: null,
              sha256_url: null,
              notes: null,
              message: "Güncel sürümü kullanıyorsunuz.",
              message_en: "You are using the latest version.",
            },
          );
        if (cmd === "open_release_url") return null;
        if (cmd === "download_and_launch_update")
          return { launched: true, verified: true, path: "C:\\Temp\\setup.exe" };
        if (cmd === "list_tier_moves")
          return structuredClone(window.__fixture.tierMoves || []);
        if (cmd === "analyze_storage_tiering") {
          const pre =
            window.__fixture.tierAnalyses &&
            window.__fixture.tierAnalyses[args.diskId];
          if (pre) {
            const out = structuredClone(pre);
            out.active_moves = structuredClone(
              window.__fixture.tierMoves || out.active_moves || [],
            );
            return out;
          }
          const d = (window.__fixture.state.disks || []).find(
            (x) => x.id === args.diskId,
          );
          const isSsd = !!(d && d.type !== "HDD" && d.type.includes("SSD"));
          return {
            disk_id: args.diskId,
            letter: args.letter || "",
            disk_type: d ? d.type : "SSD",
            is_ssd: isSsd,
            replay: window.__fixture.state.source === "replay",
            fill_percent: 0,
            free_bytes: 0,
            size_bytes: 0,
            slc_pressure: "low",
            slc_note_tr:
              "Kayıt modunda canlı doluluk ve SLC analizi yapılmaz.",
            extreme_fragmented_count: 0,
            worst_extents: 0,
            companion_role: null,
            companion: null,
            total_reclaimable_bytes: 0,
            candidates: [],
            active_moves: structuredClone(window.__fixture.tierMoves || []),
            summary_tr:
              "Kayıt modunda canlı klasör taraması kapalıdır; yalnızca disk eşleştirme ve kayıtlı taşımalar gösterilir.",
          };
        }
        if (cmd === "relocate_tier_item") {
          const rec = {
            id: `move-${Date.now()}`,
            source_path: args.sourcePath,
            target_path: `${args.targetLetter}:\\SSD-Sentinel-Arsiv\\${args.sourcePath.replace(/^[A-Za-z]:\\/, "")}`,
            source_letter: (args.sourcePath || "C")[0].toUpperCase(),
            target_letter: args.targetLetter,
            is_dir: !/\.[a-z0-9]{2,4}$/i.test(args.sourcePath || ""),
            junction_created: !/\.[a-z0-9]{2,4}$/i.test(args.sourcePath || ""),
            size_bytes: 16_000_000_000,
            file_count: 24,
            direction: args.direction || "to_hdd",
            moved_at: "2026-11-21T11:20:00Z",
          };
          window.__fixture.tierMoves = [
            rec,
            ...(window.__fixture.tierMoves || []),
          ];
          window.__fixture.state.tier_moves = structuredClone(
            window.__fixture.tierMoves,
          );
          return structuredClone(rec);
        }
        if (cmd === "rollback_tier_move") {
          window.__fixture.tierMoves = (
            window.__fixture.tierMoves || []
          ).filter((m) => m.id !== args.moveId);
          window.__fixture.state.tier_moves = structuredClone(
            window.__fixture.tierMoves,
          );
          return {
            rolled_back: true,
            id: args.moveId,
            message_tr: "Öğe SHA-256 doğrulaması ile orijinal konumuna geri alındı.",
          };
        }
        if (cmd === "analyze_dev_workspace") {
          const pre =
            window.__fixture.devAnalyses &&
            window.__fixture.devAnalyses[args.diskId];
          if (pre) return structuredClone(pre);
          return {
            disk_id: args.diskId,
            letter: args.letter || "",
            replay: window.__fixture.state.source === "replay",
            scanned_roots: [],
            scan_ms: 0,
            truncated: false,
            total_artifact_bytes: 0,
            stale_artifact_bytes: 0,
            total_cache_bytes: 0,
            total_vhdx_bytes: 0,
            total_reclaimable_bytes: 0,
            offload_target: null,
            projects: [],
            global_caches: [],
            vhdx_files: [],
            summary_tr:
              "Kayıt modunda canlı proje taraması kapalıdır; canlı bir makinede proje ve önbellek analizi etkinleşir.",
            summary_en:
              "Live project scanning is disabled in snapshot replay; switch to a live machine to analyze projects and caches.",
          };
        }
        if (cmd === "clean_dev_artifact")
          return {
            status: "completed",
            path: args.artifactPath,
            freed_bytes: 1_500_000_000,
            removed_files: 4200,
            root_removed: true,
            message_tr: `${args.artifactPath} temizlendi: 1,4 GiB serbest bırakıldı.`,
            message: `${args.artifactPath} cleaned: 1.4 GiB freed.`,
          };
        if (cmd === "pick_dev_folder") return "C:\\Users\\Demo\\Projects";
        if (cmd === "get_health_forecast")
          return structuredClone(
            (window.__fixture.forecasts &&
              window.__fixture.forecasts[args.diskId]) || {
              ready: false,
              sample_count: 1,
              span_hours: 0,
              daily_written_gb: null,
              estimated_remaining_years: null,
              pending_delta_7d: null,
              reallocated_delta_7d: null,
              uncorrectable_delta_7d: null,
              temp_avg_7d: null,
              risk_level: "ok",
              summary_tr:
                "Öngörü için en az 1 saat aralıklı iki yerel ölçüm bekleniyor.",
              advice_tr:
                "Uygulama arka planda ölçüm biriktirdikçe tahmin burada görünür.",
            },
          );
        if (cmd === "get_surface_checkpoint")
          return structuredClone(
            (window.__fixture.checkpoints &&
              window.__fixture.checkpoints[args.diskId]) ||
              null,
          );
        if (cmd === "clear_surface_checkpoint") {
          if (window.__fixture.checkpoints) {
            delete window.__fixture.checkpoints[args.diskId];
          }
          return null;
        }
        if (cmd === "preview_recovered_item") {
          if (args.itemId === "rec-photo") {
            return {
              kind: "image",
              mime: "image/png",
              data_url:
                "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
              size_bytes: 68,
              repaired: true,
              repair_note:
                "Eksik JPEG sonu (EOI) onarıldı; kurtarılan bölüm açılabilir hale getirildi.",
              media_status: "repaired",
              width: 1920,
              height: 1080,
            };
          }
          if (args.itemId === "rec-2") {
            return {
              kind: "text",
              text: "Fatura No: 2026-11-18\nToplam: 1.250 TL",
              truncated: false,
              size_bytes: 40,
            };
          }
          if (args.itemId === "rec-env") {
            return {
              kind: "text",
              text: "NODE_ENV=development\nOPENAI_API_KEY=••••••••\nDATABASE_URL=••••••••",
              raw_text:
                "NODE_ENV=development\nOPENAI_API_KEY=sk-demo-12345\nDATABASE_URL=postgres://dev:pw@localhost/app",
              secrets_masked: 2,
              truncated: false,
              size_bytes: 95,
            };
          }
          return {
            kind: "binary",
            metadata: {
              product_name: "Spotify",
              publisher: "Spotify AB",
              sub_type: "64-bit NSIS Kurulum Paketi",
              size_bytes: 94371840,
              source: "vss_shadow",
              status_label: "VSS Gölge Kopya (%100 Sağlam · TRIM Korumalı)",
            },
          };
        }
        if (cmd === "restore_recovered_batch")
          return {
            saved: true,
            cancelled: false,
            restored: (args.itemIds || []).length,
            failed: 0,
            skipped: 0,
            dest_dir: "E:\\Kurtarilan",
            paths: (args.itemIds || []).map(
              (id) => `E:\\Kurtarilan\\${id}.bin`,
            ),
            errors: [],
            repaired: true,
            repaired_count: 1,
            repair_note: "1 hasarlı medya dosyası açılabilir hale getirildi.",
          };
        if (cmd === "restore_recovered_item")
          return { saved: true, path: "E:\\Kurtarilan\\test.bin", bytes: 4096 };
        if (cmd === "disk_action")
          return { status: "read", logs: {}, note: "Recorded fixture" };
        if (cmd === "start_job") return { id: "mock-only" };
        if (cmd === "cancel_job") {
          window.__fixture.state.job.status = "cancelled";
          return window.__fixture.state.job;
        }
        throw new Error(`Unexpected command ${cmd}`);
      },
    };
  }, fixture);
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (msg) => {
    if (msg.type() === "error") errors.push(msg.text());
  });
  page.on("response", (response) => {
    if (response.status() >= 400)
      errors.push(`HTTP ${response.status()} ${response.url()}`);
  });
  await page.goto(previewUrl);
  await expect(page.locator(".disk-tile")).toHaveCount(3);
  await expect(page.locator(".nav-badge")).toHaveCount(0);
  await expect(page.locator(".replay-banner")).toBeVisible();
  checks.push(
    "Rust-exported recording renders three disks and explicit replay status",
  );
  await page.locator(".disk-tile").nth(1).click();
  await expect(page.locator(".health-ring strong")).toContainText("73");
  await expect(page.locator(".forecast-card")).toBeVisible();
  await expect(page.locator(".forecast-placeholder")).toBeVisible();
  await expect(page.locator(".device-facts")).toContainText("NVMe 1.3");
  const themeBtnBox = await page.locator('[data-action="theme"]').boundingBox();
  if (!themeBtnBox || themeBtnBox.y + themeBtnBox.height > 1000) {
    throw new Error(
      `Sidebar theme button clipped at 1000px: ${JSON.stringify(themeBtnBox)}`,
    );
  }
  await expect(
    page.locator(".overview-quick-actions .quick-hero-card"),
  ).toHaveCount(3);
  await expect(
    page.locator(".overview-quick-actions .guide-step.recommended"),
  ).toHaveCount(1);
  await page.locator('.overview-quick-actions [data-goto="recovery"]').click();
  await expect(page.locator('[data-action="start-recovery"]')).toBeDisabled();
  await page.locator('[data-page="overview"]').click();
  await page.screenshot({
    path: path.join(output, "overview-dark.png"),
    fullPage: true,
  });
  checks.push(
    "Overview shows 3-step start guide, honest health forecast placeholder, NVMe 1.3, and unclipped sidebar",
  );
  checks.push("NVMe captured health is 73%, no fabricated history points");
  await expect(page.locator(".line-chart circle")).toHaveCount(1);
  await page.locator('[data-page="smart"]').first().click();
  await expect(page.locator(".smart-table tbody tr")).not.toHaveCount(0);
  await expect(page.locator('[data-action="advanced"]')).toHaveCount(0);
  await expect(page.locator(".smart-table th")).toHaveCount(3);
  await page.locator("#smart-search").fill("dayanım");
  await expect(page.locator(".smart-table tbody tr")).toHaveCount(1);
  await expect(page.locator("#smart-search")).toBeFocused();
  await page.locator("#smart-search").fill("");
  await page.locator(".disk-tile").first().click();
  await expect(page.locator('[data-action="advanced"]')).toBeVisible();
  await page.locator('[data-action="advanced"]').click();
  await expect(page.locator(".smart-table th")).toHaveCount(6);
  await page.screenshot({
    path: path.join(output, "smart-dark.png"),
    fullPage: true,
  });
  await page.locator(".disk-tile").nth(1).click();
  checks.push(
    "SMART hides empty normalized columns on NVMe, keeps advanced columns on SATA HDD, and preserves Turkish search focus",
  );
  await page.locator('[data-page="performance"]').click();
  await expect(page.locator('[data-bench-preset="quick"]')).toBeVisible();
  await expect(page.locator('[data-bench-preset="full"]')).toBeVisible();
  await expect(page.locator('[data-action="start-benchmark"]')).toBeDisabled();
  await expect(page.locator(".score strong").first()).toHaveText("—");
  checks.push(
    "Performance shows 2 simplified presets, disables replay speed test, and keeps unmeasured scores unknown",
  );
  await page.screenshot({
    path: path.join(output, "performance-dark.png"),
    fullPage: true,
  });
  await expect(page.locator("#bench-profile")).toHaveValue("standard");
  await page.locator('[data-view-job="saved-nvme-simulation"]').click();
  await expect(page.locator("#bench-profile")).toHaveValue("nvme");
  await expect(page.locator(".bench-verdict")).toContainText("3.250");
  await expect(
    page
      .locator('[data-bench-row="RND4KiB-Q32T16"]')
      .locator("..")
      .locator(".score strong")
      .first(),
  ).toHaveText("128");
  checks.push(
    "Inspecting a saved non-default benchmark restores its profile, measured score, and disk-class verdict",
  );
  await page.locator('[data-page="surface"]').click();
  await expect(page.locator(".surface-stepper .surface-step")).toHaveCount(3);
  await expect(page.locator(".block")).toHaveCount(200);
  await expect(page.locator('[data-action="start-surface"]')).toBeDisabled();
  checks.push(
    "Surface page renders 3-step stepper and leaves 200 unmeasured cells in replay",
  );
  await page.screenshot({
    path: path.join(output, "surface-dark.png"),
    fullPage: true,
  });
  await page.locator('[data-page="history"]').first().click();
  await expect(page.locator(".event-card")).toContainText(
    "Henüz yeni bir olay yok",
  );
  checks.push("History initial baseline is not reported as a new failure");
  await page.locator('[data-page="tools"]').click();
  await expect(page.locator('[data-action="short_test"]')).toBeDisabled();
  await expect(page.locator('[data-action="retrim"]')).toBeDisabled();
  await page.locator('[data-action="logs"]').click();
  await expect(page.locator("#confirmation")).toBeVisible();
  await page.locator("#close-result").click();
  checks.push("Maintenance is guarded in replay, recorded logs can be read");
  await page.locator('[data-page="optimize"]').click();
  await expect(page.locator("#smart-tiering-card")).toBeVisible();
  await expect(page.locator("main")).not.toContainText("SSD'de Engelli");
  await expect(
    page.locator('[data-tier-job="ssd_extent_optimize"]'),
  ).toBeDisabled();
  const diagStripBox = await page.locator(".opt-diag-grid").boundingBox();
  const tierCardBox = await page.locator("#smart-tiering-card").boundingBox();
  const manualBox = await page.locator("details.manual-controls").boundingBox();
  if (
    !diagStripBox ||
    !tierCardBox ||
    !manualBox ||
    tierCardBox.y < diagStripBox.y ||
    tierCardBox.y >= manualBox.y
  ) {
    throw new Error(
      `Expected #smart-tiering-card between .opt-diag-grid and details.manual-controls: diag=${JSON.stringify(diagStripBox)} tier=${JSON.stringify(tierCardBox)} manual=${JSON.stringify(manualBox)}`,
    );
  }
  await expect(page.locator('[data-opt-action="ntfs_trim"]')).toBeDisabled();
  await expect(page.locator(".opt-diag-grid")).toContainText("NVMe 1.3");
  await expect(page.locator(".opt-diag-grid")).toContainText(
    "Windows bölüm eşlemesi olmadan 4K hizalama doğrulanamaz.",
  );
  await page.screenshot({
    path: path.join(output, "optimize-dark.png"),
    fullPage: true,
  });
  checks.push(
    "Optimize page renders prominent Smart Tiering card (no dead 'SSD'de Engelli' box), NVMe 1.3, accurate 4K copy, and locks controls in replay mode",
  );
  await page.locator('[data-action="theme"]').click();
  await page.locator('[data-page="overview"]').click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.screenshot({
    path: path.join(output, "overview-light.png"),
    fullPage: true,
  });
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  checks.push("Theme survives reload");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator(".disk-tile").nth(1).click();
  await page.screenshot({
    path: path.join(output, "overview-narrow.png"),
    fullPage: true,
  });
  for (const pageName of [
    "overview",
    "smart",
    "performance",
    "optimize",
    "surface",
    "recovery",
    "history",
    "tools",
  ]) {
    await page.locator(`[data-page="${pageName}"]`).first().click();
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    );
    if (overflow) throw new Error(`Horizontal document overflow: ${pageName}`);
  }
  checks.push(
    "All eight screens fit a 390px document; tables have scoped scrolling",
  );
  await page.locator('[data-page="surface"]').click();
  await page.screenshot({
    path: path.join(output, "surface-narrow.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 1400, height: 1000 });
  // A live fixture exists only in this test bridge; no real command runs.
  await page.evaluate(() => {
    const s = window.__fixture.state;
    s.source = "live";
    s.stale = false;
    s.busy = false;
    const d = s.disks[0];
    d.os_disk_number = 0;
    d.os_serial = d.serial;
    d.os_model = d.model;
    d.volumes = [
      {
        letter: "D",
        label: "Simulated only",
        filesystem: "NTFS",
        free_bytes: 100e9,
        size_bytes: 200e9,
      },
      {
        letter: "E",
        label: "Simulated extra",
        filesystem: "NTFS",
        free_bytes: 240e9,
        size_bytes: 500e9,
      },
    ];
    window.__fixture.forecasts[d.id] = {
      ready: true,
      sample_count: 14,
      span_hours: 168.0,
      daily_written_gb: 24.5,
      estimated_remaining_years: 8.4,
      pending_delta_7d: 0,
      reallocated_delta_7d: 0,
      uncorrectable_delta_7d: 0,
      temp_avg_7d: 36.2,
      risk_level: "ok",
      summary_tr: "Son 7 günde hata sayacı artışı yok; disk kararlı.",
      advice_tr: "Düzenli yedekleme alışkanlığınızı koruyun.",
    };
    window.__fixture.checkpoints[d.id] = {
      disk_id: d.id,
      letter: "D",
      next_zone: 60,
      total_zones: 200,
      progress_percent: 30,
      done_bytes: 300e9,
      disk_bytes: 1000e9,
      elapsed_seconds: 1200,
      slow_chunks: 4,
      error_chunks: 1,
      targets_count: 2,
      updated_at: "2026-11-18T17:30:00Z",
    };
    s.job = {
      id: "simulated-job",
      kind: "surface",
      disk_id: d.id,
      model: d.model,
      letter: "D",
      status: "running",
      phase: "Simulated only",
      progress: 40,
      completed: 80,
      total: 200,
      options: { mode: "physical_scan", size_mib: 64 },
      blocks: Array.from({ length: 80 }, (_, i) => ({
        index: i,
        status: i === 7 ? "bad" : i === 20 ? "warning" : "fast",
        latency_ms: i === 7 ? null : i === 20 ? 110 : 1.2,
        mb_s: i === 7 ? null : 185 - i,
        offset_bytes: i * 4096,
        bytes: 4096,
        file: "fixture-only.bin",
        error: i === 7 ? "Simulated CRC error" : null,
      })),
      results: [],
      summary: {},
      damaged_files: [],
      repair_summary: {
        mode: "physical_scan",
        recovered_pending: 2,
        isolated_clusters: 4,
        isolated_bytes: 16384,
        thermal_pauses: 0,
        zbr_profile: {
          outer_mb_s: 185.0,
          inner_mb_s: 106.0,
          drop_percent: 42.7,
          tail_degraded: true,
          suggested_tail_guard_pct: 5,
        },
        smart_before: {
          pending_197: 2,
          reallocated_5: 0,
          uncorrectable_198: 0,
        },
        smart_after: {
          pending_197: 0,
          reallocated_5: 0,
          uncorrectable_198: 0,
        },
        bad_lbas: [
          { lba: 1953520, offset_bytes: 1000202240, latency_ms: 145 },
        ],
        slow_threshold_ms: 68.4,
        median_chunk_ms: 8.5,
        diagnosis: {
          score: 78,
          verdict: "repairable",
          title: "Onarılabilir sektörler bulundu",
          advice:
            "1 okunamayan ve 1 zayıf sektör dağınık halde. Nokta atışı onarım bunları yeniden yazıp diskin yedek sektörlere aktarmasını sağlar.",
          recommended_mode: "magnetic_refresh",
          unreadable: 1,
          weak: 1,
          slow: 2,
          dead_zones: 0,
          engine: "Deterministik yüzey teşhisi · istatistik + kural motoru · çevrimdışı",
        },
      },
      error: null,
      cleanup_error: null,
    };
    window.__fixture.jobs.push(
      {
        id: "sim-fullscan-1",
        kind: "surface",
        disk_id: d.id,
        model: d.model,
        letter: "D",
        status: "completed",
        phase: "Tamamlandı",
        progress: 100,
        completed: 200,
        total: 200,
        started_at: "2026-11-18T18:00:00Z",
        finished_at: "2026-11-18T19:45:00Z",
        options: { mode: "physical_scan", size_mib: "auto" },
        blocks: [],
        results: [],
        summary: {},
        damaged_files: [],
        repair_summary: {
          mode: "physical_scan",
          full_surface: true,
          slow_threshold_ms: 68.4,
          median_chunk_ms: 8.5,
          targets: [
            {
              offset: 1000202240,
              len: 4096,
              kind: "weak_sector",
              latency_ms: 145.0,
            },
            {
              offset: 2000404480,
              len: 4096,
              kind: "unreadable",
              latency_ms: null,
            },
          ],
          diagnosis: {
            score: 78,
            verdict: "repairable",
            title: "Onarılabilir sektörler bulundu",
            advice:
              "1 okunamayan ve 1 zayıf sektör dağınık halde. Nokta atışı onarım bunları yeniden yazıp diskin yedek sektörlere aktarmasını sağlar.",
            recommended_mode: "magnetic_refresh",
            unreadable: 1,
            weak: 1,
            slow: 2,
            dead_zones: 0,
            engine:
              "Deterministik yüzey teşhisi · istatistik + kural motoru · çevrimdışı",
          },
        },
      },
      {
        id: "sim-pinpoint-1",
        kind: "surface",
        disk_id: d.id,
        model: d.model,
        letter: "D",
        status: "completed",
        phase: "Tamamlandı",
        progress: 100,
        completed: 2,
        total: 2,
        started_at: "2026-11-18T20:00:00Z",
        finished_at: "2026-11-18T20:01:12Z",
        options: { mode: "magnetic_refresh", size_mib: "auto" },
        blocks: [
          {
            index: 0,
            status: "fast",
            latency_ms: 142.0,
            mb_s: null,
            offset_bytes: 1000202240,
            bytes: 4096,
            file: "LBA 1953520 · Zayıf sektör · Onarıldı",
            error: null,
          },
          {
            index: 1,
            status: "warning",
            latency_ms: null,
            mb_s: null,
            offset_bytes: 2000404480,
            bytes: 4096,
            file: "LBA 3907040 · Okunamayan sektör · Onarılamadı · kapatıldı",
            error: null,
          },
        ],
        results: [],
        summary: {},
        damaged_files: [],
        repair_summary: {
          mode: "magnetic_refresh",
          engine: "pinpoint",
          scan_finished_at: "2026-11-18T19:45:00Z",
          targets_total: 2,
          regions: 2,
          outside_volume: 0,
          healed: 1,
          quarantined_runs: 1,
          unplaceable: 0,
          relocated_files: 1,
          relocated_bytes: 4194304,
          isolated_clusters: 4,
          isolated_bytes: 16384,
          data_loss_files: 0,
          thermal_pauses: 0,
          recovered_pending: 1,
          speed: { before_avg_ms: 142.0, after_avg_ms: 5.2, measured: 2 },
          smart_before: { pending_197: 1, reallocated_5: 0, uncorrectable_198: 0 },
          smart_after: { pending_197: 0, reallocated_5: 1, uncorrectable_198: 0 },
          advice: [
            "Bazı bozuk sektörler kümelenmiş; bu bölgeler otomatik kapatıldı.",
          ],
          repairs: [
            {
              lba: 1953520,
              offset_bytes: 1000202240,
              bytes: 4096,
              kind: "weak_sector",
              kind_label: "Zayıf sektör",
              dead_zone: false,
              action: "heal",
              result: "Onarıldı",
              status: "ok",
              before_ms: 142.0,
              after_ms: 5.2,
              moved_files: ["Videos\\clip.mp4"],
              moved_count: 1,
            },
            {
              lba: 3907040,
              offset_bytes: 2000404480,
              bytes: 4096,
              kind: "unreadable",
              kind_label: "Okunamayan sektör",
              dead_zone: true,
              action: "isolate",
              result: "Kalıcı olarak kapatıldı",
              status: "warning",
              before_ms: null,
              after_ms: null,
            },
          ],
          scope:
            "Nokta atışı sektör onarımı: 2 hedef bölge · 1 onarıldı · 1 dosya taşındı · 1 kapatıldı.",
        },
      },
    );
  });
  await page.locator(".disk-tile").first().click();
  await expect(page.locator(".active-job-bar")).toBeVisible();
  await expect(page.locator(".surface-stepper .surface-step")).toHaveCount(3);
  await expect(page.locator(".zbr-card")).toBeVisible();
  await expect(page.locator(".ai-diagnosis-card")).toContainText(
    "Onarılabilir sektörler bulundu",
  );
  const diagBox = await page.locator(".ai-diagnosis-card").boundingBox();
  const mapBox = await page.locator(".block-map-card").boundingBox();
  if (!diagBox || !mapBox || diagBox.y >= mapBox.y) {
    throw new Error(
      `Expected AI diagnosis card above block map: diag=${JSON.stringify(diagBox)} map=${JSON.stringify(mapBox)}`,
    );
  }
  await expect(
    page.locator('[data-recommended-surface="magnetic_refresh"]'),
  ).toBeVisible();
  await expect(page.locator(".heal-summary")).toContainText("2 → 0");
  await expect(
    page.locator('input[name="scan-mode"][value="magnetic_refresh"]'),
  ).toBeEnabled();
  await expect(page.locator("#scan-size")).toHaveCount(0);
  await expect(page.locator(".auto-budget")).toContainText("Otomatik");
  await page
    .locator('input[name="scan-mode"][value="physical_scan"]')
    .check();
  await expect(page.locator(".auto-budget")).toContainText("tamamı");
  await page
    .locator('input[name="scan-mode"][value="magnetic_refresh"]')
    .check();
  await expect(page.locator(".auto-budget")).toContainText("hedef bölge hazır");
  await page.locator('[data-block="7"]').click();
  await expect(page.locator(".block-inspector")).toContainText(
    "Simulated CRC error",
  );
  await page.screenshot({
    path: path.join(output, "surface-simulated-result.png"),
    fullPage: true,
  });
  await page.locator(".ai-diagnosis-card").scrollIntoViewIfNeeded();
  await page.screenshot({
    path: path.join(output, "surface-ai-diagnosis.png"),
    fullPage: true,
  });
  await page.locator('[data-view-job="sim-pinpoint-1"]').click();
  await expect(page.locator(".pinpoint-speed")).toContainText(
    "142 ms → Sonra: 5,2 ms",
  );
  await expect(page.locator(".pinpoint-speed")).toContainText("%96 daha hızlı");
  await expect(page.locator(".pinpoint-table")).toContainText("Videos\\clip.mp4");
  await expect(page.locator(".pinpoint-advice")).toContainText("kapatıldı");
  await page.locator(".pinpoint-summary").scrollIntoViewIfNeeded();
  await page.screenshot({
    path: path.join(output, "surface-pinpoint-result.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  const pinpointOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  if (pinpointOverflow)
    throw new Error("Horizontal document overflow on pinpoint summary at 390px");
  await page.setViewportSize({ width: 1400, height: 1000 });
  await page.locator(".disk-tile").nth(1).click();
  await expect(
    page.locator('input[name="scan-mode"][value="magnetic_refresh"]'),
  ).toHaveCount(1);
  await expect(
    page.locator('input[name="scan-mode"][value="bad_zone_isolate"]'),
  ).toHaveCount(0);
  await expect(page.locator(".mode-note")).toBeVisible();
  checks.push(
    "AI surface diagnosis, pinpoint HDD/SSD repair results, ZBR curve, and HDD-only isolate lock verified",
  );
  await expect(page.locator(".active-job-bar")).toContainText("WDC");
  await page.locator('[data-action="cancel-job"]').click();
  await expect(page.locator(".active-job-bar")).toHaveCount(0);
  checks.push(
    "Active job retains original disk, error inspector and cancellation work",
  );
  await page.locator(".disk-tile").first().click();
  await expect(
    page.locator('.toolbar [data-action="elevate"]'),
  ).toBeVisible();
  await page.locator('[data-page="performance"]').click();
  await expect(
    page.locator('#bench-profile option[value="sustained"]'),
  ).toHaveCount(1);
  await expect(
    page.locator('[data-action="toggle-write-guard"]'),
  ).toBeVisible();
  await expect(
    page.locator('[data-action="toggle-write-guard"]'),
  ).toContainText("Salt-Okunur");
  await expect(page.locator('[data-action="start-benchmark"]')).toBeDisabled();
  await page.locator('[data-action="toggle-write-guard"]').click();
  await expect(page.locator("#confirmation .dry-run-box")).toBeVisible();
  await page.locator("#confirm-run").click();
  await expect(
    page.locator('[data-action="toggle-write-guard"]'),
  ).toContainText("Yazma Açık");
  await expect(page.locator('[data-action="start-benchmark"]')).toBeEnabled();
  await page.locator('[data-action="start-benchmark"]').click();
  await expect(page.locator("#confirmation")).toContainText("Salt okuma");
  await expect(page.locator("#confirmation")).toContainText("toplam yazma");
  await expect(page.locator("#confirmation .dry-run-box")).toBeVisible();
  await expect(page.locator("#confirmation .dry-run-box")).toContainText(
    "Win32 IOCP Direct-I/O",
  );
  await page.locator("#confirm-cancel").click();
  const starts = await page.evaluate(
    () => window.__calls.filter((c) => c.cmd === "start_job").length,
  );
  if (starts !== 0) throw new Error("Cancelled confirmation dispatched a job");
  await page.evaluate(() => {
    window.__fixture.state.admin = true;
    window.__fixture.state.disks[0].type = "NVMe SSD";
  });
  await page.locator('[data-page="overview"]').click();
  await expect(page.locator(".forecast-metrics")).toContainText("24,5 GB/gün");
  await expect(page.locator(".forecast-metrics")).toContainText("~8,4 yıl");
  checks.push(
    "Overview renders ready predictive health & lifespan forecast metrics from real history schema",
  );
  await page.locator('[data-page="surface"]').click();
  await expect(page.locator(".resume-banner")).toBeVisible();
  await expect(page.locator(".resume-banner")).toContainText("%30");
  await page.locator('[data-action="resume-surface"]').click();
  await expect(page.locator("#confirmation")).toContainText("kaldığı yerden");
  await page.locator("#confirm-cancel").click();
  await page.locator('[data-action="clear-surface-checkpoint"]').click();
  await expect(page.locator(".resume-banner")).toHaveCount(0);
  const clearCpCalls = await page.evaluate(
    () =>
      window.__calls.filter((c) => c.cmd === "clear_surface_checkpoint").length,
  );
  if (clearCpCalls < 1)
    throw new Error("clear_surface_checkpoint IPC was not called");
  checks.push(
    "Surface checkpoint resume banner confirms resume:true and clears cleanly",
  );
  await page.locator('[data-page="optimize"]').click();
  await expect(page.locator("#autopilot-volume")).toBeVisible();
  await page.locator("#autopilot-volume").selectOption("E");
  await page.locator("#autopilot-volume").selectOption("D");
  await expect(page.locator('[data-action="start-autopilot"]')).toBeEnabled();
  await page.locator('[data-action="start-autopilot"]').click();
  await expect(page.locator("#confirmation")).toBeVisible();
  await page.locator("#confirm-cancel").click();
  await expect(page.locator('[data-opt-action="ntfs_trim"]')).toBeHidden();
  await page.locator(".manual-controls > summary").click();
  await expect(page.locator('[data-opt-action="ntfs_trim"]')).toBeEnabled();
  await page.locator('[data-opt-action="ntfs_trim"]').click();
  await expect(page.locator("#confirmation")).toBeVisible();
  await page.locator("#confirm-cancel").click();
  const optCalls = await page.evaluate(
    () => window.__calls.filter((c) => c.cmd === "optimize_action").length,
  );
  if (optCalls !== 0)
    throw new Error("Cancelled optimize confirmation dispatched an action");
  checks.push(
    "Elevate button, inline Autopilot volume picker, 1-click Autopilot confirmation, and explicit optimize consent verified",
  );
  // v1.8.0 AI Smart Storage Tiering & Hot/Cold Optimization (both NVMe SSD and HDD)
  await page.evaluate(() => {
    const s = window.__fixture.state;
    const hdd = s.disks[0];
    const ssd = s.disks[1];
    hdd.type = "HDD";
    ssd.os_disk_number = 1;
    ssd.os_serial = ssd.serial;
    ssd.os_model = ssd.model;
    ssd.volumes = [
      {
        letter: "C",
        label: "Sistem NVMe",
        filesystem: "NTFS",
        free_bytes: 72e9,
        size_bytes: 512e9,
      },
    ];
    window.__fixture.tierMoves = [
      {
        id: "move-sim-1",
        source_path: "C:\\Users\\Demo\\Videos\\EskiKayitlar",
        target_path:
          "E:\\SSD-Sentinel-Arsiv\\C\\Users\\Demo\\Videos\\EskiKayitlar",
        source_letter: "C",
        target_letter: "E",
        is_dir: true,
        junction_created: true,
        size_bytes: 9_663_676_416,
        file_count: 18,
        direction: "to_hdd",
        moved_at: "2026-11-20T18:15:00Z",
      },
    ];
    s.tier_moves = structuredClone(window.__fixture.tierMoves);
    window.__fixture.jobs.push({
      id: "sim-smart-tier-ssd",
      kind: "smart_tier",
      disk_id: ssd.id,
      model: ssd.model,
      letter: "C",
      status: "completed",
      phase: "Tamamlandı",
      progress: 100,
      completed: 14,
      total: 14,
      started_at: "2026-11-21T09:30:00Z",
      finished_at: "2026-11-21T09:30:42Z",
      error: null,
      cleanup_error: null,
      read_bytes: 0,
      write_bytes: 671_088_640,
      prepared_bytes: 0,
      options: { mode: "ssd_extent_optimize" },
      results: [],
      blocks: [],
      summary: {},
      damaged_files: [],
      repair_summary: {
        mode: "ssd_extent_optimize",
        files_consolidated: 14,
        extents_before: 1120,
        extents_after: 18,
        bytes_written: 671_088_640,
        retrim_ok: true,
        scope:
          "14 aşırı parçalanmış (>50 parça) dosya toparlandı ve ReTrim (/L) uygulandı.",
      },
    });
    window.__fixture.tierAnalyses[ssd.id] = {
      disk_id: ssd.id,
      letter: "C",
      disk_type: ssd.type,
      is_ssd: true,
      replay: false,
      fill_percent: 85.9,
      free_bytes: 72e9,
      size_bytes: 512e9,
      slc_pressure: "high",
      slc_note_tr:
        "SSD %85,9 dolu; dinamik SLC yazma önbelleği daralıyor. Soğuk verileri HDD'ye taşımanız yazma hızını korur.",
      extreme_fragmented_count: 14,
      worst_extents: 184,
      companion_role: "hdd_archive",
      companion: {
        disk_id: hdd.id,
        model: hdd.model,
        letter: "E",
        type: "HDD",
        free_bytes: 240e9,
        role: "hdd_archive",
      },
      total_reclaimable_bytes: 22_400_000_000,
      candidates: [
        {
          id: "cand-cold-1",
          name: "4K-Arsiv",
          path: "C:\\Users\\Demo\\Videos\\4K-Arsiv",
          is_dir: true,
          size_bytes: 16_000_000_000,
          file_count: 24,
          days_untouched: 95,
          category: "video",
          score: 94,
          reason_tr:
            "95 gündür dokunulmamış büyük 4K video klasörü; NTFS Junction ile kısayollar bozulmadan HDD'ye taşınabilir.",
          recommended_direction: "to_hdd",
        },
        {
          id: "cand-cold-2",
          name: "ubuntu-24.04-desktop.iso",
          path: "C:\\Users\\Demo\\Downloads\\ubuntu-24.04-desktop.iso",
          is_dir: false,
          size_bytes: 6_400_000_000,
          file_count: 1,
          days_untouched: 62,
          category: "iso",
          score: 88,
          reason_tr:
            "62 gündür erişilmeyen büyük ISO kalıbı; HDD arşivine taşınmaya uygun.",
          recommended_direction: "to_hdd",
        },
      ],
      active_moves: structuredClone(window.__fixture.tierMoves),
      summary_tr:
        "2 soğuk veri adayı (20,9 GiB kazanılabilir) ve 14 aşırı parçalanmış dosya bulundu.",
    };
    window.__fixture.tierAnalyses[hdd.id] = {
      disk_id: hdd.id,
      letter: "D",
      disk_type: "HDD",
      is_ssd: false,
      replay: false,
      fill_percent: 52.0,
      free_bytes: 100e9,
      size_bytes: 200e9,
      slc_pressure: "low",
      slc_note_tr:
        "HDD dış ZBR izleri (LCN 0 başlangıcı) iç izlere göre ~%40-50 daha hızlı ardışık okuma sunar.",
      extreme_fragmented_count: 31,
      worst_extents: 92,
      companion_role: "ssd_boost",
      companion: {
        disk_id: ssd.id,
        model: ssd.model,
        letter: "C",
        type: ssd.type,
        free_bytes: 72e9,
        role: "ssd_boost",
      },
      total_reclaimable_bytes: 12_800_000_000,
      candidates: [
        {
          id: "cand-hot-1",
          name: "Cyberpunk2077",
          path: "D:\\Games\\Cyberpunk2077",
          is_dir: true,
          size_bytes: 12_800_000_000,
          file_count: 142,
          days_untouched: 1,
          category: "game",
          score: 91,
          reason_tr:
            "Son 24 saatte kullanılan sıcak oyun klasörü; NTFS Junction ile SSD'ye terfi ettirilebilir.",
          recommended_direction: "to_ssd",
        },
      ],
      active_moves: structuredClone(window.__fixture.tierMoves),
      summary_tr:
        "1 sıcak oyun klasörü (SSD terfisine uygun) ve 31 parçalanmış dosya tespit edildi.",
    };
  });
  await page.locator(".disk-tile").nth(1).click();
  await expect(page.locator("#smart-tiering-card")).toContainText(
    "SSD → HDD Katmanlama Hazır",
  );
  await expect(page.locator("main")).not.toContainText("SSD'de Engelli");
  await expect(page.locator(".tier-job-banner")).toBeVisible();
  await expect(page.locator(".tier-job-banner")).toContainText("14");
  await expect(page.locator('[data-tier-job="ssd_extent_optimize"]')).toBeEnabled();
  await page.locator('[data-action="analyze-tiering"]').click();
  await expect(page.locator(".tier-analysis-panel")).toBeVisible();
  await expect(page.locator(".tier-slc-strip")).toContainText("SLC Önbellek");
  await expect(page.locator(".tier-slc-note")).toContainText("dinamik SLC");
  await expect(page.locator(".tier-candidate-row")).toHaveCount(2);
  await expect(page.locator(".tier-candidate-row").first()).toContainText(
    "NTFS Junction (Kısayol bozulmaz)",
  );
  await expect(page.locator('[data-tier-relocate="cand-cold-1"]')).toContainText(
    "HDD'ye Taşı (E:)",
  );
  await expect(page.locator(".tier-moves-section")).toBeVisible();
  await expect(page.locator(".tier-move-row")).toHaveCount(1);
  await expect(page.locator('[data-tier-rollback="move-sim-1"]')).toContainText(
    "SSD'ye Geri Al",
  );
  await page.evaluate(() => {
    const t = document.querySelector("#toast");
    if (t) {
      t.className = "";
      t.textContent = "";
    }
  });
  await page.screenshot({
    path: path.join(output, "optimize-tiering-ssd.png"),
    fullPage: true,
  });
  // Verify SSD narrow responsive layout with full AI tiering panel open
  await page.setViewportSize({ width: 390, height: 844 });
  const optNarrowOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  if (optNarrowOverflow)
    throw new Error("Horizontal document overflow on optimize AI tiering at 390px");
  await page.screenshot({
    path: path.join(output, "optimize-narrow.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 1400, height: 1000 });
  // Verify explicit consent on SSD extent optimize job (cancel dispatches nothing)
  await page.locator('[data-tier-job="ssd_extent_optimize"]').click();
  await expect(page.locator("#confirmation")).toContainText("ReTrim");
  await page.locator("#confirm-cancel").click();
  // Verify relocate confirmation + IPC + rollback confirmation + IPC
  await page.locator('[data-tier-relocate="cand-cold-1"]').click();
  await expect(page.locator("#confirmation")).toContainText("SHA-256");
  await expect(page.locator("#confirmation")).toContainText(
    "NTFS Directory Junction",
  );
  await expect(page.locator("#confirmation .dry-run-box")).toBeVisible();
  await expect(page.locator("#confirmation .dry-run-box")).toContainText(
    "FSCTL_SET_REPARSE_POINT",
  );
  await page.locator("#confirm-run").click();
  await expect(page.locator(".tier-move-row")).toHaveCount(2);
  const relocCall = await page.evaluate(() =>
    window.__calls.filter((c) => c.cmd === "relocate_tier_item").at(-1),
  );
  if (
    !relocCall ||
    relocCall.args.targetLetter !== "E" ||
    relocCall.args.direction !== "to_hdd" ||
    !String(relocCall.args.confirmation).startsWith("tier_relocate:")
  ) {
    throw new Error(
      `Unexpected relocate_tier_item payload: ${JSON.stringify(relocCall)}`,
    );
  }
  await page.locator('[data-tier-rollback="move-sim-1"]').click();
  await expect(page.locator("#confirmation")).toContainText("geri");
  await page.locator("#confirm-run").click();
  await expect(page.locator(".tier-move-row")).toHaveCount(1);
  const rbCall = await page.evaluate(() =>
    window.__calls.filter((c) => c.cmd === "rollback_tier_move").at(-1),
  );
  if (
    !rbCall ||
    rbCall.args.moveId !== "move-sim-1" ||
    rbCall.args.confirmation !== "tier_rollback:move-sim-1"
  ) {
    throw new Error(
      `Unexpected rollback_tier_move payload: ${JSON.stringify(rbCall)}`,
    );
  }
  // Switch to HDD and verify AI Outer-Track + SSD Hot Promotion + classic HDD manual tools
  await page.locator(".disk-tile").first().click();
  await expect(page.locator("#smart-tiering-card")).toContainText(
    "HDD Dış-İz + SSD Terfi Hazır",
  );
  await expect(page.locator('[data-tier-job="hdd_outer_optimize"]')).toBeEnabled();
  await page.locator('[data-action="analyze-tiering"]').click();
  await expect(page.locator(".tier-candidate-row")).toHaveCount(1);
  await expect(page.locator('[data-tier-relocate="cand-hot-1"]')).toContainText(
    "SSD'ye Terfi Et (C:)",
  );
  await page.locator('[data-tier-job="hdd_outer_optimize"]').click();
  await expect(page.locator("#confirmation")).toContainText("dış ZBR");
  await page.locator("#confirm-cancel").click();
  const manualOpen = await page
    .locator("details.manual-controls")
    .evaluate((el) => el.hasAttribute("open"));
  if (!manualOpen) {
    await page.locator(".manual-controls > summary").click();
  }
  await expect(page.locator("details.manual-controls")).toContainText(
    "Klasik HDD Birleştirme & APM Kafa Park",
  );
  await page.screenshot({
    path: path.join(output, "optimize-tiering-hdd.png"),
    fullPage: true,
  });
  const tierJobStarts = await page.evaluate(
    () =>
      window.__calls.filter(
        (c) => c.cmd === "start_job" && c.args?.request?.kind === "smart_tier",
      ).length,
  );
  if (tierJobStarts !== 0)
    throw new Error("Cancelled smart_tier confirmation dispatched a job");
  checks.push(
    "v1.8.0 AI Smart Tiering verified on NVMe SSD (SLC bar, cold candidates, Junction relocate + rollback, extent optimize banner, no 'SSD'de Engelli') and HDD (outer-track, hot SSD promotion, classic HDD manual controls) with 0 overflow at 390px",
  );
  await page.evaluate(() => {
    window.__fixture.state.disks[0].type = "NVMe SSD";
  });
  await page.evaluate(() => {
    const d = window.__fixture.state.disks[0];
    window.__fixture.jobs.push({
      id: "sim-recovery-1",
      kind: "recovery",
      disk_id: d.id,
      model: d.model,
      letter: "D",
      status: "completed",
      phase: "Tamamlandı",
      progress: 100,
      completed: 1,
      total: 1,
      started_at: "2026-11-18T12:00:00Z",
      finished_at: "2026-11-18T12:00:05Z",
      error: null,
      cleanup_error: null,
      read_bytes: 0,
      write_bytes: 0,
      prepared_bytes: 0,
      options: { mode: "quick", size_mib: 256 },
      results: [],
      blocks: [],
      summary: {},
      damaged_files: [],
      repair_summary: {
        mode: "deep",
        high_confidence: 4,
        exe_found: 1,
        mft_scanned: 42000,
        vss_found: 1,
        usn_deleted: 12,
        total_items: 6,
        deleted_apps: 2,
      },
      recovery_items: [
        {
          id: "rec-1",
          name: "SpotifySetup.exe",
          original_path: "D:\\Downloads\\SpotifySetup.exe",
          extension: "exe",
          category: "exe_setup",
          sub_type: "64-bit NSIS Kurulum Paketi",
          product_name: "Spotify",
          publisher: "Spotify AB",
          size_bytes: 94371840,
          deleted_at: "2026-11-18T11:30:00Z",
          source: "vss_shadow",
          confidence: 100,
          status_label: "VSS Gölge Kopya (%100 Sağlam · TRIM Korumalı)",
          entropy: 6.4,
          trim_zeroed: false,
          ai_summary:
            "64-bit NSIS Kurulum Paketi · VSS Gölge Kopyasında eksiksiz bulundu.",
          source_letter: "D",
          openable: true,
        },
        {
          id: "rec-2",
          name: "Fatura.pdf",
          original_path: "D:\\Belgeler\\Fatura.pdf",
          extension: "pdf",
          category: "document",
          sub_type: "PDF Belgesi",
          size_bytes: 204800,
          deleted_at: "2026-11-18T11:32:00Z",
          source: "mft_nonresident",
          confidence: 86,
          status_label: "Yüksek Bütünlük ($MFT)",
          entropy: 5.2,
          trim_zeroed: false,
          ai_summary: "Kümeler boş alanda korunuyor.",
          source_letter: "D",
          openable: true,
        },
        {
          id: "rec-photo",
          name: "tatil.jpg",
          original_path: "D:\\Fotograflar\\tatil.jpg",
          extension: "jpg",
          category: "photo",
          sub_type: "JPEG Fotoğraf (1920×1080)",
          size_bytes: 1843200,
          deleted_at: "2026-11-18T11:34:00Z",
          source: "carved",
          confidence: 82,
          status_label: "Otomatik Onarılabilir",
          entropy: 7.4,
          trim_zeroed: false,
          ai_summary: "Kısmen kesilmiş JPEG fotoğraf.",
          source_letter: "D",
          media_status: "repaired",
          openable: true,
          repair_note:
            "Eksik JPEG sonu (EOI) onarıldı; kurtarılan bölüm açılabilir hale getirildi.",
        },
        {
          id: "rec-video",
          name: "klip.mp4",
          original_path: "D:\\Videolar\\klip.mp4",
          extension: "mp4",
          category: "video_audio",
          sub_type: "MP4 Video",
          size_bytes: 14680064,
          deleted_at: "2026-11-18T11:35:00Z",
          source: "carved",
          confidence: 80,
          status_label: "Otomatik Onarılabilir",
          entropy: 7.8,
          trim_zeroed: false,
          ai_summary: "H.264 MP4 videosu.",
          source_letter: "D",
          media_status: "repaired",
          openable: true,
          repair_note:
            "Eksik MP4 moov dizini H.264 karelerinden yeniden oluşturuldu (45 kare oynatılabilir).",
        },
        {
          id: "rec-broken",
          name: "bozuk.jpg",
          original_path: "D:\\Fotograflar\\bozuk.jpg",
          extension: "jpg",
          category: "photo",
          sub_type: "JPEG Fotoğraf",
          size_bytes: 4096,
          deleted_at: "2026-11-18T11:36:00Z",
          source: "carved",
          confidence: 12,
          status_label: "Açılamaz",
          entropy: 7.9,
          trim_zeroed: false,
          ai_summary: "Üzerine yazılmış hasarlı fotoğraf verisi.",
          source_letter: "D",
          media_status: "unrecoverable",
          openable: false,
          repair_note:
            "JPEG başlığı veya kurtarılabilir EXIF önizlemesi bulunamadı.",
        },
        {
          id: "rec-3",
          name: "vcruntime140.dll",
          original_path: "D:\\Temp\\vcruntime140.dll",
          extension: "dll",
          category: "system_lib",
          sub_type: "64-bit DLL Kütüphanesi",
          size_bytes: 124928,
          deleted_at: "2026-11-18T11:33:00Z",
          source: "mft_nonresident",
          confidence: 78,
          status_label: "Yüksek Bütünlük ($MFT)",
          entropy: 5.9,
          trim_zeroed: false,
          ai_summary: "DLL kütüphanesi.",
          source_letter: "D",
          openable: true,
        },
      ],
      deleted_apps: [
        {
          name: "Spotify",
          publisher: "Spotify AB",
          exe_path: "D:\\Downloads\\SpotifySetup.exe",
          installer_found: true,
          recoverable_item_id: "rec-1",
          appdata_paths: ["C:\\Users\\Demo\\AppData\\Roaming\\Spotify"],
          winget_hint: 'winget install --id Spotify.Spotify',
          confidence: 100,
          sources: ["Prefetch", "ShimCache", "UserAssist", "VSS Gölge Kopya"],
          run_count: 19,
          last_run_at: "2026-11-18T11:30:00Z",
          vss_recoverable: true,
          category: "Uygulama",
          recovery_status: "full_restore",
          leftover_bytes: 48234496,
          install_dir: "C:\\Users\\Demo\\AppData\\Roaming\\Spotify",
          drive_letter: "D",
        },
        {
          name: "Cyberpunk 2077",
          publisher: "CD PROJEKT RED",
          exe_path: "D:\\Games\\Cyberpunk 2077\\bin\\x64\\Cyberpunk2077.exe",
          installer_found: false,
          recoverable_item_id: null,
          appdata_paths: ["C:\\Users\\Demo\\AppData\\Local\\CD Projekt Red"],
          winget_hint: 'winget search "Cyberpunk 2077"',
          confidence: 92,
          sources: ["GameConfigStore", "Güvenlik Duvarı", "AppData Kalıntısı"],
          run_count: 11,
          last_run_at: "2026-11-16T21:15:00Z",
          vss_recoverable: false,
          category: "Oyun",
          recovery_status: "reinstall_only",
          leftover_bytes: 52428800,
          install_dir: "D:\\Games\\Cyberpunk 2077",
          drive_letter: "D",
        },
      ],
    });
  });
  await page.locator(".disk-tile").nth(1).click();
  await page.locator(".disk-tile").first().click();
  await page.locator('[data-page="recovery"]').click();
  await expect(page.locator(".recovery-tabs")).toBeVisible();
  await expect(page.locator('[data-rec-tab="apps"]')).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await expect(page.locator(".app-bundle-card")).toHaveCount(2);
  await expect(page.locator(".app-bundle-card").first()).toContainText(
    "%100 Geri Getirilebilir",
  );
  await expect(page.locator(".app-bundle-card").first()).toContainText(
    "19 kez çalıştırıldı",
  );
  await expect(page.locator(".app-bundle-card").first()).toContainText(
    "VSS / Geri Dönüşüm'den Kurtar",
  );
  await expect(page.locator(".rec-telemetry")).toContainText("42.000");
  await page.locator('[data-app-filter="games"]').click();
  await expect(page.locator(".app-bundle-card")).toHaveCount(1);
  await expect(page.locator(".app-bundle-card")).toContainText("Cyberpunk 2077");
  await page.locator(".app-evidence-details > summary").first().click();
  await expect(page.locator(".app-evidence-details")).toContainText(
    "GameConfigStore",
  );
  await expect(page.locator(".app-evidence-details")).toContainText(
    "Güvenlik Duvarı",
  );
  await page.locator('[data-app-filter="all"]').click();
  await expect(page.locator(".app-bundle-card")).toHaveCount(2);
  await page.evaluate(() => {
    const t = document.querySelector("#toast");
    if (t) {
      t.className = "";
      t.textContent = "";
    }
  });
  await page.screenshot({
    path: path.join(output, "recovery-dark.png"),
    fullPage: true,
  });
  await page.locator('[data-rec-tab="files"]').click();
  await expect(page.locator(".recovery-row")).toHaveCount(4);
  await expect(page.locator(".recovery-list")).toContainText(
    "Otomatik Onarılacak",
  );
  await expect(page.locator(".recovery-list")).toContainText("klip.mp4");
  await expect(page.locator(".recovery-list")).not.toContainText("bozuk.jpg");
  await page.locator("#show-unopenable-media").check();
  await expect(page.locator(".recovery-row")).toHaveCount(5);
  await expect(page.locator(".recovery-list")).toContainText("bozuk.jpg");
  await page.locator("#show-unopenable-media").uncheck();
  await expect(page.locator(".recovery-row")).toHaveCount(4);
  await page.locator('[data-preview-id="rec-photo"]').click();
  await expect(page.locator("#confirmation .preview-modal")).toBeVisible();
  await expect(
    page.locator("#confirmation .preview-repair-banner"),
  ).toContainText("Otomatik Onarıldı");
  await expect(page.locator("#confirmation .preview-image")).toBeVisible();
  await page.locator("#close-result").click();
  await page.locator('input[data-rec-select="rec-1"]').check();
  await page.locator('input[data-rec-select="rec-photo"]').check();
  await expect(page.locator(".batch-recovery-bar")).toContainText(
    "2 dosya seçildi",
  );
  await page.locator('[data-action="rec-restore-batch"]').click();
  const batchCall = await page.evaluate(() =>
    window.__calls.filter((c) => c.cmd === "restore_recovered_batch").at(-1),
  );
  if (
    !batchCall ||
    !Array.isArray(batchCall.args.itemIds) ||
    batchCall.args.itemIds.length !== 2
  ) {
    throw new Error(
      `Expected restore_recovered_batch with 2 itemIds, got ${JSON.stringify(batchCall)}`,
    );
  }
  await page.screenshot({
    path: path.join(output, "recovery-files-dark.png"),
    fullPage: true,
  });
  await page.locator('[data-rec-cat="system_lib"]').click();
  await expect(page.locator(".recovery-row")).toHaveCount(1);
  await expect(page.locator(".recovery-row")).toContainText("vcruntime140.dll");
  await page.locator('[data-rec-cat="all"]').click();
  await page.locator("#recovery-search").fill("sağlam uygulama");
  await expect(page.locator(".recovery-row")).toHaveCount(1);
  await expect(page.locator(".recovery-row")).toContainText("SpotifySetup.exe");
  await expect(page.locator(".recovery-row")).toContainText(
    "NSIS Kurulum Paketi",
  );
  await page.setViewportSize({ width: 390, height: 844 });
  for (const tab of ["files", "apps"]) {
    await page.locator(`[data-rec-tab="${tab}"]`).click();
    const recOverflow = await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    );
    if (recOverflow)
      throw new Error(`Horizontal document overflow on recovery tab: ${tab}`);
  }
  await page.setViewportSize({ width: 1400, height: 1000 });
  checks.push(
    "Recovery Center 2-tab UI, Lost Apps filters/details, media auto-repair badges, hidden unopenable media toggle, preview modal, and batch folder restore verified",
  );
  // Developer & Vibe-Coding suite smoke verification (Dev Workspace, LongPaths/8.3, VHDX Compact, Dev Recovery & .env secret masking)
  await page.evaluate(() => {
    const s = window.__fixture.state;
    const ssd = s.disks[1];
    const hdd = s.disks[0];
    s.windows = {
      ...(s.windows || {}),
      trim_enabled: true,
      system_tuning: {
        ...(s.windows?.system_tuning || {}),
        trim_enabled: true,
        long_paths_enabled: false,
        ntfs_8dot3_creation: 2,
        dev_drive_supported: true,
      },
    };
    window.__fixture.devAnalyses = {
      [ssd.id]: {
        disk_id: ssd.id,
        letter: "C",
        replay: false,
        scanned_roots: ["C:\\Users\\Demo\\Projects"],
        scan_ms: 85,
        truncated: false,
        total_artifact_bytes: 6_200_000_000,
        stale_artifact_bytes: 4_800_000_000,
        total_cache_bytes: 9_400_000_000,
        total_vhdx_bytes: 28_000_000_000,
        total_reclaimable_bytes: 5_700_000_000,
        offload_target: {
          disk_id: hdd.id,
          model: hdd.model,
          letter: "E",
          type: "HDD",
          free_bytes: 240e9,
          direction: "to_archive",
        },
        projects: [
          {
            id: "prj-vibe",
            name: "vibe-saas",
            path: "C:\\Users\\Demo\\Projects\\vibe-saas",
            stack: "Node / JS",
            days_untouched: 21,
            stale: true,
            has_git: true,
            has_env: true,
            recent_git_edit: true,
            artifact_bytes: 4_800_000_000,
            artifacts: [
              {
                id: "art-nm",
                name: "node_modules",
                path: "C:\\Users\\Demo\\Projects\\vibe-saas\\node_modules",
                size_bytes: 1_500_000_000,
                file_count: 4200,
                clean_token: `dev_clean:${ssd.id}:abcdef0123456789`,
              },
              {
                id: "art-next",
                name: ".next",
                path: "C:\\Users\\Demo\\Projects\\vibe-saas\\.next",
                size_bytes: 3_300_000_000,
                file_count: 850,
                clean_token: `dev_clean:${ssd.id}:1234567890abcdef`,
              },
            ],
          },
          {
            id: "prj-api",
            name: "fast-api",
            path: "C:\\Users\\Demo\\Projects\\fast-api",
            stack: "Python",
            days_untouched: 2,
            stale: false,
            has_git: true,
            has_env: false,
            recent_git_edit: false,
            artifact_bytes: 1_400_000_000,
            artifacts: [
              {
                id: "art-venv",
                name: ".venv",
                path: "C:\\Users\\Demo\\Projects\\fast-api\\.venv",
                size_bytes: 1_400_000_000,
                file_count: 1900,
                clean_token: `dev_clean:${ssd.id}:fedcba9876543210`,
              },
            ],
          },
        ],
        global_caches: [
          {
            id: "cache-ollama",
            name: "Ollama yerel LLM modelleri",
            name_en: "Ollama local LLM models",
            kind: "ai_model",
            path: "C:\\Users\\Demo\\.ollama\\models",
            size_bytes: 8_500_000_000,
            file_count: 14,
            days_untouched: 5,
            is_junction: false,
            can_offload: true,
            can_clean: false,
            clean_token: null,
            note_tr: "NTFS Junction ile ikinci diske taşınabilir.",
            note_en: "Can move to another drive via NTFS Junction.",
          },
          {
            id: "cache-npm",
            name: "npm önbelleği",
            name_en: "npm cache",
            kind: "pkg_cache",
            path: "C:\\Users\\Demo\\.npm",
            size_bytes: 900_000_000,
            file_count: 3200,
            days_untouched: 1,
            is_junction: false,
            can_offload: true,
            can_clean: true,
            clean_token: `dev_clean:${ssd.id}:9988776655443322`,
            note_tr: "npm install sırasında yeniden oluşur.",
            note_en: "Rebuilt automatically by npm install.",
          },
        ],
        vhdx_files: [
          {
            id: "vhdx-wsl",
            name: "ext4.vhdx",
            kind: "wsl_vhdx",
            path: "C:\\Users\\Demo\\AppData\\Local\\wsl\\ubuntu\\ext4.vhdx",
            drive_letter: "C",
            size_bytes: 28_000_000_000,
            modified_at: "2026-11-20T10:00:00Z",
          },
        ],
        summary_tr: "2 proje ve yerel model önbelleği bulundu.",
        summary_en: "2 projects and local model caches found.",
      },
    };
    const recJob = window.__fixture.jobs.find((j) => j.id === "sim-recovery-1");
    if (recJob) {
      recJob.recovery_items.push({
        id: "rec-env",
        name: ".env.local",
        original_path: "D:\\Projects\\vibe-saas\\.env.local",
        extension: "local",
        category: "database_code",
        sub_type: "Ortam Değişkenleri (.env)",
        size_bytes: 95,
        deleted_at: "2026-11-18T11:40:00Z",
        source: "vss_shadow",
        confidence: 100,
        status_label: "VSS Gölge Kopya (%100 Sağlam)",
        entropy: 4.5,
        trim_zeroed: false,
        ai_summary: "Geliştirici ortam değişkenleri dosyası.",
        source_letter: "D",
        openable: true,
      });
    }
  });
  await page.locator(".disk-tile").nth(1).click();
  await page.locator('[data-page="optimize"]').first().click();
  await expect(page.locator("#dev-workspace-card")).toBeVisible();
  await page.locator('[data-action="analyze-dev"]').click();
  await expect(page.locator("#dev-workspace-card")).toContainText("vibe-saas");
  await expect(page.locator("#dev-workspace-card")).toContainText(".env korundu");
  await expect(page.locator("#dev-workspace-card")).toContainText("Ollama yerel LLM");
  await expect(page.locator("#dev-workspace-card")).toContainText("ext4.vhdx");
  await page.locator('[data-dev-filter="stale"]').click();
  await expect(page.locator(".dev-project-card")).toHaveCount(1);
  await page.locator('[data-dev-filter="all"]').click();
  await expect(page.locator(".dev-project-card")).toHaveCount(2);
  // Safe Clean Dry-Run + IPC
  await page.locator('[data-dev-clean-path*="node_modules"]').click();
  await expect(page.locator("#confirmation .dry-run-box")).toContainText("dev_clean:");
  await page.locator("#confirm-run").click();
  await expect(page.locator("#dev-workspace-card")).not.toContainText("vibe-saas/node_modules");
  const cleanCall = await page.evaluate(() =>
    window.__calls.filter((c) => c.cmd === "clean_dev_artifact").at(-1),
  );
  if (!cleanCall || !String(cleanCall.args.artifactPath).endsWith("node_modules")) {
    throw new Error(`Expected clean_dev_artifact IPC, got ${JSON.stringify(cleanCall)}`);
  }
  // AI model Junction offload Dry-Run
  await page.locator('[data-dev-offload-id="cache-ollama"]').click();
  await expect(page.locator("#confirmation .dry-run-box")).toContainText("FSCTL_SET_REPARSE_POINT");
  await page.locator("#confirm-cancel").click();
  // VHDX compact Dry-Run
  await page.locator('[data-opt-action="compact_wsl_vhdx"]').click();
  await expect(page.locator("#confirmation .dry-run-box")).toContainText("diskpart.exe");
  await page.locator("#confirm-cancel").click();
  // LongPaths & NTFS 8.3 manual cards Dry-Run
  const manualOpenDev = await page
    .locator("details.manual-controls")
    .evaluate((el) => el.hasAttribute("open"));
  if (!manualOpenDev) await page.locator("details.manual-controls > summary").click();
  await page.locator('[data-opt-action="long_paths"][data-opt-value="enable"]').click();
  await expect(page.locator("#confirmation .dry-run-box")).toContainText("LongPathsEnabled");
  await page.locator("#confirm-cancel").click();
  await page.locator('[data-opt-action="ntfs_8dot3"][data-opt-value="disable"]').click();
  await expect(page.locator("#confirmation .dry-run-box")).toContainText("disable8dot3");
  await page.locator("#confirm-cancel").click();
  // 390px responsive overflow check with Dev Workspace open
  await page.setViewportSize({ width: 390, height: 844 });
  const devOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  if (devOverflow) throw new Error("Horizontal document overflow on #dev-workspace-card at 390px");
  await page.setViewportSize({ width: 1400, height: 1000 });
  // Dev recovery mode & masked .env preview toggle
  await page.locator(".disk-tile").first().click();
  await page.locator('[data-page="recovery"]').first().click();
  await expect(page.locator('input[name="rec-mode"][value="dev"]')).toBeVisible();
  await page.locator('[data-rec-tab="files"]').click();
  await page.locator("#recovery-search").fill(".env");
  await expect(page.locator(".recovery-row")).toHaveCount(1);
  await page.locator('[data-preview-id="rec-env"]').click();
  await expect(page.locator(".preview-secret-banner")).toContainText("2 hassas satır");
  await expect(page.locator("#preview-text-pre")).toContainText("••••••••");
  await expect(page.locator("#preview-text-pre")).not.toContainText("sk-demo-12345");
  await page.locator("#toggle-secret-mask").click();
  await expect(page.locator("#preview-text-pre")).toContainText("sk-demo-12345");
  await page.locator("#close-result").click();
  await page.locator("#recovery-search").fill("");
  checks.push(
    "Developer & Vibe-Coding Suite verified: project bloat radar, safe clean Dry-Run, Ollama Junction offload, VHDX compact, LongPaths & 8.3 tuning, dev recovery mode, .env secret masking toggle, and 0 overflow at 390px",
  );
  await page.locator('[data-page="tools"]').click();
  await expect(page.locator('[data-action="retrim"]')).toBeEnabled();
  await page.locator('[data-action="retrim"]').click();
  await expect(page.locator("#confirmation")).toContainText("D:");
  await page.evaluate(() => {
    window.__fixture.state.disks[0].volumes[0].letter = "C";
  });
  await page.waitForTimeout(2600);
  await page.locator("#confirm-run").click();
  await expect(page.locator("#close-result")).toBeVisible();
  const maintenanceLetter = await page.evaluate(
    () =>
      window.__calls
        .filter((c) => c.cmd === "disk_action" && c.args.action === "retrim")
        .at(-1).args.letter,
  );
  if (maintenanceLetter !== "D")
    throw new Error(
      `Maintenance consent target changed to ${maintenanceLetter}`,
    );
  await page.locator("#close-result").click();
  checks.push(
    "Maintenance confirmation pins its original volume despite incoming inventory updates",
  );
  checks.push(
    "Read benchmark warns about preparation writes; cancelling sends no command",
  );
  await page.locator('[data-action="export"]').click();
  await expect(page.locator("#toast")).toContainText("Rapor kaydedildi");
  checks.push("Report command uses fixed kind and selected disk");

  // Verify cleaned UI branding & GitHub updater flows in TR + EN, then test full TR -> EN -> TR roundtrip
  await expect(page.locator("#health-mode-setting")).toContainText(
    "Varsayılan (Standart)",
  );
  await expect(page.locator("body")).not.toContainText("HDS Uyumlu");
  await expect(page.locator("body")).not.toContainText("AI DEPOLAMA");
  await page.locator('[data-action="check-updates"]').click();
  await expect(page.locator("#toast")).toContainText("Güncel sürümü");
  await expect(page.locator('[data-action="open-release"]')).toBeVisible();
  await page.locator('[data-action="open-release"]').click();

  await page.evaluate(() => {
    window.__fixture.mockUpdate = {
      repo: "yuefdev/ssd-sentinel",
      current: "1.10.0",
      status: "update_available",
      latest: "v2.0.0",
      url: "https://github.com/yuefdev/ssd-sentinel/releases/tag/v2.0.0",
      download_url:
        "https://github.com/yuefdev/ssd-sentinel/releases/download/v2.0.0/SSD%20Sentinel_2.0.0_x64-setup.exe",
      asset_name: "SSD Sentinel_2.0.0_x64-setup.exe",
      asset_size: 3800000,
      sha256_url:
        "https://github.com/yuefdev/ssd-sentinel/releases/download/v2.0.0/SHA256SUMS.txt",
      notes: "Added faster NVMe smart scan.",
      message: "Yeni sürüm mevcut: v2.0.0",
      message_en: "A new version is available: v2.0.0",
    };
  });
  await page.locator('[data-action="check-updates"]').click();
  await expect(page.locator(".update-panel")).toContainText("v2.0.0");
  await expect(page.locator('[data-action="install-update"]')).toBeVisible();
  await page.locator('[data-action="install-update"]').click();
  await expect(page.locator("#confirmation")).toContainText("SHA-256");
  await expect(page.locator("#confirmation")).toContainText("v2.0.0");
  await page.locator("#confirm-cancel").click();

  await page.evaluate(() => {
    const s = window.__fixture.state;
    s.disks[0].type = "HDD";
    s.read_only_guard = true;
    const nvme = s.disks[1];
    if (nvme) {
      window.__fixture.forecasts[nvme.id] = {
        ready: true,
        sample_count: 14,
        span_hours: 168.0,
        daily_written_gb: 26.4,
        estimated_remaining_years: 7.2,
        pending_delta_7d: 0,
        reallocated_delta_7d: 0,
        uncorrectable_delta_7d: 0,
        temp_avg_7d: 44.8,
        risk_level: "ok",
        summary_tr: "Son 7 günde hata sayacı artışı yok; disk kararlı.",
        advice_tr: "Düzenli yedekleme alışkanlığınızı koruyun.",
      };
    }
  });
  await page.locator('[data-lang="en"]').click();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.locator('[data-action="refresh"]').click();
  await expect(page.locator(".disk-tile").nth(0)).toContainText("HDD");
  const guardBtn = page.locator('[data-action="toggle-write-guard"]');
  if ((await guardBtn.getAttribute("aria-pressed")) === "true") {
    await guardBtn.click();
  }
  await expect(guardBtn).toContainText("Read-Only");

  await page.locator('[data-page="overview"]').first().click();
  await page.locator(".disk-tile").nth(1).click();
  await expect(page.locator("main")).toContainText("Health & endurance");
  await expect(page.locator("body")).not.toContainText("HDS Uyumlu");
  await page.evaluate(() => {
    const t = document.getElementById("toast");
    if (t) t.hidden = true;
  });
  await page.screenshot({
    path: path.join(output, "overview-en.png"),
    fullPage: true,
  });

  await page.locator('[data-action="theme"]').click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.screenshot({
    path: path.join(output, "overview-dark-en.png"),
    fullPage: true,
  });
  await page.locator('[data-action="theme"]').click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");

  await page.locator('[data-page="optimize"]').first().click();
  await expect(page.locator("#smart-tiering-card")).toContainText(
    "Smart Storage Tiering & Optimization",
  );
  await expect(page.locator("body")).not.toContainText("AI DEPOLAMA");
  await page.screenshot({
    path: path.join(output, "optimize-en.png"),
    fullPage: true,
  });

  await page.locator(".disk-tile").nth(0).click();

  await page.locator('[data-page="surface"]').first().click();
  await expect(page.locator("main")).toContainText("200-Zone Surface");
  await expect(page.locator("body")).not.toContainText("AI YÜZEY");
  await expect(page.locator("body")).not.toContainText("AI NOKTA");
  if ((await page.locator(".ai-diagnosis-card").count()) > 0) {
    await expect(page.locator(".ai-diagnosis-card")).toContainText(
      "SMART SURFACE DIAGNOSIS",
    );
  }

  await page.locator('[data-page="recovery"]').first().click();
  await expect(page.locator("main")).toContainText("Recoverable Files");

  await page.locator('[data-page="smart"]').first().click();
  await expect(page.locator(".smart-table")).toContainText("Raw value");

  await page.locator('[data-page="tools"]').first().click();
  await expect(page.locator("#health-mode-setting")).toContainText(
    "Default (Standard)",
  );
  await page.locator('[data-action="check-updates"]').click();
  await expect(page.locator(".update-panel")).toContainText(
    "New release available: v2.0.0",
  );
  await page.locator('[data-action="install-update"]').click();
  await expect(page.locator("#confirmation")).toContainText("SHA-256");
  await page.locator("#confirm-cancel").click();

  // Verify 0 horizontal overflow in EN at 390px mobile viewport
  await page.setViewportSize({ width: 390, height: 844 });
  for (const enPage of ["overview", "optimize"]) {
    await page.locator(`[data-page="${enPage}"]`).first().click();
    const enOverflow = await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    );
    if (enOverflow) {
      throw new Error(`Horizontal document overflow in EN mode: ${enPage}`);
    }
  }
  await page.setViewportSize({ width: 1400, height: 1000 });

  // Verify English language preference persists across reload, then restore TR
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.locator('[data-lang="tr"]').click();
  await expect(page.locator("html")).toHaveAttribute("lang", "tr");
  checks.push(
    "Clean branding (0 nav-badge, no HDS Uyumlu / AI labels), GitHub updater UI + SHA-256 confirm dialog, full TR/EN localization across all views, 0 EN 390px overflow, and EN reload persistence verified",
  );
  if (errors.length) throw new Error(errors.join("\n"));
  await writeFile(
    path.join(output, "verification.json"),
    JSON.stringify(
      { passed: true, real_disk_workload: false, checks, errors },
      null,
      2,
    ),
  );
  console.log(
    JSON.stringify({ passed: checks.length, errors, output }, null, 2),
  );
} catch (error) {
  await writeFile(
    path.join(output, "verification.json"),
    JSON.stringify(
      {
        passed: false,
        real_disk_workload: false,
        checks,
        errors,
        error: String(error),
      },
      null,
      2,
    ),
  );
  throw error;
} finally {
  await browser?.close();
  server.kill();
}
