//! Text -> audio. An engine turns one chunk of text into audio bytes (MP3 or WAV); playing them
//! is the player's job (see `speaker`).
//!
//! A voice is named `engine:name`: `edge:th-TH-PremwadeeNeural`, `google:th`, `system:th`,
//! `command:`, `openai:alloy`. A name without an engine is an Edge voice, as Lody's settings
//! and locales wrote them before there were others. `catalog` lists the voices to choose from.

pub mod catalog;
mod command;
mod edge;
mod google;
mod openai;
mod system;

use std::sync::RwLock;

pub use edge::{Edge, EdgeVoice, edge_voices};
pub use google::GoogleVoice;

use crate::Error;
use crate::settings::VoiceEngines;

pub trait Voice: Send + Sync {
    /// Audio of `text` spoken by `voice` at `rate` ("+10%", "-5%").
    fn synthesize(&self, text: &str, voice: &str, rate: &str) -> Result<Vec<u8>, Error>;
}

/// The engines a voice name can start with.
pub const ENGINES: &[&str] = &["edge", "google", "system", "command", "openai"];

/// `"google:th"` -> `("google", "th")`; a bare name is Edge's.
pub fn split_id(voice: &str) -> (&str, &str) {
    match voice.split_once(':') {
        Some((engine, name)) if ENGINES.contains(&engine) => (engine, name),
        _ => ("edge", voice),
    }
}

/// `"th-TH-PremwadeeNeural"` -> `"edge:th-TH-PremwadeeNeural"`; full names stay as they are.
pub fn full_id(voice: &str) -> String {
    let (engine, name) = split_id(voice);
    format!("{engine}:{name}")
}

/// "+10%" -> 10; anything unreadable is the normal speed.
pub fn rate_percent(rate: &str) -> i32 {
    rate.trim().trim_end_matches('%').trim_start_matches('+').parse().unwrap_or(0)
}

/// Every engine behind one `Voice`: each chunk goes to the engine its voice names. The custom
/// command and the OpenAI-compatible service need settings, set with `configure`. A chunk its
/// voice fails on is read by the fallback voice (`set_fallback`) rather than skipped.
pub struct Voices {
    edge: Edge,
    google: GoogleVoice,
    engines: RwLock<VoiceEngines>,
    fallback: RwLock<Option<String>>,
}

impl Voices {
    pub fn new(engines: &VoiceEngines) -> Voices {
        Voices {
            edge: Edge,
            google: GoogleVoice::new(),
            engines: RwLock::new(engines.clone()),
            fallback: RwLock::default(),
        }
    }

    pub fn configure(&self, engines: &VoiceEngines) {
        *self.engines.write().unwrap() = engines.clone();
    }

    /// The voice for chunks another voice fails on; `None` skips them.
    pub fn set_fallback(&self, voice: Option<String>) {
        *self.fallback.write().unwrap() = voice;
    }

    fn synthesize_with(&self, text: &str, voice: &str, rate: &str) -> Result<Vec<u8>, Error> {
        let (engine, name) = split_id(voice);
        match engine {
            "edge" => self.edge.synthesize(text, name, rate),
            "google" => self.google.synthesize(text, name, rate),
            "system" => system::synthesize(text, name, rate),
            "command" => command::synthesize(&self.engines.read().unwrap().command, text, rate),
            "openai" => {
                let config = self.engines.read().unwrap().openai.clone();
                openai::synthesize(&config, text, name, rate)
            }
            _ => unreachable!("split_id only returns known engines"),
        }
    }
}

impl Voice for Voices {
    fn synthesize(&self, text: &str, voice: &str, rate: &str) -> Result<Vec<u8>, Error> {
        let error = match self.synthesize_with(text, voice, rate) {
            Ok(audio) => return Ok(audio),
            Err(e) => e,
        };
        let fallback = self.fallback.read().unwrap().clone();
        match fallback.filter(|f| split_id(f).0 != split_id(voice).0) {
            Some(fallback) => {
                log::warn!("{error}; reading this sentence with {fallback}");
                self.synthesize_with(text, &fallback, rate)
            }
            None => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_read_like_edge_expects() {
        assert_eq!(rate_percent("+10%"), 10);
        assert_eq!(rate_percent("-5%"), -5);
        assert_eq!(rate_percent(""), 0);
    }

    #[test]
    fn voice_names_say_their_engine_and_bare_names_are_edge() {
        assert_eq!(split_id("google:th"), ("google", "th"));
        assert_eq!(split_id("th-TH-PremwadeeNeural"), ("edge", "th-TH-PremwadeeNeural"));
        assert_eq!(split_id("command:"), ("command", ""));
        assert_eq!(split_id("weird:x"), ("edge", "weird:x"));
        assert_eq!(full_id("th-TH-NiwatNeural"), "edge:th-TH-NiwatNeural");
        assert_eq!(full_id("system:th"), "system:th");
    }

    #[test]
    fn a_custom_command_reads_the_text_from_its_input() {
        if cfg!(windows) {
            return;
        }
        let voices = Voices::new(&VoiceEngines { command: "cat".into(), ..Default::default() });
        assert_eq!(voices.synthesize("hello", "command:", "").unwrap(), b"hello");
        voices.configure(&VoiceEngines { command: String::new(), ..Default::default() });
        assert!(voices.synthesize("hello", "command:", "").is_err());
    }

    #[test]
    fn a_failing_voice_hands_the_sentence_to_the_fallback() {
        if cfg!(windows) {
            return;
        }
        let openai = crate::settings::OpenAiVoice { url: String::new(), ..Default::default() };
        let voices = Voices::new(&VoiceEngines { command: "false".into(), openai });
        voices.set_fallback(Some("command:".into())); // same engine: no second try
        assert!(voices.synthesize("hi", "command:", "").is_err());
        voices.set_fallback(Some("openai:alloy".into())); // tried, and fails on no address
        let error = voices.synthesize("hi", "command:", "").unwrap_err().to_string();
        assert!(error.contains("no address"), "{error}");
    }
}
