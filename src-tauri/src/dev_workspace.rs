//! Developer & vibe-coding storage workspace.
//!
//! * Strictly deterministic (zero AI/ML, zero shell interpolation, zero extra crates).
//! * Analysis is **read-only** (metadata + directory sizing only).
//! * The only mutating helper is [`clean_dev_artifact_path`], which can delete nothing except
//!   manifest-paired rebuildable build artifacts (`node_modules`, `target`, `.venv`, ...) and a
//!   short allowlist of user-profile package caches. Source code, `.git` and `.env*` are never
//!   touched and nested reparse points are unlinked, never followed.

use crate::{model::Disk, process, tiering};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant, SystemTime},
};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;
pub const DEV_SCAN_TIME_BUDGET: Duration = Duration::from_millis(4500);
pub const DEV_CACHE_TIME_BUDGET: Duration = Duration::from_millis(3000);
const DEV_MAX_DIRS: usize = 20_000;
const DEV_MAX_SIZE_ENTRIES: usize = 600_000;
const DEV_MAX_DEPTH: usize = 4;
const STALE_DAYS: u64 = 14;
const MIN_CACHE_BYTES: u64 = 10 * MIB;
const HDD_TARGET_MIN_FREE: u64 = 10 * GIB;
const SSD_TARGET_MIN_FREE: u64 = 20 * GIB;
const MAX_CLEAN_DEPTH: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevArtifact {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub clean_token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevProject {
    pub id: String,
    pub name: String,
    pub path: String,
    pub stack: String,
    pub days_untouched: u64,
    pub stale: bool,
    pub has_git: bool,
    pub has_env: bool,
    pub recent_git_edit: bool,
    pub artifact_bytes: u64,
    pub artifacts: Vec<DevArtifact>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevCacheItem {
    pub id: String,
    pub name: String,
    pub name_en: String,
    pub kind: String,
    pub path: String,
    pub size_bytes: u64,
    pub file_count: u64,
    pub days_untouched: u64,
    pub is_junction: bool,
    pub can_offload: bool,
    pub can_clean: bool,
    pub clean_token: Option<String>,
    pub note_tr: String,
    pub note_en: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WslVhdxItem {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub path: String,
    pub drive_letter: String,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DevWorkspaceAnalysis {
    pub disk_id: String,
    pub letter: String,
    pub replay: bool,
    pub scanned_roots: Vec<String>,
    pub scan_ms: u64,
    pub truncated: bool,
    pub total_artifact_bytes: u64,
    pub stale_artifact_bytes: u64,
    pub total_cache_bytes: u64,
    pub total_vhdx_bytes: u64,
    pub total_reclaimable_bytes: u64,
    pub offload_target: Option<Value>,
    pub projects: Vec<DevProject>,
    pub global_caches: Vec<DevCacheItem>,
    pub vhdx_files: Vec<WslVhdxItem>,
    pub summary_tr: String,
    pub summary_en: String,
}

// ---------------------------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------------------------

struct ArtifactRule {
    name: &'static str,
    needs: &'static [&'static str],
}

const JS_NEEDS: &[&str] = &["package.json"];
const BUILD_NEEDS: &[&str] = &[
    "package.json",
    "cargo.toml",
    "pyproject.toml",
    "build.gradle",
    "build.gradle.kts",
    "pubspec.yaml",
];
const PY_NEEDS: &[&str] = &["pyproject.toml", "requirements.txt", "pipfile"];
const GRADLE_NEEDS: &[&str] = &["build.gradle", "build.gradle.kts"];
const DOTNET_NEEDS: &[&str] = &["*.csproj", "*.sln"];

const ARTIFACT_RULES: &[ArtifactRule] = &[
    ArtifactRule {
        name: "node_modules",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".next",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".nuxt",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".svelte-kit",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".astro",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".turbo",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".parcel-cache",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".vite",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: ".angular",
        needs: JS_NEEDS,
    },
    ArtifactRule {
        name: "target",
        needs: &["cargo.toml", "pom.xml"],
    },
    ArtifactRule {
        name: "dist",
        needs: BUILD_NEEDS,
    },
    ArtifactRule {
        name: "build",
        needs: BUILD_NEEDS,
    },
    ArtifactRule {
        name: "out",
        needs: BUILD_NEEDS,
    },
    ArtifactRule {
        name: ".cache",
        needs: BUILD_NEEDS,
    },
    ArtifactRule {
        name: ".venv",
        needs: PY_NEEDS,
    },
    ArtifactRule {
        name: "venv",
        needs: PY_NEEDS,
    },
    ArtifactRule {
        name: "__pycache__",
        needs: PY_NEEDS,
    },
    ArtifactRule {
        name: ".pytest_cache",
        needs: PY_NEEDS,
    },
    ArtifactRule {
        name: ".mypy_cache",
        needs: PY_NEEDS,
    },
    ArtifactRule {
        name: ".ruff_cache",
        needs: PY_NEEDS,
    },
    ArtifactRule {
        name: ".gradle",
        needs: GRADLE_NEEDS,
    },
    ArtifactRule {
        name: ".dart_tool",
        needs: &["pubspec.yaml"],
    },
    ArtifactRule {
        name: "bin",
        needs: DOTNET_NEEDS,
    },
    ArtifactRule {
        name: "obj",
        needs: DOTNET_NEEDS,
    },
];

const PROJECT_MANIFESTS: &[&str] = &[
    "package.json",
    "cargo.toml",
    "pyproject.toml",
    "requirements.txt",
    "pipfile",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "pubspec.yaml",
];

const STACK_PRIORITY: &[(&str, &str)] = &[
    ("package.json", "Node / JS"),
    ("cargo.toml", "Rust"),
    ("pyproject.toml", "Python"),
    ("requirements.txt", "Python"),
    ("pipfile", "Python"),
    ("go.mod", "Go"),
    ("pom.xml", "Java / Gradle"),
    ("build.gradle", "Java / Gradle"),
    ("build.gradle.kts", "Java / Gradle"),
    ("*.csproj", ".NET"),
    ("*.sln", ".NET"),
    ("pubspec.yaml", "Flutter / Dart"),
];

const ROOT_DEV_DIRS: &[&str] = &[
    "Projects",
    "Projeler",
    "Dev",
    "Code",
    "Repos",
    "Work",
    "Workspace",
    "src",
    "git",
    "github",
    "apps",
    "sites",
];

const USER_DEV_DIRS: &[&str] = &[
    "source\\repos",
    "Projects",
    "projects",
    "Projeler",
    "Dev",
    "dev",
    "Code",
    "code",
    "Workspace",
    "workspace",
    "work",
    "repos",
    "src",
    "git",
    "github",
    "Documents",
    "Desktop",
];

const MONOREPO_DIRS: &[&str] = &["packages", "apps", "crates", "services", "libs"];

const SKIP_USER_PROFILES: &[&str] = &[
    "public",
    "default",
    "default user",
    "all users",
    "defaultuser0",
    "wdagutilityaccount",
];

struct CacheRule {
    rel: &'static [&'static str],
    kind: &'static str,
    name: &'static str,
    name_en: &'static str,
    can_clean: bool,
    note_tr: &'static str,
    note_en: &'static str,
}

const CACHE_RULES: &[CacheRule] = &[
    CacheRule {
        rel: &[".ollama", "models"],
        kind: "ai_model",
        name: "Ollama yerel LLM modelleri",
        name_en: "Ollama local LLM models",
        can_clean: false,
        note_tr: "Büyük model dosyaları; NTFS Junction ile ikinci diske taşınırsa Ollama yolu değişmeden çalışır.",
        note_en: "Large model weights; an NTFS Junction keeps the Ollama path working after moving them.",
    },
    CacheRule {
        rel: &[".cache", "huggingface"],
        kind: "ai_model",
        name: "Hugging Face model önbelleği",
        name_en: "Hugging Face model cache",
        can_clean: false,
        note_tr: "İndirilen transformer / diffusion modelleri.",
        note_en: "Downloaded transformer / diffusion models.",
    },
    CacheRule {
        rel: &[".lmstudio", "models"],
        kind: "ai_model",
        name: "LM Studio modelleri",
        name_en: "LM Studio models",
        can_clean: false,
        note_tr: "LM Studio yerel GGUF modelleri.",
        note_en: "LM Studio local GGUF models.",
    },
    CacheRule {
        rel: &[".cache", "lm-studio"],
        kind: "ai_model",
        name: "LM Studio önbelleği (eski konum)",
        name_en: "LM Studio cache (legacy path)",
        can_clean: false,
        note_tr: "Eski LM Studio model konumu.",
        note_en: "Legacy LM Studio model location.",
    },
    CacheRule {
        rel: &[".rustup", "toolchains"],
        kind: "toolchain",
        name: "Rust araç zincirleri (rustup)",
        name_en: "Rust toolchains (rustup)",
        can_clean: false,
        note_tr: "Her sürüm 1-2 GB; yalnız taşınabilir, silinmez.",
        note_en: "1-2 GB per toolchain; can be moved but is never deleted.",
    },
    CacheRule {
        rel: &[".cache", "ms-playwright"],
        kind: "toolchain",
        name: "Playwright tarayıcıları",
        name_en: "Playwright browsers",
        can_clean: false,
        note_tr: "İndirilmiş Chromium / Firefox / WebKit paketleri.",
        note_en: "Downloaded Chromium / Firefox / WebKit bundles.",
    },
    CacheRule {
        rel: &[".cargo", "registry", "cache"],
        kind: "pkg_cache",
        name: "Cargo crate önbelleği",
        name_en: "Cargo crate cache",
        can_clean: true,
        note_tr: "Gerektiğinde yeniden indirilir.",
        note_en: "Re-downloaded on demand.",
    },
    CacheRule {
        rel: &[".cargo", "git"],
        kind: "pkg_cache",
        name: "Cargo git bağımlılıkları",
        name_en: "Cargo git dependencies",
        can_clean: true,
        note_tr: "Git tabanlı crate checkout'ları; yeniden çekilir.",
        note_en: "Git-based crate checkouts; re-fetched on demand.",
    },
    CacheRule {
        rel: &[".npm"],
        kind: "pkg_cache",
        name: "npm önbelleği",
        name_en: "npm cache",
        can_clean: true,
        note_tr: "npm install sırasında yeniden oluşur.",
        note_en: "Rebuilt automatically by npm install.",
    },
    CacheRule {
        rel: &[".pnpm-store"],
        kind: "pkg_cache",
        name: "pnpm içerik deposu",
        name_en: "pnpm content store",
        can_clean: true,
        note_tr: "pnpm install sırasında yeniden oluşur.",
        note_en: "Rebuilt automatically by pnpm install.",
    },
    CacheRule {
        rel: &[".bun", "install", "cache"],
        kind: "pkg_cache",
        name: "Bun paket önbelleği",
        name_en: "Bun package cache",
        can_clean: true,
        note_tr: "bun install sırasında yeniden oluşur.",
        note_en: "Rebuilt automatically by bun install.",
    },
    CacheRule {
        rel: &[".nuget", "packages"],
        kind: "pkg_cache",
        name: "NuGet paket önbelleği",
        name_en: "NuGet package cache",
        can_clean: true,
        note_tr: "dotnet restore ile yeniden oluşur.",
        note_en: "Rebuilt by dotnet restore.",
    },
    CacheRule {
        rel: &[".gradle", "caches"],
        kind: "pkg_cache",
        name: "Gradle önbelleği",
        name_en: "Gradle caches",
        can_clean: true,
        note_tr: "Sonraki derlemede yeniden indirilir.",
        note_en: "Re-downloaded on the next build.",
    },
    CacheRule {
        rel: &[".m2", "repository"],
        kind: "pkg_cache",
        name: "Maven yerel deposu",
        name_en: "Maven local repository",
        can_clean: true,
        note_tr: "Bağımlılıklar yeniden indirilir.",
        note_en: "Dependencies are re-downloaded.",
    },
    CacheRule {
        rel: &[".cache", "uv"],
        kind: "pkg_cache",
        name: "uv Python önbelleği",
        name_en: "uv Python cache",
        can_clean: true,
        note_tr: "uv sync sırasında yeniden oluşur.",
        note_en: "Rebuilt by uv sync.",
    },
    CacheRule {
        rel: &[".cache", "pip"],
        kind: "pkg_cache",
        name: "pip önbelleği",
        name_en: "pip cache",
        can_clean: true,
        note_tr: "pip install sırasında yeniden oluşur.",
        note_en: "Rebuilt by pip install.",
    },
    CacheRule {
        rel: &[".cache", "yarn"],
        kind: "pkg_cache",
        name: "Yarn önbelleği",
        name_en: "Yarn cache",
        can_clean: true,
        note_tr: "yarn install sırasında yeniden oluşur.",
        note_en: "Rebuilt by yarn install.",
    },
    CacheRule {
        rel: &[".local", "share", "pnpm", "store"],
        kind: "pkg_cache",
        name: "pnpm XDG içerik deposu",
        name_en: "pnpm XDG content store",
        can_clean: true,
        note_tr: "pnpm install sırasında yeniden oluşur.",
        note_en: "Rebuilt by pnpm install.",
    },
    CacheRule {
        rel: &["go", "pkg", "mod", "cache"],
        kind: "pkg_cache",
        name: "Go modül önbelleği",
        name_en: "Go module cache",
        can_clean: true,
        note_tr: "go build/mod download ile yeniden indirilir.",
        note_en: "Re-downloaded by go build.",
    },
    CacheRule {
        rel: &[".cache", "go-build"],
        kind: "pkg_cache",
        name: "Go derleme önbelleği",
        name_en: "Go build cache",
        can_clean: true,
        note_tr: "Sonraki go build sırasında yeniden oluşur.",
        note_en: "Rebuilt on next go build.",
    },
    CacheRule {
        rel: &[".cache", "yay"],
        kind: "pkg_cache",
        name: "yay AUR derleme önbelleği",
        name_en: "yay AUR build cache",
        can_clean: true,
        note_tr: "Eski AUR kaynak ve paket artıkları.",
        note_en: "Stale AUR build artifacts and packages.",
    },
    CacheRule {
        rel: &[".cache", "paru"],
        kind: "pkg_cache",
        name: "paru AUR derleme önbelleği",
        name_en: "paru AUR build cache",
        can_clean: true,
        note_tr: "Eski AUR kaynak ve paket artıkları.",
        note_en: "Stale AUR build artifacts and packages.",
    },
    CacheRule {
        rel: &[".local", "share", "koboldcpp"],
        kind: "ai_model",
        name: "KoboldCpp modelleri",
        name_en: "KoboldCpp models",
        can_clean: false,
        note_tr: "Yerel yapay zeka modelleri; silinmez, yalnızca taşınabilir.",
        note_en: "Local AI models; never deleted, offloadable only.",
    },
];

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

fn sha16(text: &str) -> String {
    let full = format!("{:x}", Sha256::digest(text.as_bytes()));
    full[..16].to_owned()
}

/// Case-insensitive, separator-normalized key used for ids and confirmation tokens.
pub fn path_key(path: &str) -> String {
    let trimmed = path.strip_prefix("\\\\?\\").unwrap_or(path);
    trimmed
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// Deterministic IPC confirmation token required by `Core::clean_dev_artifact`.
pub fn clean_token(disk_id: &str, path: &str) -> String {
    format!("dev_clean:{disk_id}:{}", sha16(&path_key(path)))
}

fn meta_is_reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn path_is_reparse(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| meta_is_reparse(&m))
        .unwrap_or(false)
}

fn days_since(time: SystemTime) -> u64 {
    SystemTime::now()
        .duration_since(time)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

fn fmt_gib(bytes: u64, tr: bool) -> String {
    let text = format!("{:.1} GiB", bytes as f64 / GIB as f64);
    if tr {
        text.replace('.', ",")
    } else {
        text
    }
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

struct Budget {
    deadline: Instant,
    dirs: usize,
    entries: usize,
    exhausted: bool,
}

impl Budget {
    fn new(time: Duration) -> Self {
        Self {
            deadline: Instant::now() + time,
            dirs: 0,
            entries: 0,
            exhausted: false,
        }
    }

    fn tick_entry(&mut self) -> bool {
        if self.exhausted {
            return false;
        }
        self.entries += 1;
        if self.entries > DEV_MAX_SIZE_ENTRIES
            || (self.entries.is_multiple_of(256) && Instant::now() >= self.deadline)
        {
            self.exhausted = true;
            return false;
        }
        true
    }

    fn tick_dir(&mut self) -> bool {
        if self.exhausted {
            return false;
        }
        self.dirs += 1;
        if self.dirs > DEV_MAX_DIRS || Instant::now() >= self.deadline {
            self.exhausted = true;
            return false;
        }
        true
    }
}

/// Bytes and file count below `root`; nested reparse points are skipped, never followed.
fn dir_size(root: &Path, budget: &mut Budget) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            if !budget.tick_entry() {
                return (bytes, files);
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_symlink() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta_is_reparse(&meta) {
                continue;
            }
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                bytes = bytes.saturating_add(meta.len());
                files += 1;
            }
        }
    }
    (bytes, files)
}

fn modified_of(path: &Path) -> Option<SystemTime> {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
}

// ---------------------------------------------------------------------------------------------
// Project discovery
// ---------------------------------------------------------------------------------------------

struct ProjectProbe {
    manifests: HashSet<String>,
    has_git: bool,
}

fn probe_project(dir: &Path) -> Option<ProjectProbe> {
    let read = std::fs::read_dir(dir).ok()?;
    let mut manifests = HashSet::new();
    let mut has_git = false;
    for entry in read.flatten().take(5_000) {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name == ".git" {
            has_git = true;
        } else if PROJECT_MANIFESTS.contains(&name.as_str()) {
            manifests.insert(name);
        } else if name.ends_with(".csproj") {
            manifests.insert("*.csproj".to_owned());
        } else if name.ends_with(".sln") {
            manifests.insert("*.sln".to_owned());
        }
    }
    if manifests.is_empty() && !has_git {
        None
    } else {
        Some(ProjectProbe { manifests, has_git })
    }
}

fn detect_stack(probe: &ProjectProbe) -> &'static str {
    for (key, label) in STACK_PRIORITY {
        if probe.manifests.contains(*key) {
            return label;
        }
    }
    "Git Repo"
}

