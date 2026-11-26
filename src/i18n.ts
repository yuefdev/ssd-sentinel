export type Lang = "tr" | "en";

export function resolveLangFromLocale(
  locale: string | null | undefined,
): Lang {
  const norm = (locale ?? "").trim().toLowerCase();
  return norm === "tr" || norm.startsWith("tr-") || norm.startsWith("tr_")
    ? "tr"
    : "en";
}

export function detectInitialLang(): Lang {
  try {
    const saved =
      typeof localStorage !== "undefined"
        ? localStorage.getItem("sentinel-lang")
        : null;
    if (saved === "tr" || saved === "en") return saved;
  } catch {}
  try {
    const nav =
      typeof navigator !== "undefined"
        ? navigator.language || navigator.languages?.[0] || ""
        : "";
    if (nav) {
      return resolveLangFromLocale(nav);
    }
  } catch {}
  return "en";
}

let currentLang: Lang = detectInitialLang();

export function getLang(): Lang {
  return currentLang;
}

export function setLang(lang: Lang): Lang {
  currentLang = lang === "en" ? "en" : "tr";
  try {
    if (typeof localStorage !== "undefined") {
      localStorage.setItem("sentinel-lang", currentLang);
    }
    if (typeof document !== "undefined" && document.documentElement) {
      document.documentElement.lang = currentLang;
    }
  } catch {}
  return currentLang;
}

export function localeTag(lang: Lang = currentLang): string {
  return lang === "en" ? "en-US" : "tr-TR";
}

export function L(tr: string, en: string, lang: Lang = currentLang): string {
  return lang === "en" ? en : tr;
}

export function translateDiskType(type: string, lang: Lang = currentLang): string {
  if (lang === "tr") return type;
  if (type === "Bilinmiyor") return "Unknown";
  return type;
}

const ATA_EN: Record<string, [string, string]> = {
  "1": [
    "Raw Read Error Rate",
    "Vendor-specific rate or counter; raw numbers do not directly indicate bad sectors.",
  ],
  "3": [
    "Spin-Up Time",
    "Time required to spin mechanical platters up to operating speed.",
  ],
  "4": ["Start/Stop Count", "Total spindle start and stop cycles."],
  "5": [
    "Reallocated Sectors Count",
    "Bad sectors remapped to spare reserve sectors. Track changes over time.",
  ],
  "7": [
    "Seek Error Rate",
    "Vendor-specific rate of magnetic head positioning errors.",
  ],
  "9": [
    "Power-On Hours",
    "Raw unit depends on vendor; overview shows smartctl normalized hours.",
  ],
  "10": [
    "Spin Retry Count",
    "Motor spin-up retries; may indicate power supply instability.",
  ],
  "12": ["Power Cycle Count", "Total device power-on cycles."],
  "169": [
    "Remaining Life Percentage",
    "Vendor SATA SSD remaining endurance indicator.",
  ],
  "173": [
    "Wear Leveling Count",
    "Average/maximum NAND erase cycles and remaining endurance percentage.",
  ],
  "177": [
    "Wear Leveling (Samsung)",
    "NAND erase cycle count and remaining block endurance.",
  ],
  "184": [
    "End-to-End Error",
    "Data path parity and integrity check failures.",
  ],
  "187": [
    "Reported Uncorrectable Errors",
    "Uncorrectable data or command errors reported to the host.",
  ],
  "188": [
    "Command Timeout",
    "Aborted operations due to timeout; raw value may pack multiple counters.",
  ],
  "190": [
    "Drive Temperature",
    "Vendor temperature sensor; may pack historical min/max values.",
  ],
  "191": [
    "G-Sense Error Rate",
    "Detected mechanical shock and vibration events.",
  ],
  "192": [
    "Unsafe Shutdown / Retract Count",
    "Emergency head retracts on power loss; SSD meaning varies by vendor.",
  ],
  "193": [
    "Load / Unload Cycle Count",
    "Number of times HDD heads have loaded and parked.",
  ],
  "194": [
    "Drive Temperature",
    "Vendor temperature sensor; may pack historical min/max values.",
  ],
  "196": [
    "Reallocation Event Count",
    "Total reallocation attempt events; may differ from reallocated sector count.",
  ],
  "197": [
    "Current Pending Sector Count",
    "Unstable sectors waiting to be remapped. Verify important backups first.",
  ],
  "198": [
    "Offline Uncorrectable Sector Count",
    "Uncorrectable sectors found during offline checks; data integrity risk.",
  ],
  "199": [
    "UDMA CRC Error Count",
    "SATA cable or bus transfer errors, not a disk surface defect.",
  ],
  "202": [
    "Vendor Endurance Indicator",
    "SSD remaining life percentage or vendor wear attribute.",
  ],
  "231": [
    "Vendor Endurance Indicator",
    "SSD remaining life percentage or vendor wear attribute (thermal 231 rows excluded).",
  ],
  "233": [
    "Vendor Endurance Indicator",
    "SSD remaining life percentage or vendor wear attribute.",
  ],
  "241": [
    "Total Writes Counter",
    "Raw unit depends on vendor; TB conversion is only shown for verified formats.",
  ],
  "242": [
    "Total Reads Counter",
    "Raw unit depends on vendor; TB conversion is only shown for verified formats.",
  ],
};

