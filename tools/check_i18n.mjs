// Permanent guard against Turkish text leaking into the English UI (and the reverse).
//
//   node tools/check_i18n.mjs                    run all checks (CI: `npm run check:i18n`)
//   node tools/check_i18n.mjs --update-baseline  re-record Rust strings that are intentionally
//                                                exempt from the backend translation table
//
// Rules
//  1. Frontend (src/*.ts, TypeScript AST)
//     - every `L(tr, en)` call has two non-empty arguments and the English one has no Turkish;
//     - a Turkish string may only appear as the first argument of `L(...)`, in the Turkish branch
//       of a `lang`/`en` conditional, in the translation tables (src/i18n.ts,
//       src/i18n_backend_data.ts), or as the search tokens in src/ui.ts.
//  2. index.html is English by default (`<html lang="en">`, no Turkish).
//  3. Backend (src-tauri/src/*.rs)
//     - no bare `phase(core, "<Turkish>")` progress text: use `phase_bi(core, tr, en)`;
//     - src/i18n_backend_data.ts is up to date (`npm run gen:i18n`);
//     - every Turkish string literal is covered by the translation table or listed in
//       tools/i18n_baseline.json, so new untranslated backend text fails CI.
import fs from "node:fs";
import path from "node:path";
import ts from "typescript";
import {
  TURKISH,
  buildMatcher,
  buildPairs,
  dataFile,
  productionSource,
  renderDataFile,
  repoRoot,
  rustFiles,
  rustTurkishLiterals,
} from "./gen_backend_i18n.mjs";

const baselineFile = path.join(repoRoot, "tools", "i18n_baseline.json");
const errors = [];
const fail = (where, message) => errors.push(`${where}: ${message}`);
const rel = (file) => path.relative(repoRoot, file).replace(/\\/g, "/");

// ---------------------------------------------------------------------------------------------
// 1. Frontend
// ---------------------------------------------------------------------------------------------
const TABLE_FILES = new Set(["i18n.ts", "i18n_backend_data.ts"]);
const SEARCH_TOKEN_FUNCTIONS = new Set(["aiNaturalFilter", "filterDeletedApps"]);

const isTextLiteral = (n) =>
  ts.isStringLiteral(n) ||
  ts.isNoSubstitutionTemplateLiteral(n) ||
  ts.isTemplateHead(n) ||
  ts.isTemplateMiddle(n) ||
  ts.isTemplateTail(n);

const unwrap = (n) => {
  while (n && ts.isParenthesizedExpression(n)) n = n.expression;
  return n;
};

/** Which branch of `cond` is Turkish? "true" | "false" | null (not a language condition). */
function turkishSide(cond) {
  cond = unwrap(cond);
  if (!cond) return null;
  const isLangExpr = (n) => {
    const text = n.getText();
    return /^(lang|getLang\(\))$/.test(text);
  };
  if (ts.isBinaryExpression(cond)) {
    const op = cond.operatorToken.kind;
    const eq = op === ts.SyntaxKind.EqualsEqualsEqualsToken || op === ts.SyntaxKind.EqualsEqualsToken;
    const ne = op === ts.SyntaxKind.ExclamationEqualsEqualsToken || op === ts.SyntaxKind.ExclamationEqualsToken;
    if (!eq && !ne) return null;
    let lit;
    let other;
    if (ts.isStringLiteral(cond.right)) [lit, other] = [cond.right, cond.left];
    else if (ts.isStringLiteral(cond.left)) [lit, other] = [cond.left, cond.right];
    else return null;
    if (!isLangExpr(other) || (lit.text !== "en" && lit.text !== "tr")) return null;
    if (eq) return lit.text === "en" ? "false" : "true";
    return lit.text === "en" ? "true" : "false";
  }
  if (ts.isIdentifier(cond) && (cond.text === "en" || cond.text === "isEn")) return "false";
  if (
    ts.isPrefixUnaryExpression(cond) &&
    cond.operator === ts.SyntaxKind.ExclamationToken &&
    ts.isIdentifier(unwrap(cond.operand)) &&
    ["en", "isEn"].includes(unwrap(cond.operand).text)
  ) {
    return "true";
  }
  return null;
}

