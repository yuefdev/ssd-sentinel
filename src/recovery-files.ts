import { L, localeTag, translateRecoveryText, type Lang } from "./i18n.ts";
import { icon } from "./icons.ts";
import type { RecoveredItem, RecoveryFileKind } from "./types";
import { aiNaturalFilter, bytes, escape as e, format, isRecoverableRow, isUnopenableMedia, mediaStatusBadge } from "./ui.ts";

export type RecoveryTab = "files" | "apps";
export type RecoveryFileFilters = {
  query: string;
  category: string;
  condition: "available" | "all" | "recoverable" | "trace" | "damaged";
  name: "all" | "recorded" | "carved";
  sort: "best" | "size" | "recent" | "name";
};

export const defaultRecoveryFileFilters: Readonly<RecoveryFileFilters> = {
  query: "",
  category: "all",
  condition: "available",
  name: "all",
  sort: "best",
};
export const recoveryPageSize = { files: 12, apps: 9 } as const;

const isTrace = (item: RecoveredItem) => item.source === "usn_journal" || item.source === "indx_slack";
const isDamaged = (item: RecoveredItem) => item.trim_zeroed || isUnopenableMedia(item);
const hasRecordedName = (item: RecoveredItem) => item.source !== "carved" && !!item.name?.trim();

const fallbackKinds: Record<string, RecoveryFileKind> = {
  photo: "photo", document: "document", video_audio: "media", archive: "archive",
  exe_setup: "installer", game_portable: "installer", system_lib: "system",
};
const fileKind = (item: RecoveredItem) => item.file_kind ?? fallbackKinds[item.category] ?? "other";
const priorityOrder: RecoveryFileKind[] = [
  "screenshot", "project", "photo", "document", "media", "archive", "other", "installer", "infrastructure", "system",
];
const availability = (item: RecoveredItem) => isRecoverableRow(item) ? 2 : isDamaged(item) ? 0 : 1;

function fileKindLabel(kind: RecoveryFileKind, lang: Lang): string {
  if (kind === "screenshot") return L("Ekran görüntüsü", "Screenshot", lang);
  if (kind === "project") return L("Proje dosyası", "Project file", lang);
  if (kind === "infrastructure") return L("Altyapı", "Support file", lang);
  return "";
}

export function recoveryFileName(item: RecoveredItem, lang: Lang): string {
  const name = item.name?.trim();
  const generated = item.source === "carved"
    ? name?.match(/^Kurtarilan_(Uygulama|Belge|Foto|Medya|Arsiv|Veritabani|Sistem|Dosya)_(\d+)(\.[a-z0-9]+)$/i)
    : null;
  if (generated) {
    const kinds: Record<string, string> = {
      uygulama: L("Uygulama", "App", lang),
      belge: L("Belge", "Document", lang),
      foto: L("Fotoğraf", "Photo", lang),
      medya: L("Medya", "Media", lang),
      arsiv: L("Arşiv", "Archive", lang),
      veritabani: L("Veritabanı", "Database", lang),
      sistem: L("Sistem dosyası", "System file", lang),
      dosya: L("Dosya", "File", lang),
    };
    return `${kinds[generated[1].toLowerCase()]} ${generated[2]}${generated[3]}`;
  }
  return name || item.product_name || L("Adsız dosya", "Unnamed file", lang);
}

function recoveryTimestamp(item: RecoveredItem) {
  const time = Date.parse(item.deleted_at ?? "");
  return Number.isFinite(time) ? time : Number.NEGATIVE_INFINITY;
}