const NVME_EN: Record<string, [string, string]> = {
  critical_warning: [
    "Critical Warning",
    "Bitmask: spare capacity, temperature, reliability, read-only state, volatile backup.",
  ],
  temperature: [
    "Composite Temperature",
    "Primary controller-reported temperature in °C.",
  ],
  available_spare: [
    "Available Spare",
    "Remaining spare block capacity percentage compared against threshold.",
  ],
  available_spare_threshold: [
    "Available Spare Threshold",
    "Critical warning triggers if available spare drops below this percentage.",
  ],
  percentage_used: [
    "Percentage Used",
    "Vendor endurance estimate; may exceed 100%. Not a fixed failure date.",
  ],
  data_units_read: [
    "Data Units Read",
    "Lifetime host reads. One NVMe unit equals 512,000 bytes.",
  ],
  data_units_written: [
    "Data Units Written",
    "Lifetime host writes (not physical NAND writes). One unit equals 512,000 bytes.",
  ],
  host_reads: ["Host Read Commands", "Completed host read commands."],
  host_writes: ["Host Write Commands", "Completed host write commands."],
  controller_busy_time: [
    "Controller Busy Time",
    "Total minutes the controller spent busy with I/O commands.",
  ],
  power_cycles: ["Power Cycles", "Total device power-on cycles."],
  power_on_hours: ["Power-On Hours", "Total powered-on hours."],
  unsafe_shutdowns: [
    "Unsafe Shutdowns",
    "Power losses without clean shutdown notification. Not data loss proof alone.",
  ],
  media_errors: [
    "Media & Data Integrity Errors",
    "Uncorrected data integrity events; prioritize backups if increasing.",
  ],
  num_err_log_entries: [
    "Error Information Log Entries",
    "Includes invalid/unsupported commands. Not a bad sector count.",
  ],
  warning_temp_time: [
    "Warning Temperature Time",
    "Cumulative minutes spent above warning temperature threshold.",
  ],
  critical_comp_time: [
    "Critical Temperature Time",
    "Cumulative minutes spent above critical temperature threshold.",
  ],
};

export function translateSmartField(
  row: { id: string; name: string; original: string; description: string },
  lang: Lang = currentLang,
): { name: string; description: string } {
  if (lang === "tr") {
    return { name: row.name, description: row.description };
  }
  if (ATA_EN[row.id]) {
    const [name, description] = ATA_EN[row.id];
    return { name, description };
  }
  if (NVME_EN[row.id]) {
    const [name, description] = NVME_EN[row.id];
    return { name, description };
  }
  const isNvme = !/^\d+$/.test(row.id);
  return {
    name: row.original || row.name,
    description: isNvme
      ? "NVMe/vendor field; no custom interpretation in this version."
      : "Vendor-specific SMART attribute; no health penalty is inferred without model verification.",
  };
}

export function translateHealthExplanation(
  disk: {
    read_incomplete?: boolean;
    health: { percent: number | null; status: string; method: string | null; explanation: string };
  },
  lang: Lang = currentLang,
): string {
  if (lang === "tr") return disk.health.explanation;
  if (disk.read_incomplete) {
    return "SMART read is incomplete. Restart as Administrator and verify device connection.";
  }
  const m = disk.health.method;
  const pct = disk.health.percent;
  if (m === "nvme-remaining-endurance") {
    if (pct !== null) {
      const used = Math.max(0, 100 - pct);
      return `Vendor reports ${used}% endurance used. Remaining endurance = max(0, 100 − ${used}). This is not a speed metric or exact failure date.`;
    }
    if (disk.health.status === "critical") {
      return "NVMe device reported a critical warning or spare capacity below threshold. Verify backups first.";
    }
  }
  if (m === "hdd-strict-estimate") {
    return "Strict mode: higher penalty weights and inner-counter tail decay applied to attributes 05, 07, 0A, C4, C5, and C6.";
  }
  if (m === "hdd-generic-core-estimate") {
    return "Weighted product across attributes 05, 07, 0A, C4, C5, and C6. Core estimate without vendor-specific ceiling overrides.";
  }
  if (m === "sata-ssd-wear-indicator" && pct !== null) {
    return `Calculated from vendor SATA SSD wear indicator (${pct}% remaining endurance).`;
  }
  if (pct === null) {
    return "Required SMART fields are missing or unsupported for this model. Unverified data is not marked healthy.";
  }
  return disk.health.explanation;
}

