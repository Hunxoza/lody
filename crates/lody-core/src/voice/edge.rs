//! Microsoft Edge's online neural voices (the browser's Read Aloud): free, no key, no download.

use super::{Voice, rate_percent};
use crate::Error;

pub struct Edge;

impl Voice for Edge {
    fn synthesize(&self, text: &str, voice: &str, rate: &str) -> Result<Vec<u8>, Error> {
        let config = msedge_tts::tts::SpeechConfig {
            voice_name: voice.to_string(),
            audio_format: "audio-24khz-48kbitrate-mono-mp3".to_string(),
            pitch: 0,
            rate: rate_percent(rate),
            volume: 0,
        };
        crate::init_tls();
        // Microsoft's servers reset connections now and then, in bursts when many come quickly;
        // each sentence needs its own connection (a second request on one is reset), so wait
        // longer before each new try.
        let mut last = None;
        for wait_ms in [0, 1000, 2000, 4000] {
            std::thread::sleep(std::time::Duration::from_millis(wait_ms));
            match synthesize_once(text, &config) {
                Ok(audio) => return Ok(audio),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap())
    }
}

fn synthesize_once(text: &str, config: &msedge_tts::tts::SpeechConfig) -> Result<Vec<u8>, Error> {
    let mut client =
        msedge_tts::tts::client::connect().map_err(|e| Error::Voice(format!("edge: {e}")))?;
    let audio = client.synthesize(text, config).map_err(|e| Error::Voice(format!("edge: {e}")))?;
    if audio.audio_bytes.is_empty() {
        return Err(Error::Voice("edge: no audio".into()));
    }
    Ok(audio.audio_bytes)
}

/// One of Edge's voices as its list gives it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct EdgeVoice {
    /// `th-TH-PremwadeeNeural`
    pub name: String,
    /// `th-TH`
    pub locale: String,
    pub gender: String,
}

/// Every voice Edge has now (a few hundred, over the network).
pub fn edge_voices() -> Result<Vec<EdgeVoice>, Error> {
    crate::init_tls();
    let list =
        msedge_tts::voice::get_voices_list().map_err(|e| Error::Voice(format!("edge: {e}")))?;
    Ok(list
        .into_iter()
        .filter_map(|v| {
            Some(EdgeVoice {
                name: v.short_name?,
                locale: v.locale.unwrap_or_default(),
                gender: v.gender.unwrap_or_default(),
            })
        })
        .collect())
}

#[cfg(test)]
mod live {
    use super::*;

    fn is_mp3(audio: &[u8]) -> bool {
        audio.starts_with(b"ID3") || (audio[0] == 0xFF && audio[1] & 0xE0 == 0xE0)
    }

    /// Talks to Microsoft's servers: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn edge_returns_thai_mp3() {
        let audio = Edge.synthesize("ทดสอบเสียงภาษาไทย", "th-TH-PremwadeeNeural", "+0%").unwrap();
        assert!(is_mp3(&audio) && audio.len() > 5_000, "{} bytes", audio.len());
    }

    #[test]
    #[ignore]
    fn edge_lists_its_thai_and_multilingual_voices() {
        let voices = edge_voices().unwrap();
        assert!(voices.iter().any(|v| v.name == "th-TH-NiwatNeural" && v.locale == "th-TH"));
        assert!(voices.iter().filter(|v| v.name.contains("Multilingual")).count() >= 5);
    }
}