export function filteredRecoveryFiles(items: RecoveredItem[], filters: RecoveryFileFilters, lang: Lang) {
  const category = filters.category;
  const categoryFilter = category === "screenshot" || category === "project" || category === "infrastructure"
    ? (item: RecoveredItem) => fileKind(item) === category
    : category;
  const matching = aiNaturalFilter(items, filters.query, categoryFilter).filter((item) => {
    if (filters.name === "recorded" && !hasRecordedName(item)) return false;
    if (filters.name === "carved" && hasRecordedName(item)) return false;
    switch (filters.condition) {
      case "available": return !isDamaged(item);
      case "recoverable": return isRecoverableRow(item);
      case "trace": return isTrace(item);
      case "damaged": return isDamaged(item);
      default: return true;
    }
  });
  switch (filters.sort) {
    case "size": return matching.sort((a, b) => b.size_bytes - a.size_bytes);
    case "recent": return matching.sort((a, b) => {
      const first = recoveryTimestamp(a);
      const second = recoveryTimestamp(b);
      return first === second ? 0 : second - first;
    });
    case "name": {
      const collator = new Intl.Collator(localeTag(lang), { numeric: true, sensitivity: "base" });
      return matching.sort((a, b) => collator.compare(recoveryFileName(a, lang), recoveryFileName(b, lang)));
    }
    default:
      return matching.sort((a, b) => availability(b) - availability(a)
        || priorityOrder.indexOf(fileKind(a)) - priorityOrder.indexOf(fileKind(b)));
  }
}

export function recoveryPageWindow<T>(items: T[], requestedPage: number, tab: RecoveryTab) {
  const size = recoveryPageSize[tab];
  const totalPages = Math.max(1, Math.ceil(items.length / size));
  const page = Math.max(1, Math.min(requestedPage, totalPages));
  const offset = (page - 1) * size;
  return { page, totalPages, offset, total: items.length, rows: items.slice(offset, offset + size) };
}

export function recoveryPager(
  result: ReturnType<typeof recoveryPageWindow>,
  lang: Lang,
  unit: string,
  tools = "",
): string {
  const number = (n: number) => format(n, 0, lang);
  const { offset, rows, total, page, totalPages } = result;
  const count = totalPages > 1 ? `${number(offset + 1)}–${number(offset + rows.length)} / ${number(total)}` : number(total);
  const previous = L("Önceki sayfa", "Previous page", lang);
  const next = L("Sonraki sayfa", "Next page", lang);
  return `<div class="recovery-results-bar">
    <div class="recovery-result-summary"><p class="recovery-result-count" role="status"><strong>${count}</strong> ${unit}</p>${tools}</div>
    ${totalPages > 1 ? `<nav class="recovery-pager" aria-label="${L("Sonuç sayfaları", "Result pages", lang)}">
      <button type="button" id="recovery-prev" class="button compact quiet" data-rec-page="${page - 1}" aria-label="${previous}" title="${previous}" ${page === 1 ? "disabled" : ""}>${icon("chevron", "recovery-prev-icon")}</button>
      <select id="recovery-page" aria-label="${L("Sayfaya git", "Go to page", lang)}">${Array.from({ length: totalPages }, (_, i) => {
        const n = i + 1;
        return `<option value="${n}" ${n === page ? "selected" : ""}>${L(`Sayfa ${number(n)} / ${number(totalPages)}`, `Page ${number(n)} / ${number(totalPages)}`, lang)}</option>`;
      }).join("")}</select>
      <button type="button" id="recovery-next" class="button compact quiet" data-rec-page="${page + 1}" aria-label="${next}" title="${next}" ${page === totalPages ? "disabled" : ""}>${icon("chevron")}</button>
    </nav>` : ""}
  </div>`;
}

const fileType = (item: RecoveredItem, lang: Lang) => translateRecoveryText(item.sub_type, lang);

function fileStatus(item: RecoveredItem, lang: Lang) {
  if (isTrace(item)) return { label: L("Yalnız iz", "Trace only", lang), level: "muted" };
  if (item.trim_zeroed) return { label: L("Veri sıfırlanmış", "Data zeroed", lang), level: "critical" };
  if (isUnopenableMedia(item)) return { label: L("Açılamaz", "Unopenable", lang), level: "critical" };
  const media = mediaStatusBadge(item, lang);
  if (media) return media;
  return item.confidence >= 75
    ? { label: L("Kurtarılabilir", "Recoverable", lang), level: "ok" }
    : { label: L("Bütünlük belirsiz", "Integrity uncertain", lang), level: "warning" };
}

