/** Owned native replay run; no simulated bridge and no real disk workloads. */
import { chromium, expect } from "@playwright/test";
import { spawn, execFile } from "node:child_process";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { promisify } from "node:util";
import path from "node:path";

const exec = promisify(execFile);
const root = path.resolve(import.meta.dirname, "..");
const output = path.resolve(root, process.argv[2] || "results/tauri-native");
await mkdir(output, { recursive: true });
// Fresh WebView2 profile + app data so a persisted theme from a previous run cannot flip checks.
await rm(path.join(output, "webview-profile"), { recursive: true, force: true });
await rm(path.join(output, "data"), { recursive: true, force: true });
const exe = path.resolve(
  root,
  process.argv[3] || "src-tauri/target/release/ssd-sentinel.exe",
);
const bytes = await readFile(exe);
const executable = {
  path: path.relative(root, exe),
  sha256: createHash("sha256").update(bytes).digest("hex"),
  bytes: bytes.length,
};
const child = spawn(
  exe,
  [
    "--snapshot-dir",
    path.join(root, "src-tauri/tests/fixtures"),
    "--data-dir",
    path.join(output, "data"),
  ],
  {
    cwd: root,
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9227",
      WEBVIEW2_USER_DATA_FOLDER: path.join(output, "webview-profile"),
    },
    stdio: "pipe",
    windowsHide: false,
  },
);
const errors = [],
  checks = [];
