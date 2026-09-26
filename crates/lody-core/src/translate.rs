//! Translation engines. Input is protected prose only (see `filter`), one sentence per line.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, Instant};

use fancy_regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Error;

/// Something that translates text from one language code to another (the engine's codes).
pub trait Translator: Send + Sync {
    fn translate(&self, text: &str, source: &str, target: &str) -> Result<String, Error>;
}

/// Google's free web endpoint (the one the Translate widget uses): no key, no account.
pub struct Google {
    agent: ureq::Agent,
}

const GOOGLE_URL: &str = "https://translate.googleapis.com/translate_a/single";
const CHUNK_CHARS: usize = 4000;

impl Google {
    pub fn new(timeout: Duration) -> Google {
        crate::init_tls();
        let config = ureq::Agent::config_builder().timeout_global(Some(timeout)).build();
        Google { agent: config.into() }
    }
}

impl Translator for Google {
    fn translate(&self, text: &str, source: &str, target: &str) -> Result<String, Error> {
        // POST, so the text never lands in URLs or proxy logs.
        let body = self
            .agent
            .post(GOOGLE_URL)
            .query("client", "gtx")
            .query("sl", source)
            .query("tl", target)
            .query("dt", "t")
            .header("User-Agent", "Mozilla/5.0")
            .send_form([("q", text)])
            .map_err(|e| Error::Translate(format!("google: {e}")))?
            .body_mut()
            .read_to_string()
            .map_err(|e| Error::Translate(format!("google: {e}")))?;
        let payload: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| Error::Translate(format!("google: {e}")))?;
        let parts = payload
            .get(0)
            .and_then(|p| p.as_array())
            .ok_or_else(|| Error::Translate("google: unexpected answer".into()))?;
        Ok(parts.iter().filter_map(|part| part.get(0)?.as_str()).collect())
    }
}

/// Google's second free endpoint (the one its Chrome extension uses). Same translations, but
/// limited separately from the first.
pub struct GoogleExtension {
    agent: ureq::Agent,
}

const GOOGLE_EXTENSION_URL: &str = "https://clients5.google.com/translate_a/t";

impl GoogleExtension {
    pub fn new(timeout: Duration) -> GoogleExtension {
        crate::init_tls();
        let config = ureq::Agent::config_builder().timeout_global(Some(timeout)).build();
        GoogleExtension { agent: config.into() }
    }
}

impl Translator for GoogleExtension {
    fn translate(&self, text: &str, source: &str, target: &str) -> Result<String, Error> {
        let body = self
            .agent
            .post(GOOGLE_EXTENSION_URL)
            .query("client", "dict-chrome-ex")
            .query("sl", source)
            .query("tl", target)
            .header("User-Agent", "Mozilla/5.0")
            .send_form([("q", text)])
            .map_err(|e| Error::Translate(format!("google extension: {e}")))?
            .body_mut()
            .read_to_string()
            .map_err(|e| Error::Translate(format!("google extension: {e}")))?;
        let payload: serde_json::Value = serde_json::from_str(&body)
            .map_err(|e| Error::Translate(format!("google extension: {e}")))?;
        // `["text"]`, or `[["text", "detected language"]]` when the source is "auto".
        let first = payload.get(0);
        first
            .and_then(|p| p.as_str().or_else(|| p.get(0)?.as_str()))
            .map(str::to_string)
            .ok_or_else(|| Error::Translate("google extension: unexpected answer".into()))
    }
}

/// A Claude model through the `claude` command, so it uses your Claude Code sign-in: no key.
/// Slower than Google (5 to 30 s) and it uses your Claude plan, but it reads more naturally and
/// keeps names of files and programs as they are. Its sessions are not saved, so Lody never reads
/// them back as replies.
pub struct Claude {
    program: PathBuf,
    model: String,
    timeout: Duration,
}

impl Claude {
    /// `None` when the `claude` command is not installed.
    pub fn find(model: &str, timeout: Duration) -> Option<Claude> {
        let home = dirs::home_dir().unwrap_or_default();
        let exe = if cfg!(windows) { "claude.exe" } else { "claude" };
        // On Windows npm also leaves a `claude` shell script, which only `claude.cmd` can run.
        let program = crate::which("claude").or_else(|| {
            [home.join(".local/bin").join(exe), home.join(".claude/local").join(exe)]
                .into_iter()
                .find(|p| p.is_file())
        })?;
        Some(Claude { program, model: model.to_string(), timeout })
    }
}