export function translateAlert(
  alert: { code: string; title: string; detail: string },
  lang: Lang = currentLang,
): { title: string; detail: string } {
  if (lang === "tr") return { title: alert.title, detail: alert.detail };
  switch (alert.code) {
    case "critical":
      return {
        title: "Device reports critical status",
        detail: "Verify backups of important data and inspect vendor diagnostics.",
      };
    case "unknown-health":
      return {
        title: "Health score unverified",
        detail:
          "Required SMART fields could not be read completely (try running as Administrator).",
      };
    case "low-health":
      return {
        title: "Low health / endurance",
        detail: "Verify current backups and plan drive replacement.",
      };
    case "temperature":
      return {
        title: "High drive temperature",
        detail: "Check case airflow, cooling, and sustained write workload.",
      };
    case "sata-link-bottleneck":
      return {
        title: "SATA link speed bottleneck",
        detail:
          "The drive is running below its maximum supported SATA link speed. Check the cable or motherboard port.",
      };
    case "pcie-link-bottleneck":
      return {
        title: "PCIe link bottleneck",
        detail:
          "The NVMe drive is running at a narrower width or lower PCIe generation than supported.",
      };
    case "alignment-4k":
      return {
        title: "Partition 4K alignment issue",
        detail:
          "One or more partitions are not aligned to 4K physical boundaries, reducing random I/O speed.",
      };
    case "media_errors":
      return {
        title: "Media / integrity errors recorded",
        detail: NVME_EN.media_errors[1],
      };
    case "num_err_log_entries":
      return {
        title: "Error log entries recorded",
        detail: NVME_EN.num_err_log_entries[1],
      };
    default:
      if (alert.code.startsWith("ata-")) {
        const id = alert.code.slice(4);
        const mapped = ATA_EN[id];
        if (mapped) {
          return { title: mapped[0], detail: mapped[1] };
        }
      }
      return { title: alert.title, detail: alert.detail };
  }
}

