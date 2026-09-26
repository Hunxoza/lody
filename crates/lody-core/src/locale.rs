//! Languages: one small TOML file each, bundled or in the config folder's `locales/`.

use std::path::Path;

use serde::Deserialize;

use crate::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct Locale {
    #[serde(skip)]
    pub code: String,
    pub name: String,
    pub native_name: String,
    pub stt: Stt,
    pub translate: TranslateCodes,
    pub tts: Tts,
    pub text: Text,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Stt {
    pub whisper_language: String,
    pub whisper_model: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TranslateCodes {
    pub google: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tts {
    pub voice: String,
    pub rate: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Text {
    pub sentence_split: Split,
    /// A regex character range of this language's letters, e.g. `฀-๿`.
    pub native_script: Option<String>,
    pub code_omitted: String,
    pub table_omitted: String,
    /// Said before a reply that could not be translated and is read in English instead.
    pub untranslated: Option<String>,
    /// What is said while the AI works; without it, tool steps are not read.
    pub progress: Option<ProgressText>,
}

/// `[text.progress]`: a phrase per kind of step, and how one or several are said.
#[derive(Debug, Clone, Deserialize)]
pub struct ProgressText {
    /// One step: `{step}` is the phrase below.
    pub one: String,
    /// Several of one kind: `{step}` and `{n}`.
    pub many: String,
    /// A command or task with the AI's description of it (a verb phrase), as `{detail}`.
    #[serde(default)]
    pub doing: Option<String>,
    /// A step with what it's about (a search's query, a file's name): `{step}` and `{detail}`.
    #[serde(default)]
    pub about: Option<String>,
    pub shell: String,
    pub read: String,
    pub edit: String,
    pub search: String,
    pub web_search: String,
    pub web_fetch: String,
    pub agent: String,
    pub other: String,
}

impl ProgressText {
    pub fn step(&self, kind: crate::progress::Kind) -> &str {
        use crate::progress::Kind;
        match kind {
            Kind::Shell => &self.shell,
            Kind::Read => &self.read,
            Kind::Edit => &self.edit,
            Kind::Search => &self.search,
            Kind::WebSearch => &self.web_search,
            Kind::WebFetch => &self.web_fetch,
            Kind::Agent => &self.agent,
            Kind::Other => &self.other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    Punct,
    Space,
    None,
}

const BUNDLED: &[(&str, &str)] = &[("th", include_str!("../locales/th.toml"))];

impl Locale {
    /// Reads a sentence the chosen voice fails on: Google's voice for this language.
    pub fn backup_voice(&self) -> String {
        format!("google:{}", self.translate.google)
    }

    /// The locale `code`: the user's file in `folder` wins over the bundled one.
    pub fn load(code: &str, folder: Option<&Path>) -> Result<Locale, Error> {
        let user = folder.map(|f| f.join(format!("{code}.toml")));
        let text = match user.filter(|p| p.exists()) {
            Some(path) => std::fs::read_to_string(&path)?,
            None => BUNDLED
                .iter()
                .find(|(c, _)| *c == code)
                .map(|(_, t)| t.to_string())
                .ok_or_else(|| Error::Config(format!("no locale {code:?}")))?,
        };
        Locale::parse(code, &text)
    }

    pub fn parse(code: &str, text: &str) -> Result<Locale, Error> {
        let mut locale: Locale =
            toml::from_str(text).map_err(|e| Error::Config(format!("locale {code}: {e}")))?;
        locale.code = code.to_string();
        if let Some(script) = &locale.text.native_script {
            fancy_regex::Regex::new(&format!("[{script}]"))
                .map_err(|e| Error::Config(format!("locale {code}: native_script: {e}")))?;
        }
        Ok(locale)
    }

    pub fn bundled() -> impl Iterator<Item = &'static str> {
        BUNDLED.iter().map(|(c, _)| *c)
    }

    /// Every locale that loads: the bundled ones and the `.toml` files in `folder`.
    pub fn available(folder: Option<&Path>) -> Vec<Locale> {
        let mut codes: Vec<String> = Locale::bundled().map(String::from).collect();
        if let Some(entries) = folder.and_then(|f| std::fs::read_dir(f).ok()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "toml")
                    && let Some(code) = path.file_stem().and_then(|s| s.to_str())
                    && !codes.iter().any(|c| c == code)
                {
                    codes.push(code.to_string());
                }
            }
        }
        codes.iter().filter_map(|c| Locale::load(c, folder).ok()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_thai_loads() {
        let th = Locale::load("th", None).unwrap();
        assert_eq!(th.code, "th");
        assert_eq!(th.tts.voice, "th-TH-PremwadeeNeural");
        assert_eq!(th.text.sentence_split, Split::Space);
    }

    #[test]
    fn unknown_locale_is_an_error() {
        assert!(Locale::load("xx", None).is_err());
    }
}
