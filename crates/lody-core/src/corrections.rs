//! Corrections: what Handy heard next to what you actually sent, so the speech model's mistakes
//! can be found, turned into Handy custom words, and later used to train it.
//!
//! Handy keeps each transcription in `history.db` (with its recording in `recordings/`); a
//! Claude Code prompt that mostly contains a recent transcription is taken as that
//! transcription, possibly edited. Every pair is kept in `<state>/corrections/pairs.jsonl`,
//! with a copy of the recording in `audio/` (Handy deletes its own after a few).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Only transcriptions this recent can belong to a prompt you just sent.
const WITHIN_SECS: u64 = 15 * 60;
/// Longest text compared (the word comparison grows with the square of the length).
const MAX_WORDS: usize = 1500;

/// One transcription from Handy's history.
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub handy_id: i64,
    /// Seconds since 1970.
    pub at: u64,
    /// What Handy pasted: the post-processed text when there is one (Super+E), else the
    /// transcription.
    pub text: String,
    pub post_processed: bool,
    /// The recording's file name in Handy's `recordings/`.
    pub file: String,
}

/// A changed stretch: `from` (what Handy wrote) became `to` (what you sent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Sent exactly as Handy wrote it.
    Same,
    /// Small changes: most likely fixing what the model got wrong.
    Fix,
    /// Much changed: rewording rather than correcting; check before using it to train.
    Rewrite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pair {
    pub id: u64,
    pub at: u64,
    pub project: String,
    pub heard: String,
    pub sent: String,
    pub spans: Vec<Span>,
    pub kind: Kind,
    pub post_processed: bool,
    pub handy_id: i64,
    /// The copied recording's file name in `audio/`, if it was kept.
    pub audio: Option<String>,
}

// --- Handy's history ---

fn handy_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("com.pais.handy")
}

pub fn history_path() -> PathBuf {
    handy_dir().join("history.db")
}

pub fn recordings_dir() -> PathBuf {
    handy_dir().join("recordings")
}

/// Handy's transcriptions from `since` on, newest first.
pub fn read_history(db: &Path, since: u64) -> Result<Vec<Heard>, Error> {
    use rusqlite::{Connection, OpenFlags};
    let err = |e: rusqlite::Error| Error::Config(format!("Handy history: {e}"));
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    conn.busy_timeout(std::time::Duration::from_secs(2)).map_err(err)?;
    let mut query = conn
        .prepare(
            "SELECT id, timestamp, transcription_text, post_processed_text, file_name
             FROM transcription_history WHERE timestamp >= ?1 ORDER BY timestamp DESC",
        )
        .map_err(err)?;
    let rows = query
        .query_map([since as i64], |row| {
            let raw: String = row.get(2)?;
            let post: Option<String> = row.get(3)?;
            let post = post.filter(|p| !p.trim().is_empty());
            Ok(Heard {
                handy_id: row.get(0)?,
                at: row.get::<_, i64>(1)?.max(0) as u64,
                post_processed: post.is_some(),
                text: post.unwrap_or(raw),
                file: row.get(4)?,
            })
        })
        .map_err(err)?;
    Ok(rows.flatten().collect())
}

// --- Comparing ---

/// Words for comparing: runs of Latin letters and digits, runs of Thai (or other) letters,
/// spaces, and single marks. Thai has no spaces between words, so a Thai run is a phrase.
fn words(text: &str) -> Vec<String> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Latin,
        Letter,
        Space,
        Mark,
    }
    let class = |c: char| {
        if c.is_ascii_alphanumeric() {
            Class::Latin
        } else if c.is_alphanumeric() || ('\u{0E00}'..='\u{0E7F}').contains(&c) {
            Class::Letter
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Mark
        }
    };
    let mut out: Vec<String> = Vec::new();
    let mut last = None;
    for c in text.chars() {
        let k = class(c);
        match out.last_mut() {
            Some(w) if Some(k) == last && k != Class::Mark => w.push(c),
            _ => out.push(c.to_string()),
        }
        last = Some(k);
    }
    out
}

/// Longest common subsequence of `a` and `b`, as index pairs.
fn common(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// What changed from `heard` to `sent`, and how much of `heard` is still in `sent` (0 to 1).
pub fn compare(heard: &str, sent: &str) -> (Vec<Span>, f32) {
    let a: Vec<String> = words(heard).into_iter().take(MAX_WORDS).collect();
    let b: Vec<String> = words(sent).into_iter().take(MAX_WORDS).collect();
    let same = common(&a, &b);
    let kept: usize = same.iter().map(|&(i, _)| a[i].chars().count()).sum();
    let total: usize = a.iter().map(|w| w.chars().count()).sum();
    let mut spans = Vec::new();
    let (mut i, mut j) = (0, 0);
    for &(si, sj) in same.iter().chain(std::iter::once(&(a.len(), b.len()))) {
        if si > i || sj > j {
            let span = Span { from: a[i..si].concat(), to: b[j..sj].concat() };
            if span.from.trim() != span.to.trim() {
                spans.push(Span { from: span.from.trim().into(), to: span.to.trim().into() });
            }
        }
        (i, j) = (si + 1, sj + 1);
    }
    (spans, if total == 0 { 0.0 } else { kept as f32 / total as f32 })
}

/// The transcription `sent` was made from: the newest recent one mostly still in it.
pub fn find_heard<'a>(
    heard: &'a [Heard],
    sent: &str,
    used: &HashSet<i64>,
) -> Option<(&'a Heard, Vec<Span>, Kind)> {
    heard.iter().filter(|h| !used.contains(&h.handy_id) && !h.text.trim().is_empty()).find_map(
        |h| {
            let (spans, kept) = compare(&h.text, sent);
            if kept < 0.5 {
                return None; // typed something else
            }
            let changed: usize = spans.iter().map(|s| s.from.chars().count()).sum();
            let length = h.text.chars().count().max(1);
            let kind = if spans.is_empty() {
                Kind::Same
            } else if changed * 3 > length || kept < 0.7 {
                Kind::Rewrite
            } else {
                Kind::Fix
            };
            Some((h, spans, kind))
        },
    )
}