export function translateForecast(
  fc: {
    ready: boolean;
    risk_level?: string;
    daily_written_gb?: number | null;
    estimated_remaining_years?: number | null;
    pending_delta_7d?: number | null;
    reallocated_delta_7d?: number | null;
    uncorrectable_delta_7d?: number | null;
    temp_avg_7d?: number | null;
    summary_tr?: string;
    advice_tr?: string;
  },
  isHdd: boolean,
  lang: Lang = currentLang,
): { summary: string; advice: string } {
  if (lang === "tr") {
    return { summary: fc.summary_tr ?? "", advice: fc.advice_tr ?? "" };
  }
  if (!fc.ready) {
    return {
      summary:
        "Requires at least two local measurements taken at least 1 hour apart.",
      advice: "Projections appear automatically as local history accumulates.",
    };
  }
  const uDelta = Number(fc.uncorrectable_delta_7d ?? 0);
  const pDelta = Number(fc.pending_delta_7d ?? 0);
  const rDelta = Number(fc.reallocated_delta_7d ?? 0);
  const years = fc.estimated_remaining_years;
  const gb = fc.daily_written_gb;
  const temp = fc.temp_avg_7d;

  if (fc.risk_level === "critical") {
    const parts: string[] = [];
    if (pDelta > 0) parts.push(`+${pDelta} pending sectors in 7d`);
    if (uDelta > 0) parts.push(`+${uDelta} uncorrectable / media errors in 7d`);
    if (rDelta >= 5) parts.push(`+${rDelta} reallocated sectors in 7d`);
    if (years != null && years < 0.5)
      parts.push(`~${years.toFixed(1)} yr estimated SSD life remaining`);
    if (!parts.length) parts.push("critical health threshold exceeded");
    return {
      summary: `High risk detected: ${parts.join(", ")}.`,
      advice: isHdd
        ? "Back up important data to another drive immediately, run a Full Surface Scan, and plan replacement."
        : "Back up important data to another drive immediately and plan SSD replacement.",
    };
  }
  if (fc.risk_level === "warning") {
    const parts: string[] = [];
    if (rDelta > 0) parts.push(`+${rDelta} reallocated sectors in 7d`);
    if (years != null && years < 2.0)
      parts.push(`~${years.toFixed(1)} yr estimated life left`);
    if (temp != null && temp >= 55) parts.push(`7d avg temp ${temp.toFixed(1)} °C`);
    if (gb != null) parts.push(`${gb.toFixed(1)} GB/day avg writes`);
    return {
      summary: parts.length
        ? `Trend requires attention: ${parts.join(" · ")}.`
        : "Warning signal requires closer monitoring.",
      advice:
        temp != null && temp >= 55
          ? "Inspect case airflow or M.2 heatsink and verify your backups."
          : isHdd
            ? "Verify backups; if errors keep growing, run a Full Surface Scan and Pinpoint Repair."
            : "Verify backups; reducing heavy background writes helps preserve SSD endurance.",
    };
  }
  const summary =
    gb != null && years != null
      ? `At ${gb.toFixed(1)} GB/day average writes, estimated remaining endurance is ~${years.toFixed(1)} years; 0 new errors in 7d.`
      : gb != null
        ? `Average writes ${gb.toFixed(1)} GB/day; zero new sector or media errors in 7d.`
        : temp != null
          ? `Zero new error growth in 7d; average temperature ${temp.toFixed(1)} °C.`
          : "Zero new error growth in 7d; drive trend is stable.";
  return {
    summary,
    advice:
      "No notable wear or surface risk observed; maintain regular backups.",
  };
}

export function translateDiagnosis(
  diag: {
    verdict: string;
    title: string;
    advice: string;
    unreadable: number;
    weak: number;
    slow: number;
    dead_zones: number;
    engine?: string;
  },
  isHdd: boolean,
  lang: Lang = currentLang,
): { title: string; advice: string; engine: string } {
  if (lang === "tr") {
    return {
      title: diag.title,
      advice: diag.advice,
      engine: diag.engine ?? "",
    };
  }
  const engine = "Deterministic surface diagnostics · statistics + rules · offline";
  if (diag.unreadable + diag.weak + diag.slow === 0) {
    return {
      title: "Surface is clean",
      advice: "No unreadable, weak, or slow areas detected. No repair needed.",
      engine,
    };
  }
  if (diag.verdict === "failing") {
    return {
      title: "Drive is severely damaged",
      advice: `${diag.unreadable} unreadable sectors and ${diag.dead_zones} dead zones detected. Back up data to another drive first, isolate dead zones, and plan replacement.`,
      engine,
    };
  }
  if (!isHdd) {
    return {
      title: "Slow-reading regions detected",
      advice: `${diag.slow + diag.weak + diag.unreadable} slow regions found (typically stale data). Refreshing moves those files to fresh NAND cells to restore read speed.`,
      engine,
    };
  }
  if (diag.dead_zones > 0) {
    return {
      title: "Clustered damaged zones detected",
      advice: `Bad sectors are clustered in ${diag.dead_zones} zone(s). Affected files should be relocated and those zones permanently isolated.`,
      engine,
    };
  }
  return {
    title: "Repairable sectors detected",
    advice: `${diag.unreadable} unreadable and ${diag.weak} weak sectors found scattered. Pinpoint repair relocates files first, then refreshes sectors to trigger drive reallocation.`,
    engine,
  };
}

export function translatePinpointLabel(
  text: string,
  lang: Lang = currentLang,
): string {
  if (lang === "tr") return text;
  const map: Record<string, string> = {
    "Zayıf sektör": "Weak sector",
    "Okunamayan sektör": "Unreadable sector",
    "Yavaş bölge": "Slow region",
    "Onarıldı": "Healed",
    "Kalıcı olarak kapatıldı": "Permanently isolated",
    "Tazelendi": "Refreshed",
    "Onarılamadı · kapatıldı": "Unrecoverable · isolated",
    "Taşındı": "Moved",
  };
  return map[text] ?? text;
}

