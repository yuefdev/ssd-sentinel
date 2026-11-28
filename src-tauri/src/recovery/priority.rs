use serde::Serialize;

use super::is_env_filename;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum FileKind {
    System,
    Infrastructure,
    Installer,
    Other,
    Archive,
    Media,
    Document,
    Photo,
    Project,
    Screenshot,
}

pub(super) fn is_editable_project_extension(ext: &str) -> bool {
    matches!(
        ext,
        "prproj"
            | "aep"
            | "aepx"
            | "drp"
            | "drt"
            | "veg"
            | "blend"
            | "blend1"
            | "c4d"
            | "ma"
            | "mb"
            | "max"
            | "psd"
            | "psb"
            | "kra"
            | "xcf"
            | "ora"
            | "ai"
            | "afphoto"
            | "afdesign"
            | "afpub"
            | "fig"
            | "sketch"
            | "xd"
            | "indd"
            | "als"
            | "flp"
            | "rpp"
            | "uproject"
            | "uplugin"
            | "umap"
            | "uasset"
            | "unity"
            | "prefab"
            | "asmdef"
            | "tscn"
            | "tres"
            | "sln"
            | "slnx"
            | "csproj"
            | "fsproj"
            | "vcxproj"
            | "dwg"
            | "dxf"
    )
}

pub(super) fn is_source_extension(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "vue"
            | "svelte"
            | "astro"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "swift"
            | "rb"
            | "php"
            | "c"
            | "h"
            | "hpp"
            | "cs"
            | "cpp"
            | "cc"
            | "fs"
            | "dart"
            | "lua"
            | "gd"
            | "gdshader"
            | "shader"
            | "html"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "prisma"
            | "graphql"
            | "gql"
            | "proto"
            | "tf"
            | "hcl"
            | "sh"
            | "sql"
            | "env"
    )
}

fn is_photo_extension(ext: &str) -> bool {
    matches!(
        ext,
        "png" | "jpg" | "jpeg" | "webp" | "heic" | "heif" | "avif" | "bmp" | "gif" | "tif" | "tiff"
    )
}

fn screenshot_words(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '\u{0300}'..='\u{036f}'))
        .map(|c| match c {
            'ı' => 'i',
            'ğ' => 'g',
            'ş' => 's',
            'ç' => 'c',
            'ö' => 'o',
            'ü' => 'u',
            _ => c,
        })
        .collect()
}

fn is_screenshot_folder(segment: &str) -> bool {
    let words = screenshot_words(segment);
    let compact: String = words.chars().filter(|c| c.is_alphabetic()).collect();
    is_screenshot_word(&compact)
}

fn is_screenshot_word(word: &str) -> bool {
    matches!(
        word,
        "screenshot"
            | "screenshots"
            | "screencapture"
            | "screencaptures"
            | "ekrangoruntusu"
            | "ekrangoruntuleri"
            | "ekranalintisi"
            | "ekranalintilari"
    )
}

fn has_screenshot_name(name: &str) -> bool {
    let words = screenshot_words(name);
    let mut previous = "";
    for word in words
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
    {
        if is_screenshot_word(word)
            || (previous == "screen" && matches!(word, "shot" | "shots" | "capture" | "captures"))
            || (previous == "ekran" && matches!(word, "goruntusu" | "goruntuleri" | "alintisi"))
        {
            return true;
        }
        previous = word;
    }
    false
}

pub(super) fn is_project_lockfile(name: &str) -> bool {
    matches!(
        name,
        "cargo.lock"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "poetry.lock"
            | "uv.lock"
            | "pipfile.lock"
            | "gemfile.lock"
            | "composer.lock"
            | "package-lock.json"
            | "pnpm-lock.yaml"
    )
}

fn is_infrastructure(name: &str, folder: &str, ext: &str) -> bool {
    if name.ends_with(".min.js") || name.ends_with(".min.css") || name.ends_with(".d.ts.map") {
        return true;
    }
    if is_project_lockfile(name)
        || matches!(
            ext,
            "pyc" | "pyo" | "class" | "pdb" | "cache" | "tmp" | "lock"
        )
        || matches!(
            name,
            "package.json"
                | "go.sum"
                | "cargo.toml"
                | "pyproject.toml"
                | "tsconfig.json"
                | "cmakelists.txt"
                | "cmakecache.txt"
                | "makefile"
                | "dockerfile"
                | "docker-compose.yml"
                | "docker-compose.yaml"
                | "build.gradle"
                | "settings.gradle"
                | "build.gradle.kts"
                | "settings.gradle.kts"
                | "requirements.txt"
                | ".gitignore"
                | ".dockerignore"
                | ".npmrc"
                | "desktop.ini"
                | "thumbs.db"
        )
    {
        return true;
    }

    let mut previous = "";
    for segment in folder.split(['\\', '/']).filter(|part| !part.is_empty()) {
        if matches!(
            segment,
            "node_modules"
                | "__pycache__"
                | ".venv"
                | "site-packages"
                | ".next"
                | ".nuxt"
                | ".svelte-kit"
                | ".gradle"
                | ".pytest_cache"
                | ".mypy_cache"
                | ".ruff_cache"
                | ".parcel-cache"
                | ".git"
                | ".vs"
                | ".cxx"
                | ".dart_tool"
                | ".godot"
                | "cmakefiles"
                | "packagecache"
                | "deriveddatacache"
        ) || (matches!(previous, "bin" | "obj" | "target")
            && matches!(segment, "debug" | "release"))
            || (previous == "build" && matches!(segment, "intermediates" | "generated" | "outputs"))
        {
            return true;
        }
        if (segment == "dist" && matches!(ext, "js" | "mjs" | "cjs" | "css"))
            || (matches!(segment, "dist" | "build")
                && matches!(ext, "map" | "o" | "class" | "dll" | "exe"))
        {
            return true;
        }
        previous = segment;
    }
    false
}

