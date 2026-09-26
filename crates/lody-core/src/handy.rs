//! Handy, the app that listens: find it, check how it is set up, open it.
//!
//! Lody only reads Handy's setup (`settings_store.json` in its data folder, models in the
//! shared Hugging Face cache); every change is made in Handy's own window, so the two never
//! overwrite each other.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// The Handy release Lody installs and whose settings and models it knows.
pub const RELEASE: &str = "0.9.7";
pub const RELEASES_URL: &str = "https://github.com/cjpais/Handy/releases";

/// A speech model from Handy's catalog, with the file Handy downloads by default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Hugging Face repo, e.g. `handy-computer/whisper-medium-gguf`.
    pub repo: String,
    pub name: String,
    pub description: String,
    pub languages: Vec<String>,
    /// Can turn other languages into English while transcribing.
    pub translate: bool,
    pub speed: u32,
    pub accuracy: u32,
    pub file: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct Catalog {
    models: Vec<Model>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../handy/models.json")).expect("bundled Handy catalog")
});

/// Every model in Handy's catalog.
pub fn models() -> &'static [Model] {
    &CATALOG.models
}

/// The models that understand `language`, the ones that can translate first, then by accuracy.
pub fn models_for(language: &str) -> Vec<&'static Model> {
    let mut found: Vec<_> =
        models().iter().filter(|m| m.languages.iter().any(|l| l == language)).collect();
    found.sort_by_key(|m| (!m.translate, std::cmp::Reverse(m.accuracy)));
    found
}

/// The catalog model a Handy model id (`{repo}/{file}`, any file of the repo) belongs to.
pub fn model_by_id(id: &str) -> Option<&'static Model> {
    models().iter().find(|m| id.starts_with(&format!("{}/", m.repo)))
}

/// The parts of Handy's settings that decide what reaches the AI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    /// Model id, `{repo}/{file}` (or an older Handy id like `medium`).
    pub model: String,
    /// Language Handy listens for: a code like `th`, or `auto`.
    pub language: String,
    pub translate_to_english: bool,
    /// How Handy puts the text in: `direct` (types), `ctrl_v`, `ctrl_shift_v`, `none`, ...
    pub paste_method: String,
    pub custom_words: Vec<String>,
    /// How many transcriptions Handy keeps in its history.
    pub history_limit: u64,
}

/// `settings_store.json` in Handy's data folder (`~/.local/share/com.pais.handy` on Linux).
pub fn settings_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.pais.handy/settings_store.json")
}

/// Handy's settings, or `None` when Handy has never been started.
pub fn read_config(path: &Path) -> Result<Option<Config>, Error> {
    if !path.exists() {
        return Ok(None);
    }
    let store: Value = serde_json::from_str(&std::fs::read_to_string(path)?)
        .map_err(|e| Error::Config(format!("{}: {e}", path.display())))?;
    let s = &store["settings"];
    let text = |key: &str, default: &str| s[key].as_str().unwrap_or(default).to_string();
    Ok(Some(Config {
        model: text("selected_model", ""),
        language: text("selected_language", "auto"),
        translate_to_english: s["translate_to_english"].as_bool().unwrap_or(false),
        paste_method: text("paste_method", "ctrl_v"),
        custom_words: s["custom_words"]
            .as_array()
            .map(|a| a.iter().filter_map(|w| Some(w.as_str()?.to_string())).collect())
            .unwrap_or_default(),
        history_limit: s["history_limit"].as_u64().unwrap_or(5),
    }))
}

/// What in Handy's setup keeps it from working for `language`, each with what to do in Handy.
pub fn problems(config: &Config, language: &str, language_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(model) = model_by_id(&config.model) else {
        if config.model.is_empty() {
            out.push("No speech model is chosen: in Handy, pick one under Models.".into());
        }
        return out;
    };
    if downloaded_file(model).is_none() {
        out.push(format!("{} is chosen but not downloaded: download it in Handy.", model.name));
    }
    if !model.languages.iter().any(|l| l == language) {
        out.push(format!(
            "{} does not understand {language_name}: in Handy, pick one of the models below.",
            model.name
        ));
    }
    if config.language != "auto" && config.language != language {
        out.push(format!(
            "Handy listens for \"{}\": in Handy, set Language to {language_name}.",
            config.language
        ));
    }
    if config.translate_to_english && !model.translate {
        out.push(format!(
            "{} cannot translate, so Handy types {language_name}. For English, pick a Whisper \
             model (not Turbo) in Handy.",
            model.name
        ));
    }
    if config.paste_method == "none" {
        out.push("Handy only copies the text: paste it yourself with Ctrl+Shift+V.".into());
    }
    out
}