fn rule_for(name_lower: &str) -> Option<&'static ArtifactRule> {
    ARTIFACT_RULES.iter().find(|r| r.name == name_lower)
}

fn rule_satisfied(rule: &ArtifactRule, manifests: &HashSet<String>) -> bool {
    rule.needs.iter().any(|n| manifests.contains(*n))
}

fn dir_has_protected_marker(dir: &Path) -> bool {
    [".git", ".env", ".env.local"]
        .iter()
        .any(|m| dir.join(m).exists())
}

fn has_env_file(dir: &Path) -> bool {
    dir.join(".env").is_file() || dir.join(".env.local").is_file()
}

/// Newest mtime among non-artifact entries so rebuildable caches never mask a project's real age.
fn newest_source_mtime(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut bump = |t: Option<SystemTime>| {
        if let Some(t) = t {
            if newest.is_none_or(|n| t > n) {
                newest = Some(t);
            }
        }
    };
    let read = std::fs::read_dir(dir).ok()?;
    for entry in read.flatten().take(2_000) {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let path = entry.path();
        if name == ".git" {
            for rel in ["index", "HEAD"] {
                bump(modified_of(&path.join(rel)));
            }
            continue;
        }
        if rule_for(&name).is_some() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta_is_reparse(&meta) {
            continue;
        }
        bump(meta.modified().ok());
        if meta.is_dir() && !name.starts_with('.') {
            if let Ok(inner) = std::fs::read_dir(&path) {
                for child in inner.flatten().take(200) {
                    if let Ok(child_meta) = child.metadata() {
                        if !meta_is_reparse(&child_meta) {
                            bump(child_meta.modified().ok());
                        }
                    }
                }
            }
        }
    }
    newest
}

