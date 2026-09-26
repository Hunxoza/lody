//! The computer's own voice: espeak-ng on Linux, `say` on macOS, Windows' speech (SAPI).
//! Nothing to download; how it sounds depends on the system (espeak-ng is robotic).

use std::io::Write;
use std::process::{Command, Stdio};

use super::rate_percent;
use crate::Error;

/// `voice` is a language code or the system's voice name ("th", "Kanya").
pub fn synthesize(text: &str, voice: &str, rate: &str) -> Result<Vec<u8>, Error> {
    let speed = 1.0 + rate_percent(rate) as f32 / 100.0;
    if cfg!(target_os = "macos") {
        let path = temp_path("aiff");
        let mut command = Command::new("say");
        if !voice.is_empty() {
            command.args(["-v", voice]);
        }
        let words_per_minute = (175.0 * speed).round().to_string();
        command.args(["-r", &words_per_minute, "-o"]).arg(&path).arg(text);
        run(command, None)?;
        read_and_remove(&path)
    } else if cfg!(windows) {
        let path = temp_path("wav");
        // SAPI's rate is -10..10, 0 being normal.
        let sapi_rate = ((speed - 1.0) * 10.0).round().clamp(-10.0, 10.0);
        let script = format!(
            "Add-Type -AssemblyName System.Speech; \
             $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             {pick} $s.Rate = {sapi_rate}; $s.SetOutputToWaveFile('{path}'); \
             $s.Speak([Console]::In.ReadToEnd()); $s.Dispose()",
            pick = if voice.is_empty() || voice.len() <= 3 {
                String::new()
            } else {
                format!("$s.SelectVoice('{}');", voice.replace('\'', "''"))
            },
            path = path.display(),
        );
        let mut command = crate::background("powershell");
        command.args(["-NoProfile", "-Command", &script]);
        run(command, Some(text))?;
        read_and_remove(&path)
    } else {
        let mut command = crate::background("espeak-ng");
        let words_per_minute = (175.0 * speed).round().to_string();
        command.args(["-v", if voice.is_empty() { "en" } else { voice }]);
        command.args(["-s", &words_per_minute, "--stdout"]);
        run(command, Some(text))
    }
}

/// Run `command`, giving it `input`, and return what it wrote.
pub(super) fn run(mut command: Command, input: Option<&str>) -> Result<Vec<u8>, Error> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Voice(format!("{program}: {e}")))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(text.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Voice(format!(
            "{program}: {}",
            err.trim().lines().last().unwrap_or("")
        )));
    }
    Ok(out.stdout)
}

fn temp_path(extension: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("lody-voice-{}-{nanos}.{extension}", std::process::id()))
}

fn read_and_remove(path: &std::path::Path) -> Result<Vec<u8>, Error> {
    let audio = std::fs::read(path);
    let _ = std::fs::remove_file(path);
    Ok(audio?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs espeak-ng with its Thai voice (Linux): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn the_system_voice_reads_thai() {
        let audio = synthesize("ทดสอบเสียงภาษาไทย", "th", "+20%").unwrap();
        assert!(audio.starts_with(b"RIFF") && audio.len() > 5_000, "{} bytes", audio.len());
    }
}