// --- The Handy program ---

/// Where the `handy` program is, if it is installed.
pub fn find_program() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["handy.exe", "Handy.exe"] } else { &["handy"] };
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .chain(extra_places())
        .find(|p| p.is_file())
}

fn extra_places() -> Vec<PathBuf> {
    let mut places = vec![PathBuf::from("/usr/bin/handy")];
    if let Some(home) = dirs::home_dir() {
        places.push(home.join(".local/bin/handy"));
    }
    if cfg!(target_os = "macos") {
        places.push(PathBuf::from("/Applications/Handy.app/Contents/MacOS/Handy"));
    }
    if let Some(local) = dirs::data_local_dir().filter(|_| cfg!(windows)) {
        places.push(local.join("Handy/handy.exe"));
    }
    places
}

/// Whether Handy is running (Linux and Windows; always false elsewhere).
pub fn running() -> bool {
    if cfg!(windows) {
        return crate::background("tasklist")
            .args(["/FI", "IMAGENAME eq handy.exe", "/NH"])
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|out| {
                String::from_utf8_lossy(&out.stdout).to_lowercase().contains("handy.exe")
            });
    }
    let Ok(entries) = std::fs::read_dir("/proc") else { return false };
    entries.flatten().any(|e| {
        std::fs::read_to_string(e.path().join("comm")).is_ok_and(|comm| comm.trim() == "handy")
    })
}

/// Whether Handy is recording right now: its microphone stream shows up in the sound server
/// only while it records (unless Handy keeps the microphone always on).
pub fn recording() -> bool {
    Command::new("pactl")
        .args(["list", "source-outputs"])
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|out| is_handy_capture(&String::from_utf8_lossy(&out.stdout)))
}

fn is_handy_capture(pactl: &str) -> bool {
    pactl.contains("\"alsa_capture.handy\"") || pactl.contains("[handy]\"")
}

/// Show Handy's window: starts Handy, or, when it already runs, brings its window forward
/// (Handy answers a second launch that way).
pub fn open() -> Result<(), Error> {
    let program = find_program().ok_or_else(|| Error::Config("Handy is not installed".into()))?;
    let mut command = Command::new(program);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn()?;
    // Reap the short-lived second launch so it does not linger as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// How to install Handy's pinned release on this computer: a command that asks for the
/// password in a window (`pkexec`), Handy's own installer on Windows (for this user, so no
/// password), or `None` when only the download page can help.
pub fn install_command() -> Option<Vec<String>> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => ("x86_64", "amd64"),
        "aarch64" => ("aarch64", "arm64"),
        _ => return None,
    };
    let base = format!("{RELEASES_URL}/download/v{RELEASE}");
    if cfg!(windows) {
        let setup = format!("{base}/Handy_{RELEASE}_{}-setup.exe", arch.1.replace("amd64", "x64"));
        let script = format!(
            "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; \
             $f = Join-Path $env:TEMP 'handy-setup.exe'; \
             Invoke-WebRequest -UseBasicParsing -Uri '{setup}' -OutFile $f; \
             $p = Start-Process -FilePath $f -ArgumentList '/S' -Wait -PassThru; \
             Remove-Item $f; exit $p.ExitCode"
        );
        return Some(["powershell", "-NoProfile", "-Command", &script].map(String::from).to_vec());
    }
    let has = |p: &str| Path::new(p).exists();
    if !cfg!(target_os = "linux") || !has("/usr/bin/pkexec") {
        return None;
    }
    if has("/usr/bin/dnf") {
        let rpm = format!("{base}/Handy-{RELEASE}-1.{}.rpm", arch.0);
        return Some(["pkexec", "dnf", "install", "-y", &rpm].map(String::from).to_vec());
    }
    if has("/usr/bin/apt-get") {
        // apt needs a local file: fetch it first, then install it.
        let deb = format!("{base}/Handy_{RELEASE}_{}.deb", arch.1);
        let script = format!(
            "set -e; f=$(mktemp --suffix=.deb); curl -fsSL -o \"$f\" '{deb}'; \
             pkexec apt-get install -y \"$f\"; rm -f \"$f\""
        );
        return Some(["sh", "-c", &script].map(String::from).to_vec());
    }
    None
}

// --- Models in the Hugging Face cache ---

/// Where Handy (through hf-hub) keeps models: `HF_HUB_CACHE`, `HF_HOME/hub`, or
/// `~/.cache/huggingface/hub`.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("HF_HUB_CACHE") {
        return PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HF_HOME") {
        return PathBuf::from(home).join("hub");
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".cache/huggingface/hub")
}