// --- Keeping them ---

pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `<state>/corrections` (`~/.local/state/lody/corrections` on Linux).
    pub fn open_default() -> Store {
        Store { dir: crate::settings::state_dir().join("corrections") }
    }

    pub fn at(dir: PathBuf) -> Store {
        Store { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn pairs_file(&self) -> PathBuf {
        self.dir.join("pairs.jsonl")
    }

    pub fn audio_dir(&self) -> PathBuf {
        self.dir.join("audio")
    }

    /// Keep `pair`, copying its recording from `recordings` when `keep_audio`.
    pub fn add(&self, mut pair: Pair, recording: Option<&Path>) -> Result<Pair, Error> {
        std::fs::create_dir_all(&self.dir)?;
        if let Some(src) = recording.filter(|p| p.exists()) {
            std::fs::create_dir_all(self.audio_dir())?;
            let name = src.file_name().unwrap().to_string_lossy().into_owned();
            std::fs::copy(src, self.audio_dir().join(&name))?;
            pair.audio = Some(name);
        }
        let line = serde_json::to_string(&pair).map_err(|e| Error::Config(e.to_string()))?;
        let mut file =
            std::fs::OpenOptions::new().create(true).append(true).open(self.pairs_file())?;
        writeln!(file, "{line}")?;
        Ok(pair)
    }

    /// Every kept pair, oldest first (unreadable lines are skipped).
    pub fn all(&self) -> Vec<Pair> {
        std::fs::read_to_string(self.pairs_file())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Forget a pair and its recording.
    pub fn remove(&self, id: u64) -> Result<(), Error> {
        let (gone, kept): (Vec<Pair>, Vec<Pair>) = self.all().into_iter().partition(|p| p.id == id);
        for pair in gone {
            if let Some(audio) = pair.audio {
                let _ = std::fs::remove_file(self.audio_dir().join(audio));
            }
        }
        let mut text = String::new();
        for pair in &kept {
            text += &serde_json::to_string(pair).map_err(|e| Error::Config(e.to_string()))?;
            text.push('\n');
        }
        let tmp = self.pairs_file().with_extension("jsonl.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, self.pairs_file())?;
        Ok(())
    }
}

/// English words you corrected Handy into, most often first, leaving out `known` ones: the
/// candidates for Handy's custom words (which only match A–Z and digits).
pub fn suggested_words(pairs: &[Pair], known: &[String]) -> Vec<(String, usize)> {
    let known: HashSet<String> = known.iter().map(|w| w.to_lowercase()).collect();
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut spelling: HashMap<String, String> = HashMap::new();
    for pair in pairs.iter().filter(|p| p.kind == Kind::Fix && !p.post_processed) {
        for span in &pair.spans {
            let before: HashSet<String> =
                words(&span.from).iter().map(|w| w.to_lowercase()).collect();
            for word in words(&span.to) {
                let key = word.to_lowercase();
                let latin = word.chars().all(|c| c.is_ascii_alphanumeric())
                    && word.chars().any(|c| c.is_ascii_alphabetic());
                if latin && word.len() >= 2 && !before.contains(&key) && !known.contains(&key) {
                    *counts.entry(key.clone()).or_default() += 1;
                    spelling.entry(key).or_insert(word);
                }
            }
        }
    }
    let mut out: Vec<(String, usize)> =
        counts.into_iter().map(|(k, n)| (spelling.remove(&k).unwrap_or(k), n)).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Look for the transcription behind a prompt you just sent and keep the pair.
pub fn record_prompt(
    store: &Store,
    project: &str,
    sent: &str,
    used: &mut HashSet<i64>,
    keep_audio: bool,
) -> Result<Option<Pair>, Error> {
    let db = history_path();
    if !db.exists() || sent.trim().is_empty() {
        return Ok(None);
    }
    let now = now_secs();
    let heard = read_history(&db, now.saturating_sub(WITHIN_SECS))?;
    let Some((h, spans, kind)) = find_heard(&heard, sent, used) else { return Ok(None) };
    used.insert(h.handy_id);
    let pair = Pair {
        id: now * 1000 + (h.handy_id.rem_euclid(1000)) as u64,
        at: now,
        project: project.to_string(),
        heard: h.text.clone(),
        sent: sent.to_string(),
        spans,
        kind,
        post_processed: h.post_processed,
        handy_id: h.handy_id,
        audio: None,
    };
    let recording = recordings_dir().join(&h.file);
    store.add(pair, keep_audio.then_some(recording.as_path())).map(Some)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heard(id: i64, text: &str) -> Heard {
        Heard { handy_id: id, at: 0, text: text.into(), post_processed: false, file: String::new() }
    }

    #[test]
    fn a_fixed_word_is_found() {
        let (spans, kept) = compare("please fix the Odo invoice", "please fix the Odoo invoice");
        assert_eq!(spans, vec![Span { from: "Odo".into(), to: "Odoo".into() }]);
        assert!(kept > 0.8);
    }

    #[test]
    fn thai_with_english_terms() {
        let (spans, _) = compare("ช่วยแก้ ไอดู เรื่อง อินวอยซ์", "ช่วยแก้ Odoo เรื่อง invoice");
        assert_eq!(
            spans,
            vec![
                Span { from: "ไอดู".into(), to: "Odoo".into() },
                Span { from: "อินวอยซ์".into(), to: "invoice".into() }
            ]
        );
    }

    #[test]
    fn a_prompt_is_matched_to_its_transcription() {
        let history = [heard(2, "something else entirely"), heard(1, "fix the Odo invoice please")];
        let used = HashSet::new();
        let (h, spans, kind) = find_heard(&history, "fix the Odoo invoice please", &used).unwrap();
        assert_eq!((h.handy_id, kind, spans.len()), (1, Kind::Fix, 1));

        let (_, _, kind) = find_heard(&history, "fix the Odo invoice please", &used).unwrap();
        assert_eq!(kind, Kind::Same);
        assert!(find_heard(&history, "a typed prompt about cats", &used).is_none());
        let used: HashSet<i64> = [1].into();
        assert!(find_heard(&history, "fix the Odoo invoice please", &used).is_none());
    }

    #[test]
    fn heavy_changes_count_as_rewriting() {
        let history = [heard(1, "can you look at the report and the the thing")];
        let sent = "can you look at the report, then explain every chart in it and suggest fixes";
        let (_, _, kind) = find_heard(&history, sent, &HashSet::new()).unwrap();
        assert_eq!(kind, Kind::Rewrite);
    }

    #[test]
    fn suggests_english_words_you_keep_fixing() {
        let pair = |from: &str, to: &str| Pair {
            id: 0,
            at: 0,
            project: String::new(),
            heard: from.into(),
            sent: to.into(),
            spans: compare(from, to).0,
            kind: Kind::Fix,
            post_processed: false,
            handy_id: 0,
            audio: None,
        };
        let pairs = [
            pair("แก้ ไอดู หน่อย", "แก้ Odoo หน่อย"),
            pair("เปิด ไอดู ก่อน", "เปิด Odoo ก่อน"),
            pair("ดู invoic นี้", "ดู invoice นี้"),
        ];
        let words = suggested_words(&pairs, &["invoice".into()]);
        assert_eq!(words, vec![("Odoo".to_string(), 2)]);
    }

    #[test]
    fn pairs_are_kept_and_removed_with_their_audio() {
        let dir = std::env::temp_dir().join(format!("lody-corr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::at(dir.clone());
        let wav = dir.join("in.wav");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&wav, b"RIFF").unwrap();
        let pair = Pair {
            id: 7,
            at: 1,
            project: "p".into(),
            heard: "a".into(),
            sent: "b".into(),
            spans: vec![],
            kind: Kind::Fix,
            post_processed: false,
            handy_id: 3,
            audio: None,
        };
        let kept = store.add(pair, Some(&wav)).unwrap();
        assert_eq!(kept.audio.as_deref(), Some("in.wav"));
        assert!(store.audio_dir().join("in.wav").exists());
        assert_eq!(store.all(), vec![kept]);
        store.remove(7).unwrap();
        assert!(store.all().is_empty() && !store.audio_dir().join("in.wav").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_handys_history() {
        let dir = std::env::temp_dir().join(format!("lody-hist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("history.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE transcription_history (id INTEGER PRIMARY KEY, file_name TEXT NOT NULL,
               timestamp INTEGER NOT NULL, saved BOOLEAN NOT NULL DEFAULT 0, title TEXT NOT NULL,
               transcription_text TEXT NOT NULL, post_processed_text TEXT,
               post_process_prompt TEXT, post_process_requested BOOLEAN NOT NULL DEFAULT 0);
             INSERT INTO transcription_history VALUES
               (1, 'a.wav', 100, 0, '', 'old', NULL, NULL, 0),
               (2, 'b.wav', 200, 0, '', 'สวัสดี', 'Hello', 'p', 1);",
        )
        .unwrap();
        drop(conn);
        let got = read_history(&db, 150).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].text.as_str(), got[0].post_processed), ("Hello", true));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
