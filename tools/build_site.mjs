import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(__dirname, "..");
const siteSrc = path.join(rootDir, "site");
const outDir = path.join(rootDir, "_site");
const basePath = "/ssd-sentinel";

function assert(cond, message) {
  if (!cond) {
    throw new Error(`Site validation failed: ${message}`);
  }
}

function copyDir(src, dest) {
  fs.mkdirSync(dest, { recursive: true });
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    const srcPath = path.join(src, entry.name);
    const destPath = path.join(dest, entry.name);
    if (entry.isDirectory()) {
      copyDir(srcPath, destPath);
    } else {
      fs.copyFileSync(srcPath, destPath);
    }
  }
}

function listHtmlFiles(dir) {
  const results = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...listHtmlFiles(full));
    } else if (entry.name.endsWith(".html")) {
      results.push(full);
    }
  }
  return results;
}

fs.rmSync(outDir, { recursive: true, force: true });
copyDir(siteSrc, outDir);

const assetsDir = path.join(outDir, "assets");
const screenshotsOut = path.join(assetsDir, "screenshots");
fs.mkdirSync(screenshotsOut, { recursive: true });

for (const file of fs.readdirSync(path.join(rootDir, "docs", "screenshots"))) {
  if (file.endsWith(".png")) {
    fs.copyFileSync(path.join(rootDir, "docs", "screenshots", file), path.join(screenshotsOut, file));
  }
}

fs.copyFileSync(path.join(rootDir, "assets", "sentinel.svg"), path.join(assetsDir, "sentinel.svg"));
fs.copyFileSync(path.join(rootDir, "src-tauri", "icons", "128x128@2x.png"), path.join(assetsDir, "logo.png"));
fs.copyFileSync(path.join(rootDir, "src-tauri", "icons", "icon.ico"), path.join(outDir, "favicon.ico"));

const requiredFiles = [
  "index.html",
  "404.html",
  "styles.css",
  "site.js",
  "robots.txt",
  "sitemap.xml",
  "llms.txt",
  "favicon.ico",
  "assets/sentinel.svg",
  "assets/logo.png",
  "assets/screenshots/overview-dark-en.png",
  "assets/screenshots/overview-en.png",
  "assets/screenshots/optimize-tiering.png",
  "assets/screenshots/surface-diagnosis.png",
  "assets/screenshots/recovery-center.png",
  "crystaldiskinfo-alternative/index.html",
  "hard-disk-sentinel-alternative/index.html",
  "ssd-health-check/index.html",
  "nvme-lifespan/index.html",
  "benchmark/index.html",
  "faq/index.html",
];

for (const rel of requiredFiles) {
  assert(fs.existsSync(path.join(outDir, rel)), `Missing required output file: ${rel}`);
}

const robots = fs.readFileSync(path.join(outDir, "robots.txt"), "utf8");
assert(robots.includes("OAI-SearchBot"), "robots.txt must explicitly allow OAI-SearchBot");
assert(robots.includes("Sitemap: https://yuefdev.github.io/ssd-sentinel/sitemap.xml"), "robots.txt must declare sitemap.xml");

const sitemap = fs.readFileSync(path.join(outDir, "sitemap.xml"), "utf8");
const expectedUrls = [
  "https://yuefdev.github.io/ssd-sentinel/",
  "https://yuefdev.github.io/ssd-sentinel/crystaldiskinfo-alternative/",
  "https://yuefdev.github.io/ssd-sentinel/hard-disk-sentinel-alternative/",
  "https://yuefdev.github.io/ssd-sentinel/ssd-health-check/",
  "https://yuefdev.github.io/ssd-sentinel/nvme-lifespan/",
  "https://yuefdev.github.io/ssd-sentinel/benchmark/",
  "https://yuefdev.github.io/ssd-sentinel/faq/",
];
for (const url of expectedUrls) {
  assert(sitemap.includes(`<loc>${url}</loc>`), `sitemap.xml missing URL: ${url}`);
}

const htmlFiles = listHtmlFiles(outDir);
let jsonLdCount = 0;

for (const htmlFile of htmlFiles) {
  const relHtml = path.relative(outDir, htmlFile);
  const html = fs.readFileSync(htmlFile, "utf8");
  const is404 = path.basename(htmlFile) === "404.html";

  assert(/<title>[^<]+<\/title>/.test(html), `${relHtml} missing <title>`);
  if (!is404) {
    assert(/<meta\s+name="description"\s+content="[^"]+"/i.test(html), `${relHtml} missing meta description`);
    assert(/<link\s+rel="canonical"\s+href="https:\/\/yuefdev\.github\.io\/ssd-sentinel\/[^"]*"/i.test(html), `${relHtml} missing canonical link`);
    assert(/property="og:title"/i.test(html), `${relHtml} missing og:title`);
    assert(/property="og:description"/i.test(html), `${relHtml} missing og:description`);
    assert(/property="og:image"/i.test(html), `${relHtml} missing og:image`);
  }

  const ldRegex = /<script\s+type="application\/ld\+json">([\s\S]*?)<\/script>/gi;
  for (const match of html.matchAll(ldRegex)) {
    jsonLdCount += 1;
    try {
      JSON.parse(match[1]);
    } catch (err) {
      throw new Error(`Invalid JSON-LD in ${relHtml}: ${err.message}`);
    }
  }

  const attrRegex = /\b(?:href|src)="([^"]+)"/gi;
  for (const match of html.matchAll(attrRegex)) {
    const target = match[1];
    if (
      target.startsWith("http://") ||
      target.startsWith("https://") ||
      target.startsWith("#") ||
      target.startsWith("mailto:")
    ) {
      continue;
    }

    const cleanTarget = target.split("#")[0].split("?")[0];
    let resolved;
    if (cleanTarget.startsWith(`${basePath}/`) || cleanTarget === basePath) {
      const sub = cleanTarget.slice(basePath.length).replace(/^\/+/, "");
      resolved = path.join(outDir, sub);
    } else if (cleanTarget.startsWith("/")) {
      resolved = path.join(outDir, cleanTarget.replace(/^\/+/, ""));
    } else {
      resolved = path.resolve(path.dirname(htmlFile), cleanTarget);
    }

    const existsDirect = fs.existsSync(resolved) && fs.statSync(resolved).isFile();
    const existsDirIndex = fs.existsSync(path.join(resolved, "index.html"));
    assert(existsDirect || existsDirIndex, `Broken local link "${target}" in ${relHtml} (checked ${resolved})`);
  }
}

console.log(`Built ${htmlFiles.length} HTML pages cleanly (${jsonLdCount} valid JSON-LD blocks) -> _site/`);