/// The file of `model` Handy has downloaded, if any: the default one, else another size of the
/// same model.
pub fn downloaded_file(model: &Model) -> Option<String> {
    downloaded_file_in(&cache_dir(), model)
}

fn downloaded_file_in(cache: &Path, model: &Model) -> Option<String> {
    let repo = cache.join(format!("models--{}", model.repo.replace('/', "--")));
    let snapshots = std::fs::read_dir(repo.join("snapshots")).ok()?;
    let mut files: Vec<String> = snapshots
        .flatten()
        .filter_map(|snap| std::fs::read_dir(snap.path()).ok())
        .flatten()
        .flatten()
        // `exists` follows the link to the blob: a dangling link is not a model.
        .filter(|f| f.path().exists())
        .filter_map(|f| f.file_name().into_string().ok())
        .filter(|name| name.ends_with(".gguf"))
        .collect();
    files.sort();
    files.iter().find(|f| **f == model.file).or(files.first()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lody-handy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn thai_models_put_the_ones_that_translate_first() {
        let thai = models_for("th");
        assert!(thai.iter().any(|m| m.name == "Whisper Medium" && m.translate));
        assert!(thai.iter().any(|m| m.name == "Qwen3-ASR 0.6B" && !m.translate));
        let first_without = thai.iter().position(|m| !m.translate).unwrap();
        assert!(thai[..first_without].iter().all(|m| m.translate));
        assert!(!thai.iter().any(|m| m.name.starts_with("Parakeet")));
    }

    #[test]
    fn ids_match_any_file_of_the_same_repo() {
        let m = model_by_id("handy-computer/Qwen3-ASR-0.6B-gguf/Qwen3-ASR-0.6B-Q4_K_M.gguf");
        assert_eq!(m.unwrap().name, "Qwen3-ASR 0.6B");
        let id = "handy-computer/whisper-medium-gguf/whisper-medium-Q8_0.gguf";
        assert_eq!(model_by_id(id).unwrap().name, "Whisper Medium");
        assert!(model_by_id("medium").is_none());
    }

    #[test]
    fn reads_handys_settings() {
        let dir = temp("config");
        let path = dir.join("settings_store.json");
        std::fs::write(
            &path,
            r#"{"settings":{"selected_model":"x","selected_language":"th","paste_method":"direct"}}"#,
        )
        .unwrap();
        let config = read_config(&path).unwrap().unwrap();
        assert_eq!((config.model.as_str(), config.language.as_str()), ("x", "th"));
        assert_eq!(config.paste_method, "direct");
        assert!(!config.translate_to_english);
        assert!(read_config(&dir.join("missing.json")).unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn problems_say_what_to_change_in_handy() {
        let config = Config {
            model: "handy-computer/whisper-large-v3-turbo-gguf/x.gguf".into(),
            language: "en".into(),
            translate_to_english: true,
            paste_method: "none".into(),
            custom_words: vec![],
            history_limit: 5,
        };
        let found = problems(&config, "th", "Thai").join("\n");
        assert!(found.contains("cannot translate"), "{found}");
        assert!(found.contains("set Language to Thai"), "{found}");
        assert!(found.contains("Ctrl+Shift+V"), "{found}");
    }

    #[test]
    fn sees_handys_microphone_stream() {
        let recording = "Source Output #21438\n\tCorked: no\n\tProperties:\n\t\t\
            application.name = \"PipeWire ALSA [handy]\"\n\t\tnode.name = \"alsa_capture.handy\"\n";
        assert!(is_handy_capture(recording));
        let other = "Source Output #5\n\t\tapplication.name = \"Firefox\"\n";
        assert!(!is_handy_capture(other) && !is_handy_capture(""));
    }

    #[test]
    fn finds_a_model_where_handy_downloads_it() {
        let cache = temp("cache");
        let model = models_for("th")[0].clone();
        assert!(downloaded_file_in(&cache, &model).is_none());
        // hf-hub's layout: snapshots/<revision>/<file> linking to blobs/<sha256>.
        let repo = cache.join(format!("models--{}", model.repo.replace('/', "--")));
        std::fs::create_dir_all(repo.join("blobs")).unwrap();
        std::fs::write(repo.join("blobs").join("sha256"), b"gguf").unwrap();
        let snapshot = repo.join("snapshots").join("revision");
        std::fs::create_dir_all(&snapshot).unwrap();
        #[cfg(unix)]
        {
            let blob = Path::new("../../blobs/sha256");
            std::os::unix::fs::symlink(blob, snapshot.join(&model.file)).unwrap();
            assert_eq!(downloaded_file_in(&cache, &model), Some(model.file.clone()));
        }
        std::fs::remove_dir_all(cache).unwrap();
    }
}