function fileDate(item: RecoveredItem, lang: Lang) {
  const time = recoveryTimestamp(item);
  return time === Number.NEGATIVE_INFINITY ? "—" : new Date(time).toLocaleDateString(localeTag(lang), {
    day: "2-digit", month: "short", year: "numeric",
  });
}

function fileSize(item: RecoveredItem, lang: Lang) {
  return item.size_bytes > 0 ? bytes(item.size_bytes, lang) : "—";
}

function fileLocation(item: RecoveredItem, lang: Lang) {
  if (item.source === "carved") return L("Özgün ad ve konum bulunamadı", "Original name and location unavailable", lang);
  const path = item.original_path?.trim();
  if (!path) return L("Konum bulunamadı", "Location unavailable", lang);
  const end = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return end >= 0 ? path.slice(0, end + 1) : path;
}

function fileRow(item: RecoveredItem, jobId: string, selected: boolean, canRestore: boolean, pending: boolean, lang: Lang) {
  const name = recoveryFileName(item, lang);
  const extension = item.extension?.trim().toUpperCase();
  const type = fileType(item, lang);
  const location = fileLocation(item, lang);
  const kind = fileKind(item);
  const kindLabel = fileKindLabel(kind, lang);
  const status = fileStatus(item, lang);
  const statusMarkup = `<span class="recovery-file-status ${status.level}">${e(status.label)}</span>`;
  const fileIcon = extension && extension.length <= 4 ? e(extension) : icon(kind === "project" ? "folder" : "smart");
  const recoverable = isRecoverableRow(item);
  const preview = recoverable ? L("Önizle", "Preview", lang) : L("Ayrıntılar", "Details", lang);
  const recover = L("Kurtar", "Recover", lang);
  return `<tr class="recovery-row${selected ? " row-selected" : ""}" data-file-kind="${e(kind)}">
    <td class="recovery-check-col"><input type="checkbox" class="rec-checkbox" id="rec-check-${e(item.id)}" data-rec-select="${e(item.id)}" ${selected ? "checked" : ""} ${!recoverable || pending ? "disabled" : ""} aria-label="${L(`${e(name)} dosyasını seç`, `Select ${e(name)}`, lang)}"></td>
    <td><div class="recovery-file">
      <span class="recovery-file-icon" data-file-category="${e(item.category)}" aria-hidden="true">${fileIcon}</span>
      <div class="recovery-file-text">
        <strong title="${e(name)}" dir="auto">${e(name)}</strong>
        <div class="recovery-file-meta">
          ${kindLabel ? `<span class="recovery-file-kind">${kindLabel}</span>` : ""}
          <small title="${e(location)}" dir="auto">${e(location)}</small>
        </div>
        ${item.product_name && item.product_name !== name ? `<small title="${e(item.product_name)}" dir="auto">${e(item.product_name)}</small>` : ""}
        <span class="recovery-file-mobile">${statusMarkup}${item.size_bytes > 0 ? ` <span>${fileSize(item, lang)}</span>` : ""}</span>
      </div>
    </div></td>
    <td class="recovery-type-col" title="${e(type)}">${e(type) || "—"}</td>
    <td class="recovery-size-col">${fileSize(item, lang)}</td>
    <td class="recovery-date-col">${fileDate(item, lang)}</td>
    <td class="recovery-status-col">${statusMarkup}</td>
    <td class="recovery-actions-col"><div class="recovery-row-actions">
      <button type="button" id="rec-preview-${e(item.id)}" class="button compact quiet" data-preview-job="${e(jobId)}" data-preview-id="${e(item.id)}" title="${preview}" aria-label="${L(`${e(name)} · ${preview}`, `${preview} ${e(name)}`, lang)}" ${pending ? "disabled" : ""}>${icon(recoverable ? "eye" : "info")}<span>${preview}</span></button>
      ${recoverable ? `<button type="button" class="button compact primary" data-restore-job="${e(jobId)}" data-restore-id="${e(item.id)}" title="${recover}" aria-label="${L(`${e(name)} · ${recover}`, `${recover} ${e(name)}`, lang)}" ${!canRestore || pending ? "disabled" : ""}>${icon("export")}<span>${recover}</span></button>` : ""}
    </div></td>
  </tr>`;
}