pub(super) fn file_kind(
    name: &str,
    path: &str,
    ext: &str,
    category: &str,
    has_original_location: bool,
) -> FileKind {
    let (folder, path_name) = path.rsplit_once(['\\', '/']).unwrap_or(("", path));
    let name = if name.is_empty() { path_name } else { name };
    let ext = ext.trim_start_matches('.');
    let ext = if ext.is_empty() {
        name.rsplit_once('.').map_or("", |(_, suffix)| suffix)
    } else {
        ext
    }
    .to_ascii_lowercase();

    let folder = if has_original_location { folder } else { "" };
    if is_infrastructure(
        &name.to_ascii_lowercase(),
        &folder.to_ascii_lowercase(),
        &ext,
    ) {
        return FileKind::Infrastructure;
    }
    if is_editable_project_extension(&ext) || is_source_extension(&ext) || is_env_filename(name) {
        return FileKind::Project;
    }
    if is_photo_extension(&ext) {
        let named_screenshot = has_original_location
            && (has_screenshot_name(name) || folder.split(['\\', '/']).any(is_screenshot_folder));
        return if named_screenshot {
            FileKind::Screenshot
        } else {
            FileKind::Photo
        };
    }

    match category {
        "photo" => FileKind::Photo,
        "document" => FileKind::Document,
        "video_audio" => FileKind::Media,
        "archive" => FileKind::Archive,
        "exe_setup" | "game_portable" => FileKind::Installer,
        "system_lib" => FileKind::System,
        _ => FileKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_screenshot_names_and_folders_but_ignores_carved_names() {
        let cases = [
            (
                "EKRAN GÖRÜNTÜSÜ (8).PNG",
                r"D:\Belgeler\EKRAN GÖRÜNTÜSÜ (8).PNG",
                true,
                FileKind::Screenshot,
            ),
            (
                "Screen Shot 2026.png",
                r"D:\Desktop\Screen Shot 2026.png",
                true,
                FileKind::Screenshot,
            ),
            (
                "IMG_2442.jpg",
                r"D:\Resimler\Ekran Görüntüleri\IMG_2442.jpg",
                true,
                FileKind::Screenshot,
            ),
            (
                "730_5.jpg",
                r"D:\Steam\userdata\123\760\remote\730\screenshots\730_5.jpg",
                true,
                FileKind::Screenshot,
            ),
            (
                "IMG_2442.jpg",
                r"D:\Resimler\Library\IMG_2442.jpg",
                true,
                FileKind::Photo,
            ),
            (
                "1920x1080.png",
                r"D:\Resimler\1920x1080.png",
                true,
                FileKind::Photo,
            ),
            (
                "Screenshot (8).png",
                r"D:\Screenshots\Screenshot (8).png",
                false,
                FileKind::Photo,
            ),
            (
                "Screenshot (8).png",
                r"D:\App\node_modules\Screenshots\Screenshot (8).png",
                true,
                FileKind::Infrastructure,
            ),
        ];
        for (name, path, original, expected) in cases {
            assert_eq!(
                file_kind(name, path, "", "photo", original),
                expected,
                "{path}"
            );
        }
    }

    #[test]
    fn separates_editable_projects_from_support_artifacts() {
        let cases = [
            (r"D:\Video\belgesel.PRPROJ", "other", FileKind::Project),
            (r"D:\Art\sahne.blend", "other", FileKind::Project),
            (r"D:\Art\cover.psd", "photo", FileKind::Project),
            (r"D:\Game\Content\Level.umap", "other", FileKind::Project),
            (r"D:\App\src\App.tsx", "database_code", FileKind::Project),
            (r"D:\App\.env.local", "database_code", FileKind::Project),
            (
                r"D:\App\node_modules\vendor\index.js",
                "database_code",
                FileKind::Infrastructure,
            ),
            (
                r"D:\App\target\release\app.exe",
                "exe_setup",
                FileKind::Infrastructure,
            ),
            (
                r"D:\App\dist\assets\index-AAA123.js",
                "database_code",
                FileKind::Infrastructure,
            ),
            (
                r"D:\App\package.json",
                "database_code",
                FileKind::Infrastructure,
            ),
            (
                r"D:\App\Cargo.lock",
                "database_code",
                FileKind::Infrastructure,
            ),
            (
                r"D:\App\Documents\Build\report.pdf",
                "document",
                FileKind::Document,
            ),
        ];
        for (path, category, expected) in cases {
            assert_eq!(file_kind("", path, "", category, true), expected, "{path}");
        }
    }
}
