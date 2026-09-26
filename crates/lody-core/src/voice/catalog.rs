//! The voices to choose from, each with a short note: `voices.toml` (bundled) says what Lody
//! knows about a voice; Edge's live list adds its other voices for the language, so new ones
//! show up without a Lody update.

use serde::{Deserialize, Serialize};

use super::EdgeVoice;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// `engine:name`, as `speech.voice` stores it.
    pub id: String,
    pub name: String,
    /// Who makes it, as shown ("Microsoft Edge").
    pub engine: String,
    /// Locale codes it reads; `*` for any.
    pub languages: Vec<String>,
    /// The suggested voice for its language.
    #[serde(default)]
    pub best: bool,
    /// 1 (slow) to 5 (fast): how soon speech starts.
    pub speed: Option<u8>,
    /// 1 (robotic) to 5 (like a person); unknown until someone listened.
    pub natural: Option<u8>,
    /// Needs the internet.
    pub online: bool,
    /// Which settings it needs first: `command` or `openai`.
    #[serde(default)]
    pub setup: Option<String>,
    pub note: String,
}

#[derive(Deserialize)]
struct File {
    voice: Vec<Entry>,
}

const BUNDLED: &str = include_str!("../../voices.toml");

pub fn bundled() -> Vec<Entry> {
    toml::from_str::<File>(BUNDLED).expect("voices.toml is valid").voice
}

fn reads(entry: &Entry, language: &str) -> bool {
    entry.languages.iter().any(|l| l == "*" || l == language)
}

/// The voices for `language` (a locale code): the catalog's first, the suggested one on top,
/// then Edge's other voices for it and its multilingual ones from `edge` (its live list, empty
/// when not fetched).
pub fn for_language(language: &str, edge: &[EdgeVoice]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = bundled().into_iter().filter(|e| reads(e, language)).collect();
    entries.sort_by_key(|e| !e.best); // stable: otherwise in the catalog's order
    let known = |id: &str| entries.iter().any(|e| e.id == id);
    let extra: Vec<Entry> = edge
        .iter()
        .filter(|v| v.locale.split('-').next() == Some(language) || v.name.contains("Multilingual"))
        .filter(|v| !known(&format!("edge:{}", v.name)))
        .map(|v| edge_entry(v, language))
        .collect();
    // Before the engines that need setting up, which the catalog lists last.
    let at = entries.iter().position(|e| e.setup.is_some()).unwrap_or(entries.len());
    entries.splice(at..at, extra);
    entries
}

fn edge_entry(voice: &EdgeVoice, language: &str) -> Entry {
    let short = voice.name.splitn(3, '-').nth(2).unwrap_or(&voice.name);
    let short = short.trim_end_matches("Neural");
    let multilingual = voice.name.contains("Multilingual");
    let name = short.trim_end_matches("Multilingual");
    let gender = voice.gender.to_lowercase();
    Entry {
        id: format!("edge:{}", voice.name),
        name: if multilingual { format!("{name} (multilingual)") } else { name.to_string() },
        engine: "Microsoft Edge".into(),
        languages: vec![language.to_string()],
        best: false,
        // measured: 1.1-2 s a sentence, against 0.6-0.7 s for Edge's Thai voices
        speed: Some(3),
        natural: None,
        online: true,
        setup: None,
        note: if multilingual {
            format!(
                "A {gender} {} voice made for many languages; how it reads yours isn't rated yet: \
                 try it. Free, no account, needs the internet.",
                voice.locale
            )
        } else {
            format!("Another {gender} Edge voice. Free, no account, needs the internet.")
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_loads_and_every_entry_names_a_known_engine() {
        for entry in bundled() {
            let (engine, _) = super::super::split_id(&entry.id);
            assert!(entry.id.starts_with(&format!("{engine}:")), "{}", entry.id);
            assert!(!entry.note.is_empty() && !entry.languages.is_empty(), "{}", entry.id);
        }
    }

    #[test]
    fn thai_lists_the_suggested_voice_first_and_edge_extras_before_setup_ones() {
        let edge = [
            EdgeVoice {
                name: "th-TH-PremwadeeNeural".into(),
                locale: "th-TH".into(),
                gender: "Female".into(),
            },
            EdgeVoice {
                name: "en-US-AvaMultilingualNeural".into(),
                locale: "en-US".into(),
                gender: "Female".into(),
            },
            EdgeVoice {
                name: "de-DE-KatjaNeural".into(),
                locale: "de-DE".into(),
                gender: "Female".into(),
            },
        ];
        let voices = for_language("th", &edge);
        assert!(voices[0].best && voices[0].id == "edge:th-TH-PremwadeeNeural");
        assert_eq!(voices.iter().filter(|v| v.id == "edge:th-TH-PremwadeeNeural").count(), 1);
        assert!(!voices.iter().any(|v| v.id.contains("Katja")));
        let ava = voices.iter().position(|v| v.id == "edge:en-US-AvaMultilingualNeural").unwrap();
        assert_eq!(voices[ava].name, "Ava (multilingual)");
        let setup = voices.iter().position(|v| v.setup.is_some()).unwrap();
        assert!(ava < setup);
    }
}