const endsWithReturn = (stmt) => {
  if (!stmt) return false;
  if (ts.isReturnStatement(stmt) || ts.isThrowStatement(stmt)) return true;
  if (ts.isBlock(stmt)) return endsWithReturn(stmt.statements[stmt.statements.length - 1]);
  return false;
};

function inTurkishContext(node, fileName) {
  let child = node;
  let parent = node.parent;
  while (parent) {
    if (
      ts.isCallExpression(parent) &&
      ts.isIdentifier(parent.expression) &&
      parent.expression.text === "L" &&
      parent.arguments[0] === child
    ) {
      return true;
    }
    if (ts.isConditionalExpression(parent)) {
      const side = turkishSide(parent.condition);
      if (side === "true" && parent.whenTrue === child) return true;
      if (side === "false" && parent.whenFalse === child) return true;
    }
    if (ts.isIfStatement(parent)) {
      const side = turkishSide(parent.expression);
      if (side === "true" && parent.thenStatement === child) return true;
      if (side === "false" && parent.elseStatement === child) return true;
    }
    if (ts.isBlock(parent) || ts.isSourceFile(parent) || ts.isCaseClause(parent) || ts.isDefaultClause(parent)) {
      const stmts = parent.statements;
      const index = stmts.indexOf(child);
      for (let i = 0; i < index; i++) {
        const s = stmts[i];
        // `if (lang === "en") { ...; return }` -> everything after it is the Turkish path.
        if (ts.isIfStatement(s) && !s.elseStatement && turkishSide(s.expression) === "false" && endsWithReturn(s.thenStatement)) {
          return true;
        }
      }
    }
    if (
      fileName === "ui.ts" &&
      ts.isArrayLiteralExpression(parent) &&
      (() => {
        for (let n = parent.parent; n; n = n.parent) {
          if (ts.isFunctionDeclaration(n) && n.name && SEARCH_TOKEN_FUNCTIONS.has(n.name.text)) return true;
        }
        return false;
      })()
    ) {
      return true;
    }
    child = parent;
    parent = parent.parent;
  }
  return false;
}