impl Translator for Claude {
    fn translate(&self, text: &str, source: &str, target: &str) -> Result<String, Error> {
        let fail = |e: String| Error::Translate(format!("claude: {e}"));
        // Lines go as a JSON array so they come back one to one.
        let lines: Vec<&str> = text.split('\n').collect();
        let prompt = format!(
            "You are a translator. The input is a JSON array of strings in the language with code \
             \"{source}\". Answer with only a JSON array of the same length: each string translated \
             into the language with code \"{target}\", naturally, as it would be said aloud. Keep \
             every ⟦n⟧ token exactly as it is. Keep names of programs, files and commands as they \
             are."
        );
        let mut child = crate::background(&self.program)
            .args(["-p", "--model", &self.model, "--no-session-persistence", "--tools", ""])
            .args(["--setting-sources", "", "--system-prompt", &prompt])
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| fail(e.to_string()))?;
        let input = serde_json::to_string(&lines).unwrap();
        let mut stdin = child.stdin.take().unwrap();
        std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let mut stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            let mut out = String::new();
            stdout.read_to_string(&mut out).map(|_| out)
        });
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| fail(e.to_string()))? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(format!("no answer in {} s", self.timeout.as_secs())));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let out = reader.join().unwrap().map_err(|e| fail(e.to_string()))?;
        if !status.success() {
            return Err(fail(format!("{status}: {}", out.trim())));
        }
        // It may wrap the array in a code fence.
        let json = match (out.find('['), out.rfind(']')) {
            (Some(a), Some(b)) if a < b => &out[a..=b],
            _ => return Err(fail("unexpected answer".into())),
        };
        let translated: Vec<String> =
            serde_json::from_str(json).map_err(|e| fail(e.to_string()))?;
        if translated.len() != lines.len() {
            return Err(fail("did not keep the lines one to one".into()));
        }
        Ok(translated.join("\n"))
    }
}

/// A translator Lody offers, with a short note shown when choosing (`translators.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// As `translator` in the settings stores it.
    pub id: String,
    pub name: String,
    /// Who makes it, as shown ("Google").
    pub engine: String,
    /// The suggested translator.
    #[serde(default)]
    pub best: bool,
    /// 1 (slow) to 5 (fast): how soon the translation comes back.
    pub speed: u8,
    /// 1 (stiff) to 5 (as a person would say it).
    pub quality: u8,
    /// What it needs besides the internet ("Claude Code"), if anything.
    #[serde(default)]
    pub needs: Option<String>,
    pub note: String,
}

#[derive(Deserialize)]
struct CatalogFile {
    translator: Vec<Entry>,
}

/// The translators to choose from, the suggested one first.
pub fn catalog() -> Vec<Entry> {
    let file: CatalogFile =
        toml::from_str(include_str!("../translators.toml")).expect("translators.toml is valid");
    let mut entries = file.translator;
    entries.sort_by_key(|e| !e.best); // stable: otherwise in the file's order
    entries
}

/// The translator an id names (see `translators.toml`); unknown ids get Google's.
fn engine(id: &str, timeout: Duration) -> Box<dyn Translator> {
    match id.split_once(':') {
        Some(("google", "extension")) => Box::new(GoogleExtension::new(timeout)),
        Some(("claude", model)) => {
            // A model takes longer than a web service: at least half a minute.
            match Claude::find(model, timeout.max(Duration::from_secs(30))) {
                Some(claude) => Box::new(claude),
                None => Box::new(Missing("the claude command is not installed")),
            }
        }
        _ => {
            if id != "google" {
                log::warn!("unknown translator {id:?}: using Google Translate");
            }
            Box::new(Google::new(timeout))
        }
    }
}

