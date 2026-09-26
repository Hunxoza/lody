//! Any program that reads aloud: it gets the text on its input and writes the audio (WAV or MP3)
//! to its output, e.g. `espeak-ng -v th --stdout` or `piper --model th.onnx --output_file -`.
//! `{rate}` in the command becomes the speed in percent ("10", "-5").

use super::rate_percent;
use crate::Error;

pub fn synthesize(template: &str, text: &str, rate: &str) -> Result<Vec<u8>, Error> {
    let line = template.trim().replace("{rate}", &rate_percent(rate).to_string());
    if line.is_empty() {
        return Err(Error::Voice("no command set for the custom voice".into()));
    }
    let mut command = crate::background(if cfg!(windows) { "cmd" } else { "sh" });
    command.args([if cfg!(windows) { "/C" } else { "-c" }, &line]);
    let audio = super::system::run(command, Some(text))?;
    if audio.is_empty() {
        return Err(Error::Voice("the custom voice's command wrote no audio".into()));
    }
    Ok(audio)
}