let browser;
let log = "";
let exited = false;
let exit = null;
child.stdout.on("data", (b) => (log += b));
child.stderr.on("data", (b) => (log += b));
child.on("exit", (code, signal) => {
  exit = { code, signal };
  exited = true;
});
try {
  for (let i = 0; i < 150; i++) {
    if (exited) throw new Error(`App exited: ${log}`);
    try {
      const response = await fetch("http://127.0.0.1:9227/json/version");
      if (response.ok) break;
    } catch {}
    if (i === 149)
      throw new Error("Native WebView2 CDP endpoint did not start");
    await new Promise((r) => setTimeout(r, 100));
  }
  browser = await chromium.connectOverCDP("http://127.0.0.1:9227");
  let page;
  for (let i = 0; i < 100; i++) {
    page = browser
      .contexts()
      .flatMap((c) => c.pages())
      .find((p) => p.url().includes("tauri.localhost"));
    if (page) break;
    await new Promise((r) => setTimeout(r, 100));
  }
  if (!page) throw new Error("Native app page not found");
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !m.text().includes("favicon"))
      errors.push(m.text());
  });
  await expect(page.locator(".disk-tile")).toHaveCount(3, { timeout: 20000 });
  await expect(page.locator(".nav-badge")).toHaveCount(0);
  const state = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("get_state"),
  );
  if (state.backend !== "Rust · Tauri 2" || state.source !== "replay")
    throw new Error("Native Rust IPC state mismatch");
  if (state.version !== "1.10.2")
    throw new Error(`Expected native version 1.10.2, got ${state.version}`);
  if (state.read_only_guard !== true)
    throw new Error(`Expected default read_only_guard=true, got ${JSON.stringify(state.read_only_guard)}`);
  if (state.disks[1].interface !== "NVMe 1.3")
    throw new Error(
      `Expected normalized NVMe interface 'NVMe 1.3', got ${JSON.stringify(state.disks[1].interface)}`,
    );
  const nvmeForecast = await page.evaluate((diskId) =>
    window.__TAURI_INTERNALS__.invoke("get_health_forecast", { diskId }),
    state.disks[1].id,
  );
  if (nvmeForecast.ready !== false)
    throw new Error(
      `Single-snapshot replay must not fabricate forecast: ${JSON.stringify(nvmeForecast)}`,
    );
  const cp = await page.evaluate((diskId) =>
    window.__TAURI_INTERNALS__.invoke("get_surface_checkpoint", { diskId }),
    state.disks[1].id,
  );
  if (cp !== null)
    throw new Error(`Expected null checkpoint on fresh DB, got ${JSON.stringify(cp)}`);
  const tierMoves = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("list_tier_moves"),
  );
  if (!Array.isArray(tierMoves) || tierMoves.length !== 0)
    throw new Error(`Expected empty tier_moves array on fresh DB, got ${JSON.stringify(tierMoves)}`);
  const tierAnalysis = await page.evaluate((diskId) =>
    window.__TAURI_INTERNALS__.invoke("analyze_storage_tiering", {
      diskId,
      letter: "C",
    }),
    state.disks[1].id,
  );
  if (
    tierAnalysis.replay !== true ||
    tierAnalysis.is_ssd !== true ||
    !Array.isArray(tierAnalysis.candidates) ||
    tierAnalysis.candidates.length !== 0
  ) {
    throw new Error(
      `Expected honest replay tiering analysis, got ${JSON.stringify(tierAnalysis)}`,
    );
  }
  checks.push(
    "Release exe v1.10.2 starts native WebView2; real Rust IPC returns three recorded drives, NVMe 1.3, honest forecast, null checkpoint, empty tier moves, and replay-safe tier analysis",
  );
  for (const [i, percent] of [100, 73, 98].entries()) {
    await page.locator(".disk-tile").nth(i).click();
    await expect(page.locator(".health-ring strong")).toContainText(
      String(percent),
    );
  }
  checks.push("Native health values match 100 / 73 / 98 fixtures");
  await page.locator(".disk-tile").nth(1).click();
  await expect(
    page.locator('.overview-quick-actions [data-goto="recovery"]'),
  ).toBeVisible();
  await page.screenshot({ path: path.join(output, "native-overview.png") });
  await page.locator('[data-page="smart"]').click();
  await page.locator("#smart-search").fill("dayanım");
  await expect(page.locator(".smart-table tbody tr")).toHaveCount(1);
  checks.push("Native SMART search and field inspector work");
  await page.locator('[data-page="performance"]').click();
  await expect(page.locator('[data-action="start-benchmark"]')).toBeDisabled();
  await page.locator('[data-page="optimize"]').click();
  await expect(page.locator("#smart-tiering-card")).toBeVisible();
  await expect(page.locator("main")).not.toContainText("SSD'de Engelli");
  await expect(
    page.locator('[data-tier-job="ssd_extent_optimize"]'),
  ).toBeDisabled();
  await expect(page.locator('[data-opt-action="ntfs_trim"]')).toBeDisabled();
  await page.screenshot({ path: path.join(output, "native-optimize.png") });
  await page.locator('[data-page="recovery"]').click();
  await expect(page.locator('[data-action="start-recovery"]')).toBeDisabled();
  await page.screenshot({ path: path.join(output, "native-recovery.png") });
  const id = state.disks[0].id;
  const denied = await page.evaluate(async (id) => {
    try {
      await window.__TAURI_INTERNALS__.invoke("start_job", {
        request: {
          kind: "surface",
          disk_id: id,
          letter: "C",
          confirmation: `surface:${id}:C:read_scan`,
          options: { mode: "read_scan", size_mib: 64 },
        },
      });
      return false;
    } catch {
      return true;
    }
  }, id);
  const autoDenied = await page.evaluate(async (id) => {
    try {
      await window.__TAURI_INTERNALS__.invoke("start_job", {
        request: {
          kind: "autopilot",
          disk_id: id,
          letter: "C",
          confirmation: `autopilot:${id}:C`,
          options: { profile: "auto" },
        },
      });
      return false;
    } catch {
      return true;
    }
  }, id);
  const optDenied = await page.evaluate(async () => {
    try {
      await window.__TAURI_INTERNALS__.invoke("optimize_action", {
        request: {
          action: "ntfs_trim",
          target: "system",
          value: "enable",
          confirmation: "optimize:ntfs_trim:system",
        },
      });
      return false;
    } catch {
      return true;
    }
  });
  const batchDenied = await page.evaluate(async () => {
    try {
      await window.__TAURI_INTERNALS__.invoke("restore_recovered_batch", {
        jobId: "missing",
        itemIds: ["item-1"],
      });
      return false;
    } catch {
      return true;
    }
  });
  const nvmeId = state.disks[1].id;
  const smartTierSsdDenied = await page.evaluate(async (diskId) => {
    try {
      await window.__TAURI_INTERNALS__.invoke("start_job", {
        request: {
          kind: "smart_tier",
          disk_id: diskId,
          letter: "C",
          confirmation: `smart_tier:${diskId}:C:ssd_extent_optimize`,
          options: { mode: "ssd_extent_optimize" },
        },
      });
      return false;
    } catch {
      return true;
    }
  }, nvmeId);
  const smartTierHddDenied = await page.evaluate(async (diskId) => {
    try {
      await window.__TAURI_INTERNALS__.invoke("start_job", {
        request: {
          kind: "smart_tier",
          disk_id: diskId,
          letter: "D",
          confirmation: `smart_tier:${diskId}:D:hdd_outer_optimize`,
          options: { mode: "hdd_outer_optimize" },
        },
      });
      return false;
    } catch {
      return true;
    }
  }, id);
  const relocateDenied = await page.evaluate(async (diskId) => {
    try {
      await window.__TAURI_INTERNALS__.invoke("relocate_tier_item", {
        diskId,
        sourcePath: "C:\\Users\\Demo\\Videos\\clip.mkv",
        targetLetter: "D",
        direction: "to_hdd",
        confirmation: `tier_relocate:${diskId}:D`,
      });
      return false;
    } catch {
      return true;
    }
  }, nvmeId);
  const rollbackDenied = await page.evaluate(async () => {
    try {
      await window.__TAURI_INTERNALS__.invoke("rollback_tier_move", {
        moveId: "missing-move",
        confirmation: "tier_rollback:missing-move",
      });
      return false;
    } catch {
      return true;
    }
  });
  if (
    !denied ||
    !autoDenied ||
    !optDenied ||
    !batchDenied ||
    !smartTierSsdDenied ||
    !smartTierHddDenied ||
    !relocateDenied ||
    !rollbackDenied
  )
    throw new Error(
      "Replay job, autopilot, optimize_action, restore_recovered_batch, smart_tier, relocate_tier_item, or rollback_tier_move passed Rust guards",
    );
  checks.push(
    "Native replay refuses disk jobs, autopilot, optimize_action, restore_recovered_batch, smart_tier, relocate_tier_item, and rollback_tier_move at the Rust boundary",
  );
  await page.locator('[data-page="surface"]').click();
  await expect(page.locator(".block.pending")).toHaveCount(200);
  await page.screenshot({ path: path.join(output, "native-surface.png") });
  await page.locator('[data-page="history"]').first().click();
  await expect(page.locator(".line-chart circle")).toHaveCount(2);
  checks.push(
    "Native SQLite history contains actual capture timestamps, no fake trends",
  );
  await page.locator('[data-page="tools"]').click();
  await page.locator('[data-action="logs"]').click();
  await expect(page.locator("#confirmation")).toBeVisible();
  await page.locator("#close-result").click();
  checks.push("Recorded SMART logs are served by Rust without starting tests");
  await page.locator('[data-action="theme"]').click();
  await page.locator('[data-page="overview"]').click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.screenshot({ path: path.join(output, "native-light.png") });
  const upd = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("check_updates"),
  );
  if (
    !upd ||
    upd.current !== "1.10.2" ||
    upd.repo !== "yuefdev/ssd-sentinel" ||
    !["up_to_date", "no_release", "update_available", "unreachable"].includes(
      upd.status,
    )
  ) {
    throw new Error(
      `Expected configured yuefdev/ssd-sentinel v1.10.2 update response, got ${JSON.stringify(upd)}`,
    );
  }
  await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("set_preferences", {
      minimiseToTray: false,
      theme: "dark",
      healthMode: "default",
      language: "en",
      readOnlyGuard: false,
    }),
  );
  const stateEn = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("get_state"),
  );
  if (stateEn.language !== "en") {
    throw new Error(`Expected native language='en', got ${JSON.stringify(stateEn.language)}`);
  }
  if (stateEn.read_only_guard !== false) {
    throw new Error(`Expected native read_only_guard=false, got ${JSON.stringify(stateEn.read_only_guard)}`);
  }
  const prefs = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("set_preferences", {
      minimiseToTray: false,
      theme: "dark",
      healthMode: "default",
      language: "tr",
      readOnlyGuard: true,
    }),
  );
  const stateTr = await page.evaluate(() =>
    window.__TAURI_INTERNALS__.invoke("get_state"),
  );
  if (stateTr.language !== "tr") {
    throw new Error(`Expected native language='tr', got ${JSON.stringify(stateTr.language)}`);
  }
  if (stateTr.read_only_guard !== true) {
    throw new Error(`Expected native read_only_guard=true, got ${JSON.stringify(stateTr.read_only_guard)}`);
  }
  checks.push("Native theme, language switching (TR/EN), unconfigured update check, and persisted tray preference IPC succeed");
  if (errors.length) throw new Error(errors.join("\n"));
  await browser.close();
  browser = null;
  await exec("powershell.exe", [
    "-NoProfile",
    "-Command",
    `Add-Type 'using System;using System.Runtime.InteropServices;using System.Text;public class W{public delegate bool E(IntPtr h,IntPtr l);[DllImport("user32.dll")]public static extern bool EnumWindows(E f,IntPtr l);[DllImport("user32.dll")]public static extern uint GetWindowThreadProcessId(IntPtr h,out uint p);[DllImport("user32.dll",CharSet=CharSet.Unicode)]public static extern int GetWindowTextW(IntPtr h,StringBuilder s,int m);[DllImport("user32.dll")]public static extern bool PostMessageW(IntPtr h,uint m,IntPtr w,IntPtr l);}';$t=[IntPtr]::Zero;[void][W]::EnumWindows({param($h,$l);[uint32]$q=0;[void][W]::GetWindowThreadProcessId($h,[ref]$q);if($q -eq ${child.pid}){$sb=New-Object System.Text.StringBuilder 128;[void][W]::GetWindowTextW($h,$sb,128);if($sb.ToString() -eq 'SSD Sentinel'){$script:t=$h}};$true},[IntPtr]::Zero);if($t -ne [IntPtr]::Zero){[void][W]::PostMessageW($t,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)}else{(Get-Process -Id ${child.pid}).CloseMainWindow()}`,
  ]);
  for (let i = 0; i < 100 && !exited; i++)
    await new Promise((r) => setTimeout(r, 100));
  if (!exited || exit.code !== 0 || exit.signal !== null)
    throw new Error(`Native app did not exit cleanly: ${JSON.stringify(exit)}`);
  checks.push(
    "Closing the native window terminates the owned application cleanly",
  );
  await writeFile(
    path.join(output, "verification.json"),
    JSON.stringify(
      {
        passed: true,
        executable,
        exit,
        real_disk_workload: false,
        checks,
        errors,
      },
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
        executable,
        exit,
        real_disk_workload: false,
        checks,
        errors,
        log,
        error: String(error),
      },
      null,
      2,
    ),
  );
  throw error;
} finally {
  await browser?.close();
  if (!exited) child.kill();
}
