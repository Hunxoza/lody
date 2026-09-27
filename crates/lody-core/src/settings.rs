//! Lody's settings: `config.toml` in the config folder, every key optional.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::filter::Scope;
use crate::locale::Locale;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Your language (a locale code).
    pub locale: String,
    /// Replies arrive in English and are translated into your language; false reads them as is
    /// (for an AI that already answers in your language).
    pub translate: bool,
    /// Seconds to wait for the translator.
    pub timeout: u64,
    /// Which translator: an id from `translators.toml` ("google", "claude:haiku", ...).
    pub translator: String,
    pub speech: Speech,
    /// Settings for the voices that need them (`speech.voice` picks the voice).
    pub voices: VoiceEngines,
    pub display: Display,
    pub sources: Sources,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Speech {
    pub enabled: bool,
    /// Which paragraphs are read: all, the first and last (default), or the last.
    pub scope: Scope,
    pub max_chars: usize,
    pub min_chars: usize,
    /// Voice, as `engine:name` (see `voice`); empty means the language's default.
    pub voice: String,
    /// Speed like "+10%"; empty means the language's default.
    pub rate: String,
    /// Voice used when translation fails and the English is read instead.
    pub fallback_voice: String,
    /// Say the project's name first: when several sessions are active, always, or never.
    pub announce_project: Announce,
    /// Pause while Handy records (it mutes the sound) and read the cut-off sentence again after.
    pub wait_for_handy: bool,
    /// Mute the other programs while speaking, and unmute them after (Windows).
    pub mute_others: bool,
    /// While the AI works: say nothing, or which tools it uses ("searching the web"), in your
    /// language without translating, so only the final reply is sent to the translator.
    pub progress: Progress,
    /// Seconds between progress lines of one session; tool steps in between are counted.
    pub progress_every: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceEngines {
    /// `command:`: a program that gets the text on its input and writes audio (WAV or MP3) to
    /// its output; `{rate}` becomes the speed in percent.
    pub command: String,
    /// `openai:<voice>`: a service with OpenAI's speech API.
    pub openai: OpenAiVoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAiVoice {
    /// Up to `/v1`: `https://api.openai.com/v1`, or a local server's.
    pub url: String,
    pub model: String,
    /// Sent only to `url`; empty for servers that need none.
    pub key: String,
}

impl Default for OpenAiVoice {
    fn default() -> Self {
        OpenAiVoice {
            url: "https://api.openai.com/v1".into(),
            model: "gpt-4o-mini-tts".into(),
            key: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Display {
    /// Translate the whole reply for reading in the app, not only the part read aloud.
    pub enabled: bool,
    pub max_chars: usize,
    /// Show each reply in your language in a small window that stays on top of the others.
    pub overlay: bool,
}

/// Which programs are read, by their id in `sources::PROGRAMS` (`claude_code = false`); one
/// not listed is read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sources(pub BTreeMap<String, bool>);

impl Sources {
    pub fn is_on(&self, id: &str) -> bool {
        self.0.get(id).copied().unwrap_or(true)
    }

    pub fn set(&mut self, id: &str, on: bool) {
        self.0.insert(id.to_string(), on);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Announce {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Progress {
    Off,
    /// "messages" was reading what it writes along the way, translated: too many requests.
    #[serde(alias = "messages")]
    All,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            locale: "th".into(),
            translate: true,
            timeout: 8,
            translator: "google".into(),
            speech: Speech::default(),
            voices: VoiceEngines::default(),
            display: Display::default(),
            sources: Sources::default(),
        }
    }
}

impl Speech {
    /// The voice to read in: the one picked, else the language's.
    pub fn voice_in(&self, locale: &Locale) -> String {
        if self.voice.is_empty() { locale.tts.voice.clone() } else { self.voice.clone() }
    }

    /// The speed to read at: the one picked, else the language's.
    pub fn rate_in(&self, locale: &Locale) -> String {
        if self.rate.is_empty() { locale.tts.rate.clone() } else { self.rate.clone() }
    }
}

impl Default for Speech {
    fn default() -> Self {
        Speech {
            enabled: true,
            scope: Scope::FirstLast,
            max_chars: 1500,
            min_chars: 2,
            voice: String::new(),
            rate: String::new(),
            fallback_voice: "en-US-AvaMultilingualNeural".into(),
            announce_project: Announce::Auto,
            wait_for_handy: true,
            mute_others: false,
            progress: Progress::All,
            progress_every: 4,
        }
    }
}

impl Default for Display {
    fn default() -> Self {
        Display { enabled: true, max_chars: 9000, overlay: false }
    }
}

/// Where Lody keeps its settings and extra locales (`~/.config/lody` on Linux).
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LODY_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("lody")
}

/// Where Lody keeps logs and its state (`~/.local/state/lody` on Linux).
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LODY_STATE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lody")
}

impl Settings {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// The settings in `path`, or the defaults when there is no file yet.
    pub fn load_from(path: &Path) -> Result<Settings, Error> {
        if !path.exists() {
            return Ok(Settings::default());
        }
        let text = std::fs::read_to_string(path)?;
        toml::from_str(&text).map_err(|e| Error::Config(format!("{}: {e}", path.display())))
    }

    pub fn load() -> Result<Settings, Error> {
        Settings::load_from(&Settings::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| Error::Config(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults_and_partial_files_fill_in() {
        let dir = std::env::temp_dir().join(format!("lody-settings-{}", std::process::id()));
        let path = dir.join("config.toml");
        assert_eq!(Settings::load_from(&path).unwrap(), Settings::default());

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "[speech]\nscope = \"all\"\n").unwrap();
        let settings = Settings::load_from(&path).unwrap();
        assert_eq!(settings.speech.scope, Scope::All);
        assert!(settings.speech.enabled && settings.sources.is_on("claude_code"));

        std::fs::write(&path, "[sources]\nclaude_code = false\n").unwrap();
        let settings = Settings::load_from(&path).unwrap();
        assert!(!settings.sources.is_on("claude_code") && settings.sources.is_on("other"));

        settings.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).unwrap(), settings);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