fn recent_git_edit(dir: &Path) -> bool {
    let index = modified_of(&dir.join(".git").join("index"));
    let head_log = modified_of(&dir.join(".git").join("logs").join("HEAD"));
    matches!((index, head_log), (Some(i), Some(h)) if i > h)
}

fn build_project(
    dir: &Path,
    probe: &ProjectProbe,
    disk_id: &str,
    budget: &mut Budget,
) -> DevProject {
    let mut artifacts = Vec::new();
    if let Ok(read) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries.into_iter().take(5_000) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            let Some(rule) = rule_for(&lower) else {
                continue;
            };
            if !rule_satisfied(rule, &probe.manifests) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_dir() || meta_is_reparse(&meta) {
                continue;
            }
            let path = entry.path();
            if dir_has_protected_marker(&path) {
                continue;
            }
            let (size, files) = dir_size(&path, budget);
            if files == 0 {
                continue;
            }
            let path_str = path.to_string_lossy().into_owned();
            artifacts.push(DevArtifact {
                id: format!("art-{}", sha16(&path_key(&path_str))),
                name,
                clean_token: clean_token(disk_id, &path_str),
                path: path_str,
                size_bytes: size,
                file_count: files,
            });
        }
    }
    let artifact_bytes = artifacts.iter().map(|a| a.size_bytes).sum();
    let days = newest_source_mtime(dir).map(days_since).unwrap_or(0);
    let path_str = dir.to_string_lossy().into_owned();
    DevProject {
        id: format!("prj-{}", sha16(&path_key(&path_str))),
        name: dir
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone()),
        stack: detect_stack(probe).to_owned(),
        days_untouched: days,
        stale: days >= STALE_DAYS,
        has_git: probe.has_git,
        has_env: has_env_file(dir),
        recent_git_edit: recent_git_edit(dir),
        artifact_bytes,
        artifacts,
        path: path_str,
    }
}

fn is_skipped_dir_name(lower: &str) -> bool {
    lower.starts_with('.') || rule_for(lower).is_some() || lower == "node_modules"
}

fn discover_projects(
    roots: &[PathBuf],
    disk_id: &str,
    forbidden: &dyn Fn(&Path) -> bool,
    budget: &mut Budget,
) -> Vec<DevProject> {
    let mut projects = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize, Option<u64>)> = roots
        .iter()
        .rev()
        .map(|r| {
            #[cfg(unix)]
            let dev = {
                use std::os::unix::fs::MetadataExt;
                std::fs::metadata(r).map(|m| m.dev()).ok()
            };
            #[cfg(not(unix))]
            let dev = None;
            (r.clone(), 0, dev)
        })
        .collect();
    while let Some((dir, depth, root_dev)) = stack.pop() {
        if budget.exhausted || !budget.tick_dir() {
            break;
        }
        let key = path_key(&dir.to_string_lossy());
        if !seen.insert(key) || forbidden(&dir) {
            continue;
        }
        let probe = probe_project(&dir);
        let is_project = probe.is_some();
        if let Some(probe) = &probe {
            projects.push(build_project(&dir, probe, disk_id, budget));
        }
        if depth >= DEV_MAX_DEPTH {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in read.flatten().take(4_000) {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            #[cfg(unix)]
            if let Some(r_dev) = root_dev {
                use std::os::unix::fs::MetadataExt;
                if meta.dev() != r_dev {
                    continue;
                }
            }
            if !meta.is_dir() || meta_is_reparse(&meta) {
                continue;
            }
            let lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if is_skipped_dir_name(&lower) {
                continue;
            }
            if is_project && !MONOREPO_DIRS.contains(&lower.as_str()) {
                continue;
            }
            children.push(entry.path());
        }
        children.sort();
        for child in children.into_iter().rev() {
            stack.push((child, depth + 1, root_dev));
        }
    }
    projects.sort_by(|a, b| {
        b.artifact_bytes
            .cmp(&a.artifact_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    projects
}

/// Scans a single folder read-only for CLI `dev` inspection while enforcing system-path protections.
pub fn scan_directory_projects(root: &Path) -> Vec<DevProject> {
    let is_sys_forbidden =
        |p: &Path| crate::tiering::is_forbidden_path(p) || crate::platform::reparse(p);
    let mut budget = Budget::new(Duration::from_secs(12));
    discover_projects(&[root.to_path_buf()], "cli", &is_sys_forbidden, &mut budget)
}

// ---------------------------------------------------------------------------------------------
// Global caches, AI models & VHDX discovery
// ---------------------------------------------------------------------------------------------

fn cache_item(
    rule: &CacheRule,
    path: &Path,
    disk_id: &str,
    budget: &mut Budget,
    min_bytes: u64,
) -> Option<DevCacheItem> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() && !meta_is_reparse(&meta) {
        return None;
    }
    let is_junction = meta_is_reparse(&meta);
    let (size, files) = dir_size(path, budget);
    if !is_junction && size < min_bytes {
        return None;
    }
    let path_str = path.to_string_lossy().into_owned();
    let days = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(days_since)
        .unwrap_or(0);
    let can_clean = rule.can_clean && !is_junction;
    Some(DevCacheItem {
        id: format!("cache-{}", sha16(&path_key(&path_str))),
        name: rule.name.to_owned(),
        name_en: rule.name_en.to_owned(),
        kind: rule.kind.to_owned(),
        size_bytes: size,
        file_count: files,
        days_untouched: days,
        is_junction,
        can_offload: !is_junction,
        can_clean,
        clean_token: can_clean.then(|| clean_token(disk_id, &path_str)),
        note_tr: rule.note_tr.to_owned(),
        note_en: rule.note_en.to_owned(),
        path: path_str,
    })
}

fn scan_global_caches(
    profiles: &[PathBuf],
    disk_id: &str,
    forbidden: &dyn Fn(&Path) -> bool,
    budget: &mut Budget,
    min_bytes: u64,
) -> Vec<DevCacheItem> {
    let mut out = Vec::new();
    for profile in profiles {
        for rule in CACHE_RULES {
            if budget.exhausted {
                break;
            }
            let mut path = profile.clone();
            for part in rule.rel {
                path.push(part);
            }
            if forbidden(&path) && !path_is_reparse(&path) {
                continue;
            }
            if let Some(item) = cache_item(rule, &path, disk_id, budget, min_bytes) {
                out.push(item);
            }
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
    out
}

fn vhdx_item(path: &Path, kind: &str) -> Option<WslVhdxItem> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta_is_reparse(&meta) {
        return None;
    }
    let path_str = path.to_string_lossy().into_owned();
    let letter = drive_letter(&path_str).unwrap_or_else(|| {
        path_str
            .strip_prefix("\\\\?\\")
            .unwrap_or(&path_str)
            .chars()
            .next()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase().to_string())
            .unwrap_or_default()
    });
    Some(WslVhdxItem {
        id: format!("vhdx-{}", sha16(&path_key(&path_str))),
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        kind: kind.to_owned(),
        drive_letter: letter,
        size_bytes: meta.len(),
        modified_at: meta.modified().ok().map(rfc3339),
        path: path_str,
    })
}

fn local_app_data() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    }
}

