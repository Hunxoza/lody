//! Google Translate's read-aloud voice: free, no key, one voice per language, no speed control.

use std::time::Duration;

use super::Voice;
use crate::Error;

const URL: &str = "https://translate.google.com/translate_tts";
/// Google refuses longer text in one request.
const MAX_CHARS: usize = 190;

pub struct GoogleVoice {
    agent: ureq::Agent,
}

impl GoogleVoice {
    pub fn new() -> GoogleVoice {
        crate::init_tls();
        let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15)));
        GoogleVoice { agent: config.build().into() }
    }
}

impl Default for GoogleVoice {
    fn default() -> Self {
        GoogleVoice::new()
    }
}

impl Voice for GoogleVoice {
    /// `voice` is a language code ("th").
    fn synthesize(&self, text: &str, voice: &str, _rate: &str) -> Result<Vec<u8>, Error> {
        let language = if voice.is_empty() { "en" } else { voice };
        let mut audio = Vec::new();
        // MP3 pieces played back to back are one MP3.
        for piece in pieces(text, MAX_CHARS) {
            let mut answer = self
                .agent
                .get(URL)
                .query("ie", "UTF-8")
                .query("client", "tw-ob")
                .query("tl", language)
                .query("q", &piece)
                .header("User-Agent", "Mozilla/5.0")
                .call()
                .map_err(|e| Error::Voice(format!("google: {e}")))?;
            let bytes = answer
                .body_mut()
                .read_to_vec()
                .map_err(|e| Error::Voice(format!("google: {e}")))?;
            audio.extend(bytes);
        }
        if audio.is_empty() {
            return Err(Error::Voice("google: no audio".into()));
        }
        Ok(audio)
    }
}

/// Split at spaces into pieces of at most `max` characters (a word longer than that is cut).
fn pieces(text: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match out.last_mut() {
            Some(last) if last.chars().count() + 1 + word.chars().count() <= max => {
                last.push(' ');
                last.push_str(word);
            }
            _ => {
                let chars: Vec<char> = word.chars().collect();
                out.extend(chars.chunks(max).map(|c| c.iter().collect()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_goes_in_pieces_split_at_spaces() {
        assert_eq!(pieces("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(pieces("abcdefg", 3), vec!["abc", "def", "g"]);
        assert!(pieces("  ", 5).is_empty());
    }

    /// Talks to Google: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn google_returns_thai_mp3() {
        let audio = GoogleVoice::new().synthesize("ทดสอบเสียงภาษาไทย", "th", "").unwrap();
        assert!(audio.len() > 5_000, "{} bytes", audio.len());
    }
}
