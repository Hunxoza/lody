//! Any service with OpenAI's speech API (`POST {url}/audio/speech`): OpenAI itself, or a local
//! server that copies it. The key is sent only to that address.

use std::time::Duration;

use super::rate_percent;
use crate::Error;
use crate::settings::OpenAiVoice;

pub fn synthesize(
    config: &OpenAiVoice,
    text: &str,
    voice: &str,
    rate: &str,
) -> Result<Vec<u8>, Error> {
    let url = config.url.trim().trim_end_matches('/');
    if url.is_empty() {
        return Err(Error::Voice("no address set for the OpenAI-compatible voice".into()));
    }
    crate::init_tls();
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).build().into();
    let mut request = agent.post(format!("{url}/audio/speech"));
    if !config.key.trim().is_empty() {
        request = request.header("Authorization", &format!("Bearer {}", config.key.trim()));
    }
    let body = serde_json::json!({
        "model": config.model,
        "input": text,
        "voice": if voice.is_empty() { "alloy" } else { voice },
        "response_format": "mp3",
        "speed": (1.0 + rate_percent(rate) as f32 / 100.0).clamp(0.25, 4.0),
    });
    let audio = request
        .send_json(body)
        .map_err(|e| Error::Voice(format!("openai: {e}")))?
        .body_mut()
        .read_to_vec()
        .map_err(|e| Error::Voice(format!("openai: {e}")))?;
    if audio.is_empty() {
        return Err(Error::Voice("openai: no audio".into()));
    }
    Ok(audio)
}