export function translateTierAnalysis(
  analysis: {
    is_ssd: boolean;
    fill_percent: number;
    slc_note_tr: string;
    summary_tr: string;
    total_reclaimable_bytes: number;
    extreme_fragmented_count: number;
    worst_extents: number;
    letter: string;
    candidates: { id: string; is_dir: boolean; days_untouched: number; category: string; reason_tr: string }[];
  },
  formatBytes: (n: unknown) => string,
  lang: Lang = currentLang,
): { slcNote: string; summary: string; reasons: Map<string, string> } {
  const reasons = new Map<string, string>();
  if (lang === "tr") {
    for (const c of analysis.candidates) reasons.set(c.id, c.reason_tr);
    return {
      slcNote: analysis.slc_note_tr,
      summary: analysis.summary_tr,
      reasons,
    };
  }

  const fp = Number(analysis.fill_percent || 0);
  let slcNote = "";
  if (analysis.is_ssd) {
    slcNote =
      fp > 75
        ? `SSD is ${fp.toFixed(1)}% full; dynamic SLC write cache is shrinking and write speed may drop. Moving cold data to HDD is recommended.`
        : fp >= 55
          ? `SSD usage is moderate (${fp.toFixed(1)}%); SLC cache is healthy, though archiving large cold data preserves endurance.`
          : `SSD usage is low (${fp.toFixed(1)}%); dynamic SLC write cache operates at full capacity.`;
  } else {
    slcNote =
      fp > 85
        ? `HDD is ${fp.toFixed(1)}% full; files may spill onto slower inner tracks. Outer-track placement is recommended.`
        : fp >= 65
          ? `HDD is ${fp.toFixed(1)}% full; placing active folders on outer tracks or promoting to SSD improves access speed.`
          : `HDD is ${fp.toFixed(1)}% full; plenty of fast outer-track space is available.`;
  }

  let summary = "";
  const count = analysis.candidates.length;
  if (analysis.is_ssd) {
    summary =
      count === 0
        ? analysis.extreme_fragmented_count > 0
          ? `No large cold data to offload on ${analysis.letter}:, but ${analysis.extreme_fragmented_count} heavily fragmented (>=50 extents) files were found.`
          : `Drive ${analysis.letter}: looks balanced; no notable cold files detected.`
        : `${count} cold candidate(s) found on ${analysis.letter}: (${formatBytes(analysis.total_reclaimable_bytes)} reclaimable); max extents: ${analysis.worst_extents}.`;
  } else {
    summary =
      count === 0
        ? `No hot folders found for SSD promotion on ${analysis.letter}:; HDD Outer-Track Placement is ready.`
        : `${count} active hot candidate(s) found on ${analysis.letter}: (${formatBytes(analysis.total_reclaimable_bytes)}).`;
  }

  const catEn: Record<string, string> = {
    video: "video archive",
    archive: "compressed archive",
    iso: "disk image (ISO)",
    installer: "installer package",
    photo: "photo archive",
    audio: "audio archive",
    game: "game / app folder",
    code: "project / source folder",
    database: "database / config",
    folder: "user folder",
    other: "file",
  };
  for (const c of analysis.candidates) {
    const lbl = catEn[c.category] ?? "item";
    if (analysis.is_ssd) {
      reasons.set(
        c.id,
        c.is_dir
          ? `Untouched for ${c.days_untouched} days (${lbl}); can move to HDD via NTFS Junction without breaking shortcuts.`
          : `Unused for ${c.days_untouched} days (${lbl}); offloading to HDD relieves SSD SLC cache.`,
      );
    } else {
      const when =
        c.days_untouched === 0
          ? "Actively used today"
          : `Used within the last ${c.days_untouched} day(s)`;
      reasons.set(
        c.id,
        c.is_dir
          ? `${when} (${lbl}); promoting to SSD via NTFS Junction speeds up loads without breaking paths.`
          : `${when} (${lbl}); eligible for SSD promotion or outer-track placement.`,
      );
    }
  }

  return { slcNote, summary, reasons };
}

export function translateUpdateMessage(
  status: {
    status: string;
    latest?: string | null;
    message: string;
    message_en?: string;
  },
  lang: Lang = currentLang,
): string {
  if (lang === "tr") return status.message;
  if (status.message_en) return status.message_en;
  if (status.status === "unconfigured") {
    return "Release repository is not configured yet; check the project page for updates.";
  }
  if (status.status === "no_release") {
    return "No published release was found yet.";
  }
  if (status.status === "update_available") {
    return `A new version is available: ${status.latest ?? ""}`;
  }
  if (status.status === "up_to_date") {
    return "You are using the latest version.";
  }
  return status.message;
}