/// A translator that can't be used here, saying why each time it is asked.
struct Missing(&'static str);

impl Translator for Missing {
    fn translate(&self, _: &str, _: &str, _: &str) -> Result<String, Error> {
        Err(Error::Translate(self.0.into()))
    }
}

/// The translator chosen in the settings, changed in place when they change.
pub struct Translators {
    engine: RwLock<Arc<dyn Translator>>,
}

impl Translators {
    pub fn new(timeout: Duration, id: &str) -> Translators {
        Translators { engine: RwLock::new(engine(id, timeout).into()) }
    }

    pub fn configure(&self, timeout: Duration, id: &str) {
        *self.engine.write().unwrap() = engine(id, timeout).into();
    }
}

impl Translator for Translators {
    fn translate(&self, text: &str, source: &str, target: &str) -> Result<String, Error> {
        // A copy, so changing the settings needn't wait for a slow answer.
        let engine = self.engine.read().unwrap().clone();
        engine.translate(text, source, target)
    }
}

fn batches(sentences: &[String]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut size = 0;
    for sentence in sentences {
        let sentence = sentence.replace('\n', " ");
        match out.last_mut() {
            Some(current) if size + sentence.len() <= CHUNK_CHARS => {
                size += sentence.len() + 1;
                current.push(sentence);
            }
            _ => {
                size = sentence.len() + 1;
                out.push(vec![sentence]);
            }
        }
    }
    out
}

/// Translate keeping a 1:1 sentence mapping, so the caller can rebuild the layout.
pub fn translate_sentences(
    engine: &dyn Translator,
    sentences: &[String],
    source: &str,
    target: &str,
) -> Result<Vec<String>, Error> {
    static DOUBLE_DOT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?<!\.)\.\.(?!\.)").unwrap());
    let mut out = Vec::with_capacity(sentences.len());
    for batch in batches(sentences) {
        let answer = engine.translate(&batch.join("\n"), source, target)?;
        let mut lines: Vec<String> = answer.split('\n').map(str::to_string).collect();
        if lines.len() != batch.len() {
            // The engine merged or split lines; fall back to one request per sentence.
            if batch.len() > 20 {
                return Err(Error::Translate("engine did not keep line structure".into()));
            }
            lines = batch
                .iter()
                .map(|s| engine.translate(s, source, target).map(|t| t.replace('\n', " ")))
                .collect::<Result<_, _>>()?;
        }
        out.extend(lines.iter().map(|l| DOUBLE_DOT.replace_all(l, ".").trim().to_string()));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Uppercases each line and records requests; can merge lines to test the fallback.
    #[derive(Default)]
    pub struct Fake {
        pub calls: Mutex<Vec<String>>,
        pub merge: bool,
        pub fail: bool,
    }

    impl Translator for Fake {
        fn translate(&self, text: &str, _: &str, _: &str) -> Result<String, Error> {
            self.calls.lock().unwrap().push(text.to_string());
            if self.fail {
                return Err(Error::Translate("offline".into()));
            }
            let upper = text.to_uppercase();
            Ok(if self.merge { upper.replace('\n', " ") } else { upper })
        }
    }

    #[test]
    fn sentences_go_in_one_request_and_come_back_one_to_one() {
        let fake = Fake::default();
        let got = translate_sentences(&fake, &["a.".into(), "b..".into()], "en", "th").unwrap();
        assert_eq!(got, vec!["A.", "B."]);
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn merged_lines_fall_back_to_one_request_per_sentence() {
        let fake = Fake { merge: true, ..Default::default() };
        let got = translate_sentences(&fake, &["a".into(), "b".into()], "en", "th").unwrap();
        assert_eq!(got, vec!["A", "B"]);
        assert_eq!(fake.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn every_catalog_entry_names_a_translator_lody_has() {
        let entries = catalog();
        assert!(entries[0].best && entries[0].id == "google");
        for e in &entries {
            let known = ["google", "google:extension"].contains(&e.id.as_str())
                || e.id.starts_with("claude:");
            assert!(known, "{}", e.id);
        }
    }

    /// Talks to Google: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn google_extension_translates_line_by_line() {
        let got =
            GoogleExtension::new(Duration::from_secs(8)).translate("hello\nworld", "en", "th");
        assert_eq!(got.unwrap().lines().count(), 2);
    }

    /// Needs the `claude` command, signed in: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn claude_translates_line_by_line_keeping_tokens() {
        let claude = Claude::find("haiku", Duration::from_secs(60)).expect("claude installed");
        let got = claude.translate("Run the tests.\nI fixed ⟦0⟧ in ⟦1⟧.", "en", "th").unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines.len(), 2, "{got}");
        assert!(lines[1].contains("⟦0⟧") && lines[1].contains("⟦1⟧"), "{got}");
    }

    #[test]
    fn long_replies_are_split_into_batches() {
        let long: Vec<String> = (0..3).map(|_| "x".repeat(1500)).collect();
        assert_eq!(batches(&long).len(), 2);
    }
}