#[cfg(windows)]
fn lxss_base_paths() -> Vec<PathBuf> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        RRF_RT_REG_SZ,
    };

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
    let sub = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Lxss");
    let mut root: HKEY = null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut root) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for index in 0..64u32 {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        let rc = unsafe {
            RegEnumKeyExW(
                root,
                index,
                name.as_mut_ptr(),
                &mut len,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        if rc != 0 {
            break;
        }
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        let value = wide("BasePath");
        let rc = unsafe {
            RegGetValueW(
                root,
                name.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if rc == 0 {
            let chars = (size as usize / 2).min(buf.len());
            let end = buf[..chars].iter().position(|&c| c == 0).unwrap_or(chars);
            let base = String::from_utf16_lossy(&buf[..end]);
            if !base.is_empty() {
                out.push(PathBuf::from(base.strip_prefix("\\\\?\\").unwrap_or(&base)));
            }
        }
    }
    unsafe {
        RegCloseKey(root);
    }
    out
}

#[cfg(not(windows))]
fn lxss_base_paths() -> Vec<PathBuf> {
    Vec::new()
}

/// Directories under which WSL / Docker Desktop VHDX (or Linux VM/container disk images) are recognized.
pub fn wsl_vhdx_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(base) = local_app_data() {
        roots.push(base.join("Docker").join("wsl"));
        roots.push(base.join("Packages"));
        roots.push(base.join("wsl"));
        #[cfg(target_os = "linux")]
        {
            roots.push(base.join("libvirt").join("images"));
            roots.push(base.join("gnome-boxes").join("images"));
            roots.push(base.join("containers").join("storage"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        roots.push(PathBuf::from("/var/lib/libvirt/images"));
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            roots.push(PathBuf::from(home).join("VirtualBox VMs"));
        }
    }
    roots.extend(lxss_base_paths());
    roots
}

fn vhdx_candidates() -> Vec<(PathBuf, &'static str)> {
    let mut out: Vec<(PathBuf, &'static str)> = Vec::new();
    if let Some(base) = local_app_data() {
        out.push((
            base.join("Docker")
                .join("wsl")
                .join("disk")
                .join("docker_data.vhdx"),
            "docker_vhdx",
        ));
        out.push((
            base.join("Docker")
                .join("wsl")
                .join("data")
                .join("ext4.vhdx"),
            "docker_vhdx",
        ));
        for (parent, tail) in [
            (base.join("Packages"), Some("LocalState")),
            (base.join("wsl"), None),
        ] {
            let Ok(read) = std::fs::read_dir(parent) else {
                continue;
            };
            for entry in read.flatten().take(500) {
                let mut candidate = entry.path();
                if let Some(tail) = tail {
                    candidate.push(tail);
                }
                candidate.push("ext4.vhdx");
                out.push((candidate, "wsl_vhdx"));
            }
        }
        #[cfg(target_os = "linux")]
        for vm_dir in [
            base.join("libvirt").join("images"),
            base.join("gnome-boxes").join("images"),
        ] {
            if let Ok(read) = std::fs::read_dir(vm_dir) {
                for entry in read.flatten().take(200) {
                    let p = entry.path();
                    if p.extension().is_some_and(|e| {
                        matches!(
                            e.to_string_lossy().to_ascii_lowercase().as_str(),
                            "qcow2" | "img" | "vdi" | "vhdx"
                        )
                    }) {
                        out.push((p, "vm_disk"));
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(read) = std::fs::read_dir("/var/lib/libvirt/images") {
            for entry in read.flatten().take(200) {
                let p = entry.path();
                if p.extension().is_some_and(|e| {
                    matches!(
                        e.to_string_lossy().to_ascii_lowercase().as_str(),
                        "qcow2" | "img" | "vdi" | "vhdx"
                    )
                }) {
                    out.push((p, "vm_disk"));
                }
            }
        }
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            let vbox = PathBuf::from(home).join("VirtualBox VMs");
            if let Ok(read) = std::fs::read_dir(vbox) {
                for vm in read.flatten().take(100) {
                    if let Ok(files) = std::fs::read_dir(vm.path()) {
                        for f in files.flatten().take(50) {
                            let p = f.path();
                            if p.extension().is_some_and(|e| {
                                e.eq_ignore_ascii_case("vdi") || e.eq_ignore_ascii_case("vmdk")
                            }) {
                                out.push((p, "vm_disk"));
                            }
                        }
                    }
                }
            }
        }
    }
    for base in lxss_base_paths() {
        out.push((base.join("ext4.vhdx"), "wsl_vhdx"));
    }
    out
}

fn scan_wsl_vhdx_files(candidates: &[(PathBuf, &'static str)], letter: &str) -> Vec<WslVhdxItem> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (path, kind) in candidates {
        if let Some(item) = vhdx_item(path, kind) {
            let matches_vol = if letter.is_empty() {
                true
            } else if letter.starts_with('/') {
                item.drive_letter == letter
            } else {
                item.drive_letter.eq_ignore_ascii_case(letter)
            };
            if matches_vol && seen.insert(path_key(&item.path)) {
                out.push(item);
            }
        }
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.size_bytes));
    out
}

/// Validates a `.vhdx` (or Linux `.qcow2`/`.img`/`.vdi`) path destined for compaction.
pub fn validate_wsl_vhdx_path_with_roots(raw: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    let text = raw.trim();
    if text.is_empty() || text.len() > 520 {
        return Err("VHDX yolu geçersiz.".into());
    }
    if text.contains("..")
        || text.chars().any(|c| {
            c.is_control()
                || matches!(
                    c,
                    '\'' | '"' | '`' | '$' | ';' | '|' | '&' | '<' | '>' | '\r' | '\n'
                )
        })
    {
        return Err("VHDX yolu yasaklı karakter veya üst dizin içeriyor.".into());
    }
    let path = PathBuf::from(text);
    let name = file_name_lower(&path);
    let valid_linux_img = cfg!(target_os = "linux")
        && (name.ends_with(".qcow2")
            || name.ends_with(".img")
            || name.ends_with(".vdi")
            || matches!(name.as_str(), "ext4.vhdx" | "docker_data.vhdx"));
    if !matches!(name.as_str(), "ext4.vhdx" | "docker_data.vhdx") && !valid_linux_img {
        return Err("Yalnız WSL / Docker ext4.vhdx ve docker_data.vhdx sıkıştırılabilir.".into());
    }
    let meta = std::fs::symlink_metadata(&path).map_err(|_| "VHDX dosyası bulunamadı.")?;
    if meta_is_reparse(&meta) || !meta.is_file() {
        return Err("VHDX yolu normal bir dosya olmalıdır (reparse point reddedildi).".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "VHDX yolu çözümlenemedi.".to_owned())?;
    let canon_key = path_key(&canonical.to_string_lossy());
    let allowed = roots.iter().any(|root| {
        let base = root
            .canonicalize()
            .map(|c| path_key(&c.to_string_lossy()))
            .unwrap_or_else(|_| path_key(&root.to_string_lossy()));
        !base.is_empty() && canon_key.starts_with(&format!("{base}\\"))
    });
    if !allowed {
        return Err("VHDX dosyası tanınan WSL / Docker konumlarının dışında.".into());
    }
    let clean = canonical.to_string_lossy().into_owned();
    Ok(PathBuf::from(
        clean.strip_prefix("\\\\?\\").unwrap_or(&clean).to_owned(),
    ))
}

pub fn validate_wsl_vhdx_path(raw: &str) -> Result<PathBuf, String> {
    validate_wsl_vhdx_path_with_roots(raw, &wsl_vhdx_roots())
}

// ---------------------------------------------------------------------------------------------
// Offload target & analysis
// ---------------------------------------------------------------------------------------------

/// Best NTFS companion volume on a *different* physical disk (HDD preferred, then spare SSD).
pub fn find_dev_offload_target(disks: &[Disk], source_disk_id: &str) -> Option<Value> {
    let mut best: Option<(bool, u64, Value)> = None;
    for disk in disks.iter().filter(|d| d.id != source_disk_id) {
        let is_hdd = disk.kind == "HDD";
        let is_ssd = tiering::is_ssd_disk(disk);
        if !is_hdd && !is_ssd {
            continue;
        }
        let min_free = if is_hdd {
            HDD_TARGET_MIN_FREE
        } else {
            SSD_TARGET_MIN_FREE
        };
        for volume in &disk.volumes {
            if cfg!(windows) && volume["filesystem"] != "NTFS" {
                continue;
            }
            let Some(letter) = volume["letter"].as_str().filter(|l| {
                if cfg!(windows) {
                    l.len() == 1
                } else {
                    crate::platform::is_valid_volume_id(l)
                }
            }) else {
                continue;
            };
            let free = volume["free_bytes"].as_u64().unwrap_or(0);
            if free < min_free {
                continue;
            }
            let norm_letter = if letter.starts_with('/') {
                letter.to_owned()
            } else {
                letter.to_ascii_uppercase()
            };
            let candidate = json!({
                "disk_id": disk.id,
                "model": disk.model,
                "letter": norm_letter,
                "type": disk.kind,
                "free_bytes": free,
                "direction": if is_hdd { "to_hdd" } else { "to_archive" }
            });
            let better = match &best {
                None => true,
                Some((best_hdd, best_free, _)) => {
                    (is_hdd && !*best_hdd) || (is_hdd == *best_hdd && free > *best_free)
                }
            };
            if better {
                best = Some((is_hdd, free, candidate));
            }
        }
    }
    best.map(|(_, _, v)| v)
}

pub struct ScanConfig<'a> {
    pub roots: Vec<PathBuf>,
    pub profile_dirs: Vec<PathBuf>,
    pub vhdx: Vec<(PathBuf, &'static str)>,
    pub min_cache_bytes: u64,
    pub project_budget: Duration,
    pub cache_budget: Duration,
    pub forbidden: &'a dyn Fn(&Path) -> bool,
}

/// Core analysis over explicit roots (unit-testable without a live Windows volume).
pub fn analyze_dev_workspace_root(
    cfg: &ScanConfig<'_>,
    disk_id: &str,
    letter: &str,
) -> DevWorkspaceAnalysis {
    let started = Instant::now();
    let mut project_budget = Budget::new(cfg.project_budget);
    let projects = discover_projects(&cfg.roots, disk_id, cfg.forbidden, &mut project_budget);
    let mut cache_budget = Budget::new(cfg.cache_budget);
    let global_caches = scan_global_caches(
        &cfg.profile_dirs,
        disk_id,
        cfg.forbidden,
        &mut cache_budget,
        cfg.min_cache_bytes,
    );
    let vhdx_files = scan_wsl_vhdx_files(&cfg.vhdx, letter);

    let total_artifact_bytes: u64 = projects.iter().map(|p| p.artifact_bytes).sum();
    let stale_artifact_bytes: u64 = projects
        .iter()
        .filter(|p| p.stale)
        .map(|p| p.artifact_bytes)
        .sum();
    let total_cache_bytes: u64 = global_caches
        .iter()
        .filter(|c| !c.is_junction)
        .map(|c| c.size_bytes)
        .sum();
    let total_vhdx_bytes: u64 = vhdx_files.iter().map(|v| v.size_bytes).sum();
    let reclaimable_cache: u64 = global_caches
        .iter()
        .filter(|c| c.can_clean)
        .map(|c| c.size_bytes)
        .sum();
    let total_reclaimable_bytes = stale_artifact_bytes.saturating_add(reclaimable_cache);
    let truncated = project_budget.exhausted || cache_budget.exhausted;

    let summary_tr = format!(
        "{} proje bulundu; yeniden üretilebilir yük {} ({} 14+ gün eski), yerel model ve paket önbellekleri {}, WSL/Docker VHDX {}.{}",
        projects.len(),
        fmt_gib(total_artifact_bytes, true),
        fmt_gib(stale_artifact_bytes, true),
        fmt_gib(total_cache_bytes, true),
        fmt_gib(total_vhdx_bytes, true),
        if truncated { " Tarama süre sınırında kesildi." } else { "" }
    );
    let summary_en = format!(
        "{} projects found; rebuildable load {} ({} untouched 14+ days), local model & package caches {}, WSL/Docker VHDX {}.{}",
        projects.len(),
        fmt_gib(total_artifact_bytes, false),
        fmt_gib(stale_artifact_bytes, false),
        fmt_gib(total_cache_bytes, false),
        fmt_gib(total_vhdx_bytes, false),
        if truncated { " Scan stopped at its time budget." } else { "" }
    );

    DevWorkspaceAnalysis {
        disk_id: disk_id.to_owned(),
        letter: letter.to_owned(),
        replay: false,
        scanned_roots: cfg
            .roots
            .iter()
            .map(|r| r.to_string_lossy().into_owned())
            .collect(),
        scan_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        truncated,
        total_artifact_bytes,
        stale_artifact_bytes,
        total_cache_bytes,
        total_vhdx_bytes,
        total_reclaimable_bytes,
        offload_target: None,
        projects,
        global_caches,
        vhdx_files,
        summary_tr,
        summary_en,
    }
}

fn list_profile_dirs(letter: &str) -> Vec<PathBuf> {
    if letter.starts_with('/') {
        let mut out = Vec::new();
        let mount_path = Path::new(letter);
        let home_dir = if letter == "/" {
            PathBuf::from("/home")
        } else if mount_path.file_name().is_some_and(|n| n == "home") {
            mount_path.to_path_buf()
        } else {
            mount_path.join("home")
        };
        if let Ok(read) = std::fs::read_dir(&home_dir) {
            for entry in read.flatten() {
                let lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if lower != "lost+found"
                    && entry
                        .metadata()
                        .map(|m| m.is_dir() && !meta_is_reparse(&m))
                        .unwrap_or(false)
                {
                    out.push(entry.path());
                }
            }
        }
        if let Some(env_home) = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
        {
            if drive_letter(&env_home.to_string_lossy()).as_deref() == Some(letter)
                && env_home.is_dir()
                && !path_is_reparse(&env_home)
                && !out.contains(&env_home)
            {
                out.push(env_home);
            }
        }
        out.sort();
        return out;
    }
    let users = PathBuf::from(format!("{letter}:\\Users"));
    let Ok(read) = std::fs::read_dir(users) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = read
        .flatten()
        .filter(|entry| {
            let lower = entry.file_name().to_string_lossy().to_ascii_lowercase();
            !SKIP_USER_PROFILES.contains(&lower.as_str())
                && entry
                    .metadata()
                    .map(|m| m.is_dir() && !meta_is_reparse(&m))
                    .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

fn build_roots(letter: &str, profiles: &[PathBuf], custom: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(custom) = custom {
        candidates.push(custom.to_path_buf());
    }
    if letter.starts_with('/') {
        let base = Path::new(letter);
        for name in ROOT_DEV_DIRS {
            candidates.push(base.join(name));
        }
    } else {
        for name in ROOT_DEV_DIRS {
            candidates.push(PathBuf::from(format!("{letter}:\\{name}")));
        }
    }
    for profile in profiles {
        for name in USER_DEV_DIRS {
            if letter.starts_with('/') && name.contains('\\') {
                candidates.push(profile.join(name.replace('\\', "/")));
            } else {
                candidates.push(profile.join(name));
            }
        }
    }
    #[cfg(unix)]
    let root_dev = if letter.starts_with('/') {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(letter).map(|m| m.dev()).ok()
    } else {
        None
    };
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|p| {
            let meta_ok = std::fs::metadata(p).is_ok_and(|m| {
                #[cfg(unix)]
                if let Some(r_dev) = root_dev {
                    use std::os::unix::fs::MetadataExt;
                    if m.dev() != r_dev {
                        return false;
                    }
                }
                m.is_dir()
            });
            meta_ok && !path_is_reparse(p) && seen.insert(path_key(&p.to_string_lossy()))
        })
        .collect()
}

pub fn validate_custom_root(
    raw: &str,
    letter: &str,
    forbidden: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, String> {
    let text = raw.trim();
    if text.is_empty() || text.len() > 520 || text.chars().any(char::is_control) {
        return Err("Proje klasörü yolu geçersiz.".into());
    }
    let path = PathBuf::from(text);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Proje klasörü yolu üst dizin içeremez.".into());
    }
    if letter.starts_with('/') {
        if !text.starts_with('/') {
            return Err("Proje klasörü seçili Linux bağlama noktasında olmalıdır.".into());
        }
        if drive_letter(text).as_deref() != Some(letter) {
            return Err("Proje klasörü seçili bölümde olmalıdır.".into());
        }
    } else {
        let path_letter = text
            .strip_prefix("\\\\?\\")
            .unwrap_or(text)
            .chars()
            .next()
            .filter(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase().to_string())
            .ok_or("Proje klasörü bir sürücü harfiyle başlamalıdır.")?;
        if !path_letter.eq_ignore_ascii_case(letter) {
            return Err("Proje klasörü seçili bölümde olmalıdır.".into());
        }
    }
    let meta = std::fs::symlink_metadata(&path).map_err(|_| "Proje klasörü bulunamadı.")?;
    if !meta.is_dir() || meta_is_reparse(&meta) {
        return Err("Proje klasörü normal bir klasör olmalıdır.".into());
    }
    if forbidden(&path) {
        return Err("Sistem veya yasaklı klasör taranamaz.".into());
    }
    Ok(path)
}

/// Live, strictly read-only analysis for one NTFS volume.
pub fn analyze_dev_workspace(
    disks: &[Disk],
    selected: &Disk,
    letter: &str,
    custom_root: Option<&str>,
) -> Result<Value, String> {
    let forbidden: &dyn Fn(&Path) -> bool = &tiering::is_forbidden_path;
    let custom = match custom_root.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => Some(validate_custom_root(raw, letter, forbidden)?),
        None => None,
    };
    let profiles = list_profile_dirs(letter);
    let roots = build_roots(letter, &profiles, custom.as_deref());
    let cfg = ScanConfig {
        roots,
        profile_dirs: profiles,
        vhdx: vhdx_candidates(),
        min_cache_bytes: MIN_CACHE_BYTES,
        project_budget: DEV_SCAN_TIME_BUDGET,
        cache_budget: DEV_CACHE_TIME_BUDGET,
        forbidden,
    };
    let mut analysis = analyze_dev_workspace_root(&cfg, &selected.id, letter);
    analysis.offload_target = find_dev_offload_target(disks, &selected.id);
    serde_json::to_value(analysis).map_err(|e| e.to_string())
}

/// Snapshot replay never touches the filesystem.
pub fn analyze_dev_workspace_replay(disks: &[Disk], selected: &Disk, letter: &str) -> Value {
    let analysis = DevWorkspaceAnalysis {
        disk_id: selected.id.clone(),
        letter: letter.to_owned(),
        replay: true,
        scanned_roots: Vec::new(),
        scan_ms: 0,
        truncated: false,
        total_artifact_bytes: 0,
        stale_artifact_bytes: 0,
        total_cache_bytes: 0,
        total_vhdx_bytes: 0,
        total_reclaimable_bytes: 0,
        offload_target: find_dev_offload_target(disks, &selected.id),
        projects: Vec::new(),
        global_caches: Vec::new(),
        vhdx_files: Vec::new(),
        summary_tr: "Kayıt modunda canlı proje taraması kapalıdır; canlı bir makinede proje ve önbellek analizi etkinleşir.".into(),
        summary_en: "Live project scanning is disabled in snapshot replay; switch to a live machine to analyze projects and caches.".into(),
    };
    serde_json::to_value(analysis).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------------------------
// Safe deletion
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanKind {
    ProjectArtifact,
    PackageCache,
}

fn normal_components_lower(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(os) => Some(os.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// Accepts only manifest-paired rebuildable artifacts or allowlisted user-profile package caches.
pub fn validate_cleanable_dev_path(
    path: &Path,
    forbidden: &dyn Fn(&Path) -> bool,
) -> Result<CleanKind, String> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("Geçersiz temizlik yolu.".into());
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| "Temizlenecek klasör bulunamadı.")?;
    if !meta.is_dir() || meta_is_reparse(&meta) {
        return Err(
            "Yalnız normal klasörler temizlenebilir (junction / sembolik bağ reddedildi).".into(),
        );
    }
    if forbidden(path) {
        return Err("Sistem veya yasaklı yol temizlik için reddedildi.".into());
    }
    let name = file_name_lower(path);
    if let Some(rule) = rule_for(&name) {
        let parent = path.parent().ok_or("Üst klasör belirlenemedi.")?;
        if path_is_reparse(parent) {
            return Err("Üst klasör bir reparse point; temizlik reddedildi.".into());
        }
        let probe = probe_project(parent).ok_or(
            "Üst klasörde tanınan bir proje bildirimi (package.json, Cargo.toml, ...) yok.",
        )?;
        if !rule_satisfied(rule, &probe.manifests) {
            return Err("Bu klasör için gerekli proje bildirimi üst klasörde bulunamadı.".into());
        }
        if dir_has_protected_marker(path) {
            return Err(".git veya .env içeren klasör temizlenemez.".into());
        }
        return Ok(CleanKind::ProjectArtifact);
    }
    let comps = normal_components_lower(path);
    if CACHE_RULES.iter().filter(|r| r.can_clean).any(|r| {
        let user_profile_match = comps.len() >= r.rel.len() + 2 && {
            let users_idx = comps.len() - r.rel.len() - 2;
            (comps[users_idx] == "users" || comps[users_idx] == "home")
                && comps[users_idx + 2..]
                    .iter()
                    .zip(r.rel)
                    .all(|(a, b)| a == b)
        };
        let root_profile_match = comps.len() > r.rel.len() && {
            let root_idx = comps.len() - r.rel.len() - 1;
            comps[root_idx] == "root"
                && comps[root_idx + 1..].iter().zip(r.rel).all(|(a, b)| a == b)
        };
        user_profile_match || root_profile_match
    }) {
        return Ok(CleanKind::PackageCache);
    }
    Err("Bu klasör güvenli temizlik izin listesinde değil.".into())
}

struct CleanStats {
    bytes: u64,
    files: u64,
    skipped: u64,
}

fn clear_readonly(path: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let mut perms = meta.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

fn remove_tree(
    dir: &Path,
    depth: usize,
    cancel: &AtomicBool,
    stats: &mut CleanStats,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    if depth > MAX_CLEAN_DEPTH {
        stats.skipped += 1;
        return Ok(());
    }
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => {
            stats.skipped += 1;
            return Ok(());
        }
    };
    for entry in read.flatten() {
        if cancel.load(Ordering::SeqCst) {
            return Err(process::CANCELLED.into());
        }
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            stats.skipped += 1;
            continue;
        };
        if meta_is_reparse(&meta) {
            // Unlink the junction / symlink itself; never recurse into its target.
            let removed = std::fs::remove_dir(&path).or_else(|_| std::fs::remove_file(&path));
            if removed.is_err() {
                stats.skipped += 1;
            }
            continue;
        }
        if meta.is_dir() {
            remove_tree(&path, depth + 1, cancel, stats, on_progress)?;
            if std::fs::remove_dir(&path).is_err() {
                stats.skipped += 1;
            }
            continue;
        }
        let size = meta.len();
        let mut result = std::fs::remove_file(&path);
        if result.is_err() {
            clear_readonly(&path);
            result = std::fs::remove_file(&path);
        }
        match result {
            Ok(()) => {
                stats.bytes = stats.bytes.saturating_add(size);
                stats.files += 1;
                if stats.files.is_multiple_of(200) {
                    on_progress(stats.files, stats.bytes);
                }
            }
            Err(_) => stats.skipped += 1,
        }
    }
    Ok(())
}

/// Deletes a validated rebuildable artifact directory. Cancellation is safe: everything removed so
/// far is regenerable by the project's package manager / compiler.
pub fn clean_dev_artifact_path(
    path: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64),
    forbidden: &dyn Fn(&Path) -> bool,
) -> Result<Value, String> {
    let kind = validate_cleanable_dev_path(path, forbidden)?;
    if cancel.load(Ordering::SeqCst) {
        return Err(process::CANCELLED.into());
    }
    let mut stats = CleanStats {
        bytes: 0,
        files: 0,
        skipped: 0,
    };
    remove_tree(path, 0, cancel, &mut stats, on_progress)?;
    let root_removed = std::fs::remove_dir(path).is_ok();
    let path_str = path.to_string_lossy().into_owned();
    Ok(json!({
        "status": "completed",
        "path": path_str,
        "kind": if kind == CleanKind::ProjectArtifact { "project_artifact" } else { "package_cache" },
        "freed_bytes": stats.bytes,
        "removed_files": stats.files,
        "skipped": stats.skipped,
        "root_removed": root_removed,
        "message_tr": format!(
            "{} temizlendi: {} serbest bırakıldı ({} dosya{}).",
            path_str,
            fmt_gib(stats.bytes, true),
            stats.files,
            if stats.skipped > 0 { format!(", {} öğe kilitli olduğu için atlandı", stats.skipped) } else { String::new() }
        ),
        "message": format!(
            "{} cleaned: {} freed ({} files{}).",
            path_str,
            fmt_gib(stats.bytes, false),
            stats.files,
            if stats.skipped > 0 { format!(", {} locked items skipped", stats.skipped) } else { String::new() }
        )
    }))
}

/// Production wrapper that applies the real 25-entry system path firewall.
pub fn clean_dev_artifact_checked(
    path: &Path,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, u64),
) -> Result<Value, String> {
    clean_dev_artifact_path(path, cancel, on_progress, &tiering::is_forbidden_path)
}

/// Volume letter (`C`) of an absolute Windows path, or Linux mount point (`/home`) of a POSIX path.
pub fn drive_letter(path: &str) -> Option<String> {
    let text = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let bytes = text.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return Some((bytes[0] as char).to_ascii_uppercase().to_string());
    }
    #[cfg(target_os = "linux")]
    if text.starts_with('/') {
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let mounts = crate::platform_linux::parse_mountinfo_str(&mountinfo);
        let best = mounts
            .into_iter()
            .filter(|m| {
                let mp = m.mount_point.as_str();
                mp == "/"
                    || text == mp
                    || text
                        .strip_prefix(mp)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|m| m.mount_point.len());
        return Some(best.map(|m| m.mount_point).unwrap_or_else(|| "/".into()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration as StdDuration;

    fn sandbox(path: &Path) -> bool {
        tiering::is_forbidden_in_test_sandbox(path, None)
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn age(path: &Path, days: u64) {
        let file = fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - StdDuration::from_secs(days * 86_400))
            .unwrap();
    }

    fn config<'a>(
        roots: Vec<PathBuf>,
        profiles: Vec<PathBuf>,
        vhdx: Vec<(PathBuf, &'static str)>,
        forbidden: &'a dyn Fn(&Path) -> bool,
    ) -> ScanConfig<'a> {
        ScanConfig {
            roots,
            profile_dirs: profiles,
            vhdx,
            min_cache_bytes: 1,
            project_budget: StdDuration::from_secs(20),
            cache_budget: StdDuration::from_secs(20),
            forbidden,
        }
    }

    #[test]
    fn clean_token_is_deterministic_and_case_insensitive() {
        let a = clean_token("disk1", "C:\\Dev\\App\\node_modules");
        let b = clean_token("disk1", "c:/dev/app/NODE_MODULES/");
        assert_eq!(a, b);
        assert!(a.starts_with("dev_clean:disk1:"));
        assert_ne!(a, clean_token("disk2", "C:\\Dev\\App\\node_modules"));
    }

    #[test]
    fn discovers_projects_pairs_artifacts_and_protects_env_and_git() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Projects");
        let web = root.join("web");
        write(&web.join("package.json"), b"{}");
        write(&web.join(".env"), b"SECRET=1");
        write(&web.join("src").join("index.ts"), b"export {}");
        write(
            &web.join("node_modules").join("a").join("b.js"),
            &[0u8; 4096],
        );
        write(&web.join(".next").join("cache.bin"), &[0u8; 2048]);
        write(&web.join(".git").join("index"), b"x");
        let rs = root.join("engine");
        write(&rs.join("Cargo.toml"), b"[package]");
        write(&rs.join("target").join("debug").join("x.bin"), &[0u8; 8192]);
        age(&rs.join("Cargo.toml"), 40);
        // Looks like an artifact but has no manifest -> must not be offered.
        write(&root.join("loose").join("node_modules").join("z.js"), b"z");
        // Artifact folder that hides a git repo -> protected.
        write(&web.join("dist").join(".git").join("HEAD"), b"ref");
        write(&web.join("dist").join("bundle.js"), b"js");

        let cfg = config(vec![root.clone()], vec![], vec![], &sandbox);
        let out = analyze_dev_workspace_root(&cfg, "disk1", "C");
        assert_eq!(out.projects.len(), 2, "{:?}", out.projects);

        let web_p = out.projects.iter().find(|p| p.name == "web").unwrap();
        assert_eq!(web_p.stack, "Node / JS");
        assert!(web_p.has_git);
        assert!(web_p.has_env);
        let names: Vec<_> = web_p.artifacts.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"node_modules"));
        assert!(names.contains(&".next"));
        assert!(!names.contains(&"dist"), "dist hides .git: {names:?}");
        assert_eq!(web_p.artifact_bytes, 4096 + 2048);
        assert!(web_p
            .artifacts
            .iter()
            .all(|a| a.clean_token.starts_with("dev_clean:disk1:")));

        let rs_p = out.projects.iter().find(|p| p.name == "engine").unwrap();
        assert_eq!(rs_p.stack, "Rust");
        assert!(rs_p.stale, "40 day old project must be stale");
        assert!(rs_p.days_untouched >= 39);
        assert_eq!(rs_p.artifacts.len(), 1);
        assert_eq!(out.stale_artifact_bytes, 8192);
        assert_eq!(out.total_artifact_bytes, 4096 + 2048 + 8192);
    }

    #[test]
    fn forbidden_roots_are_never_scanned() {
        let tmp = tempfile::tempdir().unwrap();
        let win = tmp.path().join("Windows");
        write(&win.join("package.json"), b"{}");
        write(&win.join("node_modules").join("a.js"), b"a");
        let cfg = config(vec![win], vec![], vec![], &sandbox);
        let out = analyze_dev_workspace_root(&cfg, "d", "C");
        assert!(out.projects.is_empty());
    }

    #[test]
    fn global_cache_classification_respects_size_floor_and_clean_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("Users").join("dev");
        write(
            &profile.join(".ollama").join("models").join("blob.bin"),
            &[1u8; 20_000],
        );
        write(
            &profile.join(".npm").join("_cacache").join("x"),
            &[1u8; 5_000],
        );
        write(
            &profile.join(".cache").join("pip").join("w.whl"),
            &[1u8; 100],
        );
        let mut cfg = config(vec![], vec![profile.clone()], vec![], &sandbox);
        cfg.min_cache_bytes = 1_000;
        let out = analyze_dev_workspace_root(&cfg, "disk1", "C");
        assert_eq!(out.global_caches.len(), 2, "{:?}", out.global_caches);
        let ollama = out
            .global_caches
            .iter()
            .find(|c| c.kind == "ai_model")
            .unwrap();
        assert!(ollama.can_offload && !ollama.can_clean && ollama.clean_token.is_none());
        let npm = out
            .global_caches
            .iter()
            .find(|c| c.kind == "pkg_cache")
            .unwrap();
        assert!(npm.can_clean && npm.clean_token.is_some());
        assert_eq!(out.total_cache_bytes, 25_000);
        assert_eq!(out.total_reclaimable_bytes, 5_000);
    }

    #[test]
    fn safe_clean_removes_only_the_allowlisted_artifact() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        write(&app.join("package.json"), b"{}");
        write(&app.join(".env"), b"KEY=1");
        write(&app.join("src").join("main.ts"), b"code");
        write(
            &app.join("node_modules").join("p").join("i.js"),
            &[7u8; 1024],
        );
        write(&app.join("node_modules").join("q.js"), &[7u8; 512]);
        let cancel = AtomicBool::new(false);
        let res =
            clean_dev_artifact_path(&app.join("node_modules"), &cancel, &mut |_, _| {}, &sandbox)
                .unwrap();
        assert_eq!(res["freed_bytes"], 1536);
        assert_eq!(res["root_removed"], true);
        assert!(!app.join("node_modules").exists());
        assert_eq!(fs::read(app.join(".env")).unwrap(), b"KEY=1");
        assert_eq!(fs::read(app.join("src").join("main.ts")).unwrap(), b"code");
        assert!(app.join("package.json").is_file());
    }

    #[test]
    fn safe_clean_rejects_source_unpaired_and_protected_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        write(&app.join("package.json"), b"{}");
        write(&app.join("src").join("main.ts"), b"code");
        write(&app.join("dist").join(".env"), b"S=1");
        write(&app.join("build").join(".git").join("HEAD"), b"ref");
        let loose = tmp.path().join("loose");
        write(&loose.join("node_modules").join("a.js"), b"a");
        let cancel = AtomicBool::new(false);
        for bad in [
            app.join("src"),
            app.clone(),
            app.join("dist"),
            app.join("build"),
            loose.join("node_modules"),
            tmp.path().join("missing"),
        ] {
            assert!(
                clean_dev_artifact_path(&bad, &cancel, &mut |_, _| {}, &sandbox).is_err(),
                "{} must be rejected",
                bad.display()
            );
        }
        assert_eq!(fs::read(app.join("src").join("main.ts")).unwrap(), b"code");
        assert_eq!(fs::read(app.join("dist").join(".env")).unwrap(), b"S=1");
        assert!(loose.join("node_modules").join("a.js").is_file());
        // Production firewall blocks protected system components outright.
        let win = Path::new("C:\\Windows\\node_modules");
        assert!(validate_cleanable_dev_path(win, &tiering::is_forbidden_path).is_err());
    }

    #[test]
    fn package_cache_leaf_requires_user_profile_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = tmp.path().join("Users").join("dev").join(".npm");
        write(&ok.join("_cacache").join("x"), b"x");
        assert_eq!(
            validate_cleanable_dev_path(&ok, &sandbox).unwrap(),
            CleanKind::PackageCache
        );
        let wrong = tmp.path().join("Other").join("dev").join(".npm");
        write(&wrong.join("x"), b"x");
        assert!(validate_cleanable_dev_path(&wrong, &sandbox).is_err());
        let ai = tmp
            .path()
            .join("Users")
            .join("dev")
            .join(".ollama")
            .join("models");
        write(&ai.join("m.bin"), b"m");
        assert!(
            validate_cleanable_dev_path(&ai, &sandbox).is_err(),
            "AI models must never be deletable"
        );
    }

    #[cfg(windows)]
    #[test]
    fn clean_unlinks_nested_junction_without_touching_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        write(&app.join("package.json"), b"{}");
        let outside = tmp.path().join("outside");
        write(&outside.join("keep.txt"), b"precious");
        write(&app.join("node_modules").join("a.js"), b"a");
        let link = app.join("node_modules").join("linked");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success(), "mklink failed");
        let cancel = AtomicBool::new(false);
        clean_dev_artifact_path(&app.join("node_modules"), &cancel, &mut |_, _| {}, &sandbox)
            .unwrap();
        assert!(!app.join("node_modules").exists());
        assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"precious");
    }

    #[test]
    fn clean_honours_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        write(&app.join("package.json"), b"{}");
        write(&app.join("node_modules").join("a.js"), b"a");
        let cancel = AtomicBool::new(true);
        let err =
            clean_dev_artifact_path(&app.join("node_modules"), &cancel, &mut |_, _| {}, &sandbox)
                .unwrap_err();
        assert_eq!(err, process::CANCELLED);
        assert!(app.join("node_modules").join("a.js").is_file());
    }

    #[test]
    fn vhdx_validation_rejects_metacharacters_traversal_and_foreign_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("wsl");
        let good = root.join("distro").join("ext4.vhdx");
        write(&good, b"vhdx");
        let roots = vec![root.clone()];
        let ok = validate_wsl_vhdx_path_with_roots(&good.to_string_lossy(), &roots).unwrap();
        assert!(ok.to_string_lossy().ends_with("ext4.vhdx"));

        for bad in [
            "".to_owned(),
            format!("{}\\..\\ext4.vhdx", root.display()),
            format!("{}\"; format c:", good.display()),
            format!("{}`whoami`", good.display()),
            format!("{} & calc", good.display()),
            format!("{}\r\nselect", good.display()),
        ] {
            assert!(
                validate_wsl_vhdx_path_with_roots(&bad, &roots).is_err(),
                "{bad:?} must be rejected"
            );
        }
        let other = tmp.path().join("elsewhere").join("ext4.vhdx");
        write(&other, b"vhdx");
        assert!(validate_wsl_vhdx_path_with_roots(&other.to_string_lossy(), &roots).is_err());
        let wrong_ext = root.join("distro").join("notes.txt");
        write(&wrong_ext, b"t");
        assert!(validate_wsl_vhdx_path_with_roots(&wrong_ext.to_string_lossy(), &roots).is_err());
        let renamed = root.join("distro").join("evil.vhdx");
        write(&renamed, b"v");
        assert!(validate_wsl_vhdx_path_with_roots(&renamed.to_string_lossy(), &roots).is_err());
        assert!(validate_wsl_vhdx_path_with_roots(&root.to_string_lossy(), &roots).is_err());
    }

    #[test]
    fn vhdx_scan_filters_by_volume_letter() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("ext4.vhdx");
        write(&f, &[0u8; 64]);
        let letter = drive_letter(&f.to_string_lossy()).unwrap_or_default();
        let items = scan_wsl_vhdx_files(&[(f.clone(), "wsl_vhdx")], &letter);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].size_bytes, 64);
        assert!(scan_wsl_vhdx_files(&[(f, "wsl_vhdx")], "Q").is_empty());
    }

    #[test]
    fn custom_root_validation_enforces_volume_and_firewall() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("mine");
        fs::create_dir_all(&dir).unwrap();
        let letter = drive_letter(&dir.to_string_lossy()).unwrap_or_default();
        assert!(validate_custom_root(&dir.to_string_lossy(), &letter, &sandbox).is_ok());
        assert!(validate_custom_root(&dir.to_string_lossy(), "Q", &sandbox).is_err());
        assert!(validate_custom_root("", &letter, &sandbox).is_err());
        assert!(validate_custom_root(&format!("{letter}:\\..\\x"), &letter, &sandbox).is_err());
        assert!(validate_custom_root("C:\\Windows", "C", &tiering::is_forbidden_path).is_err());
        assert!(validate_custom_root("C:\\", "C", &tiering::is_forbidden_path).is_err());
    }

    #[test]
    fn drive_letter_parses_plain_and_verbatim_paths() {
        assert_eq!(drive_letter("e:\\a").as_deref(), Some("E"));
        assert_eq!(drive_letter("\\\\?\\D:\\x").as_deref(), Some("D"));
        assert_eq!(drive_letter("relative\\x"), None);
        assert_eq!(drive_letter("C:"), None);
    }
}