function checkTypeScript() {
  const srcDir = path.join(repoRoot, "src");
  for (const name of fs.readdirSync(srcDir).filter((f) => f.endsWith(".ts"))) {
    const file = path.join(srcDir, name);
    const text = fs.readFileSync(file, "utf8");
    const sf = ts.createSourceFile(file, text, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TS);
    const where = (n) => `${rel(file)}:${sf.getLineAndCharacterOfPosition(n.getStart()).line + 1}`;
    const visit = (node) => {
      if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === "L") {
        if (node.arguments.length < 2) {
          fail(where(node), "L(tr, en) needs both a Turkish and an English argument");
        } else {
          const en = node.arguments[1];
          if (isTextLiteral(en) && !en.text.trim() && !ts.isTemplateHead(en)) {
            fail(where(en), "the English argument of L(tr, en) is empty");
          }
          const scan = (n) => {
            if (isTextLiteral(n) && TURKISH.test(n.text)) {
              fail(where(n), `Turkish text in the English argument of L(): ${JSON.stringify(n.text.slice(0, 60))}`);
            }
            ts.forEachChild(n, scan);
          };
          scan(en);
        }
      }
      if (!TABLE_FILES.has(name) && isTextLiteral(node) && TURKISH.test(node.text) && !(node.parent && ts.isLiteralTypeNode(node.parent))) {
        // The language selector option legitimately names Turkish in Turkish.
        const stripped = node.text.replace(/Türkçe \(TR\)/g, "");
        if (TURKISH.test(stripped) && !inTurkishContext(node, name)) {
          fail(
            where(node),
            `untranslated Turkish text outside L(tr, en) / a Turkish branch: ${JSON.stringify(node.text.slice(0, 70))}`,
          );
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
  }
}

// ---------------------------------------------------------------------------------------------
// 2. index.html
// ---------------------------------------------------------------------------------------------
function checkIndexHtml() {
  const file = path.join(repoRoot, "index.html");
  const html = fs.readFileSync(file, "utf8");
  if (!/<html[^>]*\blang="en"/i.test(html)) fail("index.html", 'the document must default to <html lang="en">');
  html.split(/\r?\n/).forEach((line, i) => {
    if (TURKISH.test(line)) fail(`index.html:${i + 1}`, "Turkish text in the static HTML shell");
  });
}

// ---------------------------------------------------------------------------------------------
// 3. Rust backend
// ---------------------------------------------------------------------------------------------
function checkRust() {
  const barePhase = /(?<![\w])phase\(\s*core\s*,\s*(?:&?format!\(\s*)?"((?:[^"\\]|\\.)*)"/g;
  for (const file of rustFiles()) {
    const src = productionSource(file);
    let m;
    barePhase.lastIndex = 0;
    while ((m = barePhase.exec(src))) {
      if (TURKISH.test(m[1])) {
        const line = src.slice(0, m.index).split("\n").length;
        fail(`${rel(file)}:${line}`, `Turkish progress text in phase(core, ..): use phase_bi(core, tr, en): ${JSON.stringify(m[1].slice(0, 60))}`);
      }
    }
  }

  const report = fs.readFileSync(path.join(repoRoot, "src-tauri", "src", "report.rs"), "utf8");
  if (!/html_report_renders_clean_english_without_turkish_leakage/.test(report) || !/"language"/.test(report)) {
    fail("src-tauri/src/report.rs", "the HTML report must be language-aware and keep its English-leakage test");
  }

  const pairs = buildPairs();
  const current = fs.existsSync(dataFile) ? fs.readFileSync(dataFile, "utf8").replace(/\r\n/g, "\n") : "";
  if (current !== renderDataFile(pairs)) {
    fail("src/i18n_backend_data.ts", "stale translation table; run `npm run gen:i18n` and commit the result");
  }

  const covered = buildMatcher(pairs);
  const baseline = new Set(
    fs.existsSync(baselineFile) ? (JSON.parse(fs.readFileSync(baselineFile, "utf8")).uncovered ?? []) : [],
  );
  const uncovered = new Map();
  for (const [file, line, text] of rustTurkishLiterals()) {
    if (covered(text)) continue;
    if (!uncovered.has(text)) uncovered.set(text, `src-tauri/src/${file}:${line}`);
  }
  if (process.argv.includes("--update-baseline")) {
    const body = {
      _comment:
        "Turkish Rust strings that are intentionally not in the backend translation table (they have a dedicated English path, are developer-only, or are dictionary data). New entries need a review: prefer core.t(tr, en) / phase_bi, or add a pair to tools/backend_i18n_manual.mjs.",
      uncovered: [...uncovered.keys()].sort((a, b) => a.localeCompare(b, "en")),
    };
    fs.writeFileSync(baselineFile, JSON.stringify(body, null, 1) + "\n");
    console.log(`Recorded ${body.uncovered.length} baseline strings in ${rel(baselineFile)}.`);
    return;
  }
  for (const [text, where] of uncovered) {
    if (!baseline.has(text)) {
      fail(where, `Turkish backend text has no English counterpart: ${JSON.stringify(text.slice(0, 80))}. Use core.t(tr, en) / phase_bi, or add a pair to tools/backend_i18n_manual.mjs and run \`npm run gen:i18n\``);
    }
  }
}

checkTypeScript();
checkIndexHtml();
checkRust();

if (process.argv.includes("--update-baseline")) process.exit(errors.length ? 1 : 0);
if (errors.length) {
  console.error(`i18n guard failed (${errors.length} problem${errors.length === 1 ? "" : "s"}):\n`);
  for (const e of errors) console.error(`  - ${e}`);
  console.error("\nEvery visible string needs a Turkish and an English version. See CONTRIBUTING.md.");
  process.exit(1);
}
console.log("i18n guard passed: frontend, static HTML and Rust backend strings are bilingual.");
