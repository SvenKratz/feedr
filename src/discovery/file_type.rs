use crate::model::{Resource, ResourceKind};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    Text,
    Code,
    Markdown,
    Image,
    Video,
    Audio,
    Pdf,
    Web,
    Folder,
    App,
    Other,
}
impl FileType {
    pub const ALL: [Self; 11] = [
        Self::Text,
        Self::Code,
        Self::Markdown,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Pdf,
        Self::Web,
        Self::Folder,
        Self::App,
        Self::Other,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Code => "code",
            Self::Markdown => "markdown",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Pdf => "pdf",
            Self::Web => "web",
            Self::Folder => "folder",
            Self::App => "app",
            Self::Other => "other",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Code => "Code",
            Self::Markdown => "Markdown",
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::Pdf => "PDF",
            Self::Web => "Web links",
            Self::Folder => "Folders",
            Self::App => "Apps",
            Self::Other => "Other files",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Self::Text => "Plain text, logs, CSV, reStructuredText and Org files",
            Self::Code => {
                "Source code, scripts, HTML/CSS, JSON, YAML, TOML and other configuration files"
            }
            Self::Markdown => "Markdown documents, including .md, .markdown, .mdx and .rmd",
            Self::Image => "Images, including PNG, JPEG, GIF, SVG, TIFF and WebP",
            Self::Video => "Video files, including MP4, MOV, MKV, WebM and AVI",
            Self::Audio => "Audio files, including MP3, WAV, FLAC, M4A, Ogg and Opus",
            Self::Pdf => "PDF documents",
            Self::Web => "HTTP and HTTPS links, regardless of the URL's extension",
            Self::Folder => "Directories and mounted volumes",
            Self::App => "Application bundles and app references",
            Self::Other => "Files whose type is not in another category",
        }
    }
    pub fn of(resource: &Resource) -> Self {
        match resource.kind {
            ResourceKind::Url => return Self::Web,
            ResourceKind::Folder => return Self::Folder,
            ResourceKind::App => return Self::App,
            ResourceKind::File => (),
        }
        Self::path(&resource.target)
    }
    pub(super) fn path(target: &str) -> Self {
        let path = Path::new(target);
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "makefile"
                | "gnumakefile"
                | "dockerfile"
                | "containerfile"
                | "justfile"
                | "gemfile"
                | "rakefile"
                | "cmakelists.txt"
                | ".gitignore"
                | ".gitattributes"
                | ".dockerignore"
                | ".editorconfig"
                | ".env"
        ) || name.starts_with(".env.")
        {
            return Self::Code;
        }
        match path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase()
            .as_str()
        {
            "md" | "markdown" | "mdown" | "mkd" | "mdx" | "rmd" | "qmd" => Self::Markdown,
            "txt" | "text" | "log" | "csv" | "tsv" | "rst" | "adoc" | "org" => Self::Text,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "tif" | "tiff" | "avif"
            | "heic" | "ico" => Self::Image,
            "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "mpg" | "mpeg" | "ogv" => Self::Video,
            "mp3" | "wav" | "flac" | "m4a" | "aac" | "ogg" | "oga" | "opus" | "aif" | "aiff"
            | "alac" | "wma" => Self::Audio,
            "pdf" => Self::Pdf,
            "rs" | "py" | "pyi" | "ipynb" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "go"
            | "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "cs" | "java" | "kt" | "kts" | "swift"
            | "m" | "mm" | "rb" | "php" | "pl" | "pm" | "r" | "jl" | "lua" | "sh" | "bash"
            | "zsh" | "fish" | "ps1" | "bat" | "cmd" | "sql" | "html" | "htm" | "css" | "scss"
            | "sass" | "less" | "vue" | "svelte" | "ex" | "exs" | "erl" | "hs" | "clj" | "cljs"
            | "cljc" | "edn" | "dart" | "zig" | "nix" | "json" | "jsonl" | "yaml" | "yml"
            | "toml" | "xml" | "ini" | "cfg" | "conf" | "env" | "lock" | "proto" | "graphql"
            | "tf" | "hcl" | "cmake" | "wasm" => Self::Code,
            _ => Self::Other,
        }
    }
}