export function recoveryFileTable(
  rows: RecoveredItem[],
  jobId: string,
  selected: ReadonlySet<string>,
  canRestore: boolean,
  pending: boolean,
  lang: Lang,
): string {
  const selectable = rows.filter(isRecoverableRow);
  const selectedCount = selectable.filter((item) => selected.has(item.id)).length;
  const selectPage = L("Bu sayfadaki kurtarılabilir dosyaları seç", "Select recoverable files on this page", lang);
  return `<table class="recovery-list" aria-label="${L("Bulunan dosyalar", "Files found", lang)}">
    <colgroup><col class="recovery-check-col"><col><col class="recovery-type-col"><col class="recovery-size-col"><col class="recovery-date-col"><col class="recovery-status-col"><col class="recovery-actions-col"></colgroup>
    <thead><tr>
      <th scope="col" class="recovery-check-col"><input type="checkbox" class="rec-checkbox" id="rec-select-page" title="${selectPage}" aria-label="${selectPage}" ${selectedCount > 0 && selectedCount === selectable.length ? "checked" : ""} ${selectedCount > 0 && selectedCount < selectable.length ? "data-partial" : ""} ${selectable.length === 0 || pending ? "disabled" : ""}></th>
      <th scope="col">${L("Dosya", "File", lang)}</th>
      <th scope="col" class="recovery-type-col">${L("Tür", "Type", lang)}</th>
      <th scope="col" class="recovery-size-col">${L("Boyut", "Size", lang)}</th>
      <th scope="col" class="recovery-date-col">${L("Kayıt tarihi", "Recorded date", lang)}</th>
      <th scope="col" class="recovery-status-col">${L("Durum", "Status", lang)}</th>
      <th scope="col" class="recovery-actions-col"><span class="sr-only">${L("İşlemler", "Actions", lang)}</span></th>
    </tr></thead>
    <tbody>${rows.map((item) => fileRow(item, jobId, selected.has(item.id), canRestore, pending, lang)).join("")}</tbody>
  </table>`;
}

export function recoveryFileDetails(item: RecoveredItem, sourceLabel: string, open: boolean, lang: Lang): string {
  const generated = item.source === "carved";
  const confidence = Number.isFinite(item.confidence) ? format(item.confidence, 0, lang) : "";
  const summary = translateRecoveryText(item.ai_summary, lang);
  const repair = translateRecoveryText(item.repair_note, lang);
  const facts = [
    [generated ? L("Taramanın verdiği ad", "Name assigned by scan", lang) : L("Dosya adı", "Filename", lang), item.name],
    [L("Dosya yolu", "File path", lang), generated ? fileLocation(item, lang) : item.original_path],
    [L("Dosya grubu", "File group", lang), fileKindLabel(fileKind(item), lang)],
    [L("Tür", "Type", lang), fileType(item, lang) || item.extension],
    [L("Boyut", "Size", lang), fileSize(item, lang)],
    [L("Kayıt tarihi", "Recorded date", lang), fileDate(item, lang)],
    [L("Kaynak", "Source", lang), sourceLabel],
    [L("Durum", "Status", lang), translateRecoveryText(item.status_label, lang) || fileStatus(item, lang).label],
    [L("Bütünlük tahmini", "Estimated integrity", lang), confidence ? L(`%${confidence}`, `${confidence}%`, lang) : ""],
    [L("Ürün adı", "Product name", lang), item.product_name],
    [L("Yayıncı", "Publisher", lang), item.publisher],
  ];
  return `<details class="recovery-file-details" ${open ? "open" : ""}>
    <summary>${L("Dosya bilgileri", "File information", lang)}</summary>
    <dl class="preview-kv">${facts.filter(([, value]) => !!value && value !== "—").map(([label, value]) => `<dt>${label}</dt><dd dir="auto">${e(value)}</dd>`).join("")}</dl>
    ${summary ? `<p>${e(summary)}</p>` : ""}
    ${repair && !summary.includes(repair) ? `<p>${e(repair)}</p>` : ""}
  </details>`;
}
