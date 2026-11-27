// Turkish -> English translation of text that comes back from the Rust backend.
//
// The backend speaks the active UI language for everything it generates live, but two sources
// of Turkish remain: job records saved by older builds (SQLite) and the deep forensic engines.
// Everything the UI renders from the backend goes through `translateBackendText` so the English
// UI never shows Turkish. Turkish mode always passes text through untouched.
import { BACKEND_PAIRS } from "./i18n_backend_data.ts";

type Part = string | number;
interface Pattern {
  re: RegExp;
  parts: Part[];
  weight: number;
}

const exact = new Map<string, string>();
const patterns: Pattern[] = [];
const PLACEHOLDER = /\{([^{}]*)\}/g;

function placeholderKey(raw: string, positional: { n: number }): string {
  const name = raw.split(":")[0];
  return name === "" || /^\d+$/.test(name) ? `#${positional.n++}` : name;
}

function compile(tr: string, en: string): void {
  if (!tr.includes("{")) {
    exact.set(tr, en);
    return;
  }
  const trKeys: string[] = [];
  const posTr = { n: 0 };
  let source = "";
  let last = 0;
  for (const m of tr.matchAll(PLACEHOLDER)) {
    source += tr.slice(last, m.index).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    source += "(.+?)";
    trKeys.push(placeholderKey(m[1], posTr));
    last = (m.index ?? 0) + m[0].length;
  }
  source += tr.slice(last).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const parts: Part[] = [];
  const posEn = { n: 0 };
  last = 0;
  for (const m of en.matchAll(PLACEHOLDER)) {
    parts.push(en.slice(last, m.index));
    const group = trKeys.indexOf(placeholderKey(m[1], posEn));
    parts.push(group < 0 ? "" : group + 1);
    last = (m.index ?? 0) + m[0].length;
  }
  parts.push(en.slice(last));
  patterns.push({ re: new RegExp(`^${source}$`, "s"), parts, weight: tr.length });
}

for (const [tr, en] of BACKEND_PAIRS) compile(tr, en);
patterns.sort((a, b) => b.weight - a.weight);

function translateOne(text: string, depth: number): string | null {
  const hit = exact.get(text);
  if (hit !== undefined) return hit;
  if (depth > 4) return null;
  for (const p of patterns) {
    const m = p.re.exec(text);
    if (!m) continue;
    let out = "";
    for (const part of p.parts) {
      out += typeof part === "number" ? translateBackendFragment(m[part] ?? "", depth + 1) : part;
    }
    return out;
  }
  return null;
}

/**
 * Translates `pieces` greedily: at every position the longest run of pieces that forms a known
 * string wins, anything else is kept as is. Returns null when nothing matched.
 */
function translatePieces(pieces: string[], joiner: string, depth: number): string | null {
  const out: string[] = [];
  let changed = false;
  let i = 0;
  while (i < pieces.length) {
    let matched = false;
    for (let j = pieces.length; j > i; j--) {
      const t = translateOne(pieces.slice(i, j).join(joiner), depth + 1);
      if (t !== null) {
        out.push(t);
        i = j;
        matched = changed = true;
        break;
      }
    }
    if (!matched) out.push(pieces[i++]);
  }
  return changed ? out.join(joiner) : null;
}

function translateBackendFragment(text: string, depth: number): string {
  const direct = translateOne(text, depth);
  if (direct !== null) return direct;
  if (depth > 4) return text;
  for (const [splitter, joiner] of [
    [/(?<=[.;!?])\s+/, " "],
    [/\s+·\s+/, " · "],
  ] as const) {
    const pieces = text.split(splitter);
    if (pieces.length < 2) continue;
    const translated = translatePieces(pieces, joiner, depth);
    if (translated !== null) return translated;
  }
  return text;
}

/** Translates backend text to English; returns the input unchanged when no rule matches. */
export function translateBackendText(text: string | null | undefined, lang: "tr" | "en"): string {
  const value = text ?? "";
  if (lang !== "en" || !value) return value;
  return translateBackendFragment(value, 0);
}

/** True when `text` is covered by an exact entry or a template (used by tools/check_i18n.mjs). */
export function isBackendTextCovered(text: string): boolean {
  return translateOne(text, 0) !== null;
}
