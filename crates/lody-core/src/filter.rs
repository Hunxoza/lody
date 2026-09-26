//! Turn an AI reply into translatable prose plus protected slots.
//!
//! Everything that must not be translated or sent off the machine (code blocks, tables, inline
//! code, URLs, paths, secrets) is swapped for a `⟦n⟧` token. Each slot keeps two renderings: how
//! it shows in the translated text, and how it is spoken (often nothing).

use std::sync::LazyLock;

use fancy_regex::{Captures, Regex};

use crate::locale::Split;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid pattern")
}

static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| re(r"⟦\s*(\d+)\s*⟧"));
static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?ms)^([ \t]*)(```|~~~)[^\n]*\n.*?^\1\2[ \t]*$"));
static PEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----")
});
static SECRET_RES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\bsk-[A-Za-z0-9_-]{20,}",
        r"\bgh[pousr]_[A-Za-z0-9]{30,}",
        r"\bgithub_pat_\w{30,}",
        r"\bAKIA[0-9A-Z]{16}\b",
        r"\bxox[abprs]-[\w-]{10,}",
        r"\bAIza[\w-]{35}\b",
        r"\beyJ[\w-]{10,}\.[\w-]{10,}\.[\w-]{10,}",
        r"\b[A-Fa-f0-9]{32,}\b",
        r"(?<![\w/])[A-Za-z0-9+/]{40,}={0,2}(?![\w/])",
    ]
    .into_iter()
    .map(re)
    .collect()
});
static SECRET_ASSIGN_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b((?:password|passwd|secret|token|api[_-]?key|access[_-]?key)\s*[:=]\s*)(\S+)")
});
static INLINE_CODE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"`([^`\n]+)`"));
static MD_LINK_RE: LazyLock<Regex> = LazyLock::new(|| re(r"!?\[([^\]\n]*)\]\([^)\s]+\)"));
static URL_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r#"\b(?:https?|ftp|file)://[^\s<>()"']+|\bwww\.[^\s<>()"']+"#));
static PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?<![\w.])(?:~|\.{1,2}|[A-Za-z]:)?(?:[\\/][\w.@+-]+){2,}[\\/]?",
        r"|(?<![\w.])[\w.@+-]+(?:[\\/][\w.@+-]+)+"
    ))
});
static FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?<![\w./\\-])[\w-]+(?:\.[\w-]+)*\.[A-Za-z][A-Za-z0-9]{0,5}\b(?![\\/])"));
static SPEAKABLE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z][\w.+-]{0,23}$"));
static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s{0,3}#{1,6}\s+"));
static QUOTE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*>\s?"));
static BULLET_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^(\s*)([-*+]|\d{1,3}[.)])\s+"));
static RULE_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*([-*_])(\s*\1){2,}\s*$"));
static EMPHASIS_RE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(\*\*|__)(.+?)\1|(?<![\w*])\*(?!\s)([^*\n]+?)\*(?![\w*])"));
static SENTENCE_RE: LazyLock<Regex> = LazyLock::new(|| re(r#"(?<=[.!?])\s+(?=["'(⟦A-Z0-9])"#));
static PUNCT_SPLIT_RE: LazyLock<Regex> = LazyLock::new(|| re(r"(?<=[.!?。！？])\s*"));
static CJK_RE: LazyLock<Regex> = LazyLock::new(|| re(r"^[\u{3000}-\u{9FFF}\u{AC00}-\u{D7AF}]"));
static WORD_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\w"));

const FILE_EXTS: &[&str] = &[
    "py", "js", "ts", "tsx", "jsx", "json", "toml", "yaml", "yml", "md", "txt", "sh", "ps1", "rs",
    "go", "java", "kt", "c", "h", "cpp", "hpp", "cs", "rb", "php", "html", "css", "scss", "xml",
    "sql", "lock", "cfg", "ini", "env", "log", "csv", "vue", "svelte", "swift", "lua",
];

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// `heading`: a markdown heading (`## Setup`), a title more than content.
    Text {
        prefix: String,
        sentences: Vec<String>,
        heading: bool,
    },
    Blank,
    Block {
        slot: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Prepared {
    pub lines: Vec<Line>,
    /// (display, speech) for each ⟦n⟧.
    pub slots: Vec<(String, String)>,
}

impl Prepared {
    /// Put the protected text back: its display form, or its spoken form.
    pub fn restore(&self, text: &str, speech: bool) -> String {
        let mut text = text.to_string();
        for _ in 0..3 {
            // slots may contain other slots (a secret inside a code block)
            if !TOKEN_RE.is_match(&text).unwrap_or(false) {
                break;
            }
            text = TOKEN_RE
                .replace_all(&text, |c: &Captures<'_, str>| {
                    let index: usize = c[1].parse().unwrap_or(usize::MAX);
                    self.slots
                        .get(index)
                        .map(|(display, spoken)| if speech { spoken } else { display })
                        .cloned()
                        .unwrap_or_default()
                })
                .into_owned();
        }
        text
    }

    /// Indexes of non-blank lines, grouped by blank-line separated paragraph.
    pub fn paragraphs(&self) -> Vec<Vec<usize>> {
        let mut groups = Vec::new();
        let mut current = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            if *line == Line::Blank {
                if !current.is_empty() {
                    groups.push(std::mem::take(&mut current));
                }
            } else {
                current.push(i);
            }
        }
        if !current.is_empty() {
            groups.push(current);
        }
        groups
    }
}

#[derive(Default)]
struct Builder {
    slots: Vec<(String, String)>,
    blocks: Vec<usize>,
}

impl Builder {
    fn slot(&mut self, display: &str, speech: &str) -> String {
        self.slots.push((display.to_string(), speech.to_string()));
        format!("⟦{}⟧", self.slots.len() - 1)
    }

    fn block(&mut self, display: &str, speech: &str) -> String {
        let token = self.slot(display, speech);
        self.blocks.push(self.slots.len() - 1);
        token
    }

    fn redact_secrets(&mut self, text: &str) -> String {
        let mut text = PEM_RE
            .replace_all(text, |_: &Captures<'_, str>| self.slot("[private key]", ""))
            .into_owned();
        text = SECRET_ASSIGN_RE
            .replace_all(&text, |c: &Captures<'_, str>| {
                format!("{}{}", &c[1], self.slot("[secret]", ""))
            })
            .into_owned();
        for pattern in SECRET_RES.iter() {
            text = pattern
                .replace_all(&text, |_: &Captures<'_, str>| self.slot("[secret]", ""))
                .into_owned();
        }
        text
    }

    fn protect_inline(&mut self, text: &str) -> String {
        let mut text = INLINE_CODE_RE
            .replace_all(text, |c: &Captures<'_, str>| self.slot(&c[0], &speakable(&c[1])))
            .into_owned();
        text = MD_LINK_RE.replace_all(&text, |c: &Captures<'_, str>| c[1].to_string()).into_owned();
        text = URL_RE
            .replace_all(&text, |c: &Captures<'_, str>| self.trailing(&c[0], |_| String::new()))
            .into_owned();
        text = PATH_RE
            .replace_all(&text, |c: &Captures<'_, str>| self.trailing(&c[0], speak_path))
            .into_owned();
        text = FILE_RE.replace_all(&text, |c: &Captures<'_, str>| self.file(&c[0])).into_owned();
        EMPHASIS_RE
            .replace_all(&text, |c: &Captures<'_, str>| {
                c.get(2).or_else(|| c.get(3)).map(|m| m.as_str()).unwrap_or("").to_string()
            })
            .into_owned()
    }

    /// Protect `value` but leave sentence punctuation (like a final '.') outside the slot.
    fn trailing(&mut self, value: &str, speak: impl Fn(&str) -> String) -> String {
        let stripped = value.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']);
        let rest = &value[stripped.len()..];
        format!("{}{rest}", self.slot(stripped, &speak(stripped)))
    }

    fn file(&mut self, name: &str) -> String {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if !FILE_EXTS.contains(&ext.as_str()) {
            return name.to_string();
        }
        self.slot(name, &speakable(name))
    }
}

fn speakable(code: &str) -> String {
    let code = code.trim();
    let code = code.strip_suffix("()").unwrap_or(code);
    if SPEAKABLE_RE.is_match(code).unwrap_or(false) { code.to_string() } else { String::new() }
}

fn speak_path(path: &str) -> String {
    speakable(path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(""))
}

/// Split on every match of `re` (zero-width matches included), like Python's `re.split`.
fn split<'a>(re: &Regex, text: &'a str) -> Vec<&'a str> {
    let mut pieces = Vec::new();
    let mut last = 0;
    for m in re.find_iter(text).flatten() {
        pieces.push(&text[last..m.start()]);
        last = m.end();
    }
    pieces.push(&text[last..]);
    pieces
}

fn fullmatch_token(text: &str) -> Option<usize> {
    let caps = TOKEN_RE.captures(text).ok().flatten()?;
    let whole = caps.get(0)?;
    (whole.start() == 0 && whole.end() == text.len()).then(|| caps[1].parse().ok())?
}

pub fn prepare(text: &str, code_omitted: &str, table_omitted: &str) -> Prepared {
    let mut b = Builder::default();
    let text = b.redact_secrets(&text.replace("\r\n", "\n"));
    let text = FENCE_RE
        .replace_all(&text, |c: &Captures<'_, str>| {
            format!("\n{}\n", b.block(c[0].trim_matches('\n'), code_omitted))
        })
        .into_owned();

    let raw_lines: Vec<&str> = text.split('\n').collect();
    let mut lines = Vec::new();
    let mut i = 0;
    while i < raw_lines.len() {
        let raw = raw_lines[i];
        if raw.trim_start().starts_with('|') {
            let start = i;
            while i < raw_lines.len() && raw_lines[i].trim_start().starts_with('|') {
                i += 1;
            }
            lines.push(Line::Block { slot: b.slots.len() });
            b.slot(&raw_lines[start..i].join("\n"), table_omitted);
            continue;
        }
        i += 1;
        let stripped = raw.trim();
        if stripped.is_empty() || RULE_RE.is_match(raw).unwrap_or(false) {
            lines.push(Line::Blank);
            continue;
        }
        if let Some(slot) = fullmatch_token(stripped).filter(|s| b.blocks.contains(s)) {
            lines.push(Line::Block { slot });
            continue;
        }
        let heading = HEADING_RE.is_match(raw).unwrap_or(false);
        let body = HEADING_RE.replace(raw, "");
        let mut body = QUOTE_RE.replace(&body, "").into_owned();
        let mut prefix = String::new();
        if let Ok(Some(bullet)) = BULLET_RE.find(&body) {
            prefix = bullet.as_str().to_string();
            body = body[bullet.end()..].to_string();
        }
        let body = b.protect_inline(&body);
        let body = body.trim();
        let sentences = split(&SENTENCE_RE, body)
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .collect();
        lines.push(Line::Text { prefix, sentences, heading });
    }

    // Collapse runs of blank lines, and drop blank lines at the start and end.
    let mut collapsed: Vec<Line> = Vec::new();
    for line in lines {
        if line == Line::Blank && collapsed.last().is_none_or(|l| *l == Line::Blank) {
            continue;
        }
        collapsed.push(line);
    }
    while collapsed.last() == Some(&Line::Blank) {
        collapsed.pop();
    }
    Prepared { lines: collapsed, slots: b.slots }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    All,
    FirstLast,
    Last,
}

/// Line indexes to speak: all, the first and last paragraph, or the last one. A first paragraph
/// that is only a heading brings the paragraph after it along, so the start says something.
pub fn select_lines(prepared: &Prepared, scope: Scope) -> Vec<usize> {
    let groups = prepared.paragraphs();
    let only_headings = |group: &[usize]| {
        group.iter().all(|&i| matches!(prepared.lines[i], Line::Text { heading: true, .. }))
    };
    match (scope, groups.as_slice()) {
        (_, []) => Vec::new(),
        (Scope::All, _) => groups.concat(),
        (Scope::Last, [.., last]) => last.clone(),
        (Scope::FirstLast, [only]) => only.clone(),
        (Scope::FirstLast, [.., last]) => {
            let mut first = 1;
            while first < groups.len() - 1 && only_headings(&groups[first - 1]) {
                first += 1;
            }
            let mut picked = groups[..first].concat();
            if first < groups.len() {
                picked.extend(last);
            }
            picked
        }
    }
}

/// Remove markdown leftovers; empty when nothing speakable is left.
pub fn clean_speech(text: &str) -> String {
    static MARKS: LazyLock<Regex> = LazyLock::new(|| re(r"[*_#`>|~]+"));
    static SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"\s{2,}"));
    static BEFORE_PUNCT: LazyLock<Regex> = LazyLock::new(|| re(r"\s+([,.;:!?])"));
    let text = MARKS.replace_all(text, " ");
    let text = SPACES.replace_all(&text, " ");
    let text = BEFORE_PUNCT.replace_all(&text, "$1");
    let text = text.trim();
    if WORD_RE.is_match(text).unwrap_or(false) { text.to_string() } else { String::new() }
}

/// Split text in the target language into spoken chunks per the locale's rule.
pub fn split_chunks(text: &str, mode: Split) -> Vec<String> {
    const MIN_LEN: usize = 40;
    const MAX_LEN: usize = 220;
    let pieces: Vec<&str> = match mode {
        Split::None => vec![text],
        Split::Punct => split(&PUNCT_SPLIT_RE, text),
        // join words back up so chunks are phrase-sized, not word-sized
        Split::Space => text.split_whitespace().collect(),
    };
    let mut chunks: Vec<String> = Vec::new();
    for piece in pieces.into_iter().map(str::trim).filter(|p| !p.is_empty()) {
        let joiner =
            if mode == Split::Punct && CJK_RE.is_match(piece).unwrap_or(false) { "" } else { " " };
        match chunks.last_mut() {
            Some(last)
                if last.chars().count() < MIN_LEN
                    && last.chars().count() + piece.chars().count() < MAX_LEN =>
            {
                last.push_str(joiner);
                last.push_str(piece);
            }
            _ => chunks.push(piece.to_string()),
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prep(text: &str) -> Prepared {
        prepare(text, "CODE", "TABLE")
    }

    fn sentences(p: &Prepared) -> Vec<String> {
        p.lines
            .iter()
            .filter_map(|l| match l {
                Line::Text { sentences, .. } => Some(sentences.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn code_blocks_and_tables_become_blocks_spoken_as_a_phrase() {
        let p = prep("Intro.\n\n```py\nprint(1)\n```\n\n| a | b |\n|---|---|\n\nEnd.");
        let blocks: Vec<_> = p.lines.iter().filter(|l| matches!(l, Line::Block { .. })).collect();
        assert_eq!(blocks.len(), 2);
        assert!(p.slots.iter().any(|(d, s)| d.contains("print(1)") && s == "CODE"));
        assert!(p.slots.iter().any(|(d, s)| d.starts_with("| a") && s == "TABLE"));
    }

    #[test]
    fn inline_code_paths_urls_and_files_are_protected() {
        let p =
            prep("Run `cargo test` on src/main.rs, see https://example.com/x. Edit lib.rs now.");
        let text = sentences(&p).join(" ");
        assert!(!text.contains("cargo test") && !text.contains("example.com"));
        assert!(!text.contains("src/main.rs") && !text.contains("lib.rs"));
        let spoken = p.restore(&text, true);
        assert!(spoken.contains("main.rs") && spoken.contains("lib.rs"));
        assert!(!spoken.contains("https"));
        assert!(p.restore(&text, false).contains("https://example.com/x"));
        assert!(text.contains(". Edit")); // the URL's final '.' stays outside its slot
    }

    #[test]
    fn secrets_never_reach_the_text() {
        let p = prep("Use key sk-abcdefghijklmnopqrstuvwxyz123 and password: hunter2 please.");
        let text = sentences(&p).join(" ");
        assert!(!text.contains("sk-abc") && !text.contains("hunter2"));
        assert!(p.restore(&text, false).contains("[secret]"));
        assert!(!p.restore(&text, true).contains("secret]"));
    }

    #[test]
    fn markdown_is_stripped_and_sentences_split() {
        let p = prep("## Done\n- **Fixed** the bug. Tests pass.\n> Quoted *note*.");
        assert!(matches!(&p.lines[0], Line::Text { sentences, .. } if sentences == &["Done"]));
        match &p.lines[1] {
            Line::Text { prefix, sentences, .. } => {
                assert_eq!(prefix, "- ");
                assert_eq!(sentences, &["Fixed the bug.", "Tests pass."]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(sentences(&p).last().unwrap(), "Quoted note.");
    }

    #[test]
    fn first_and_last_paragraphs_are_read_by_default() {
        let p = prep("One.\n\nTwo.\n\nThree.");
        let picked: Vec<_> = select_lines(&p, Scope::FirstLast);
        assert_eq!(picked.len(), 2);
        assert_eq!(select_lines(&p, Scope::All).len(), 3);
        assert_eq!(select_lines(&p, Scope::Last).len(), 1);
        assert!(select_lines(&prep(""), Scope::All).is_empty());
    }

    #[test]
    fn a_heading_first_brings_the_paragraph_after_it() {
        let p = prep("## Setup\n\nInstall it first.\n\nMiddle.\n\nThat's all.");
        let spoken: Vec<String> = select_lines(&p, Scope::FirstLast)
            .into_iter()
            .flat_map(|i| match &p.lines[i] {
                Line::Text { sentences, .. } => sentences.clone(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(spoken, ["Setup", "Install it first.", "That's all."]);
        // headings all the way down: nothing is read twice
        assert_eq!(select_lines(&prep("# A\n\n# B"), Scope::FirstLast).len(), 2);
    }

    #[test]
    fn chunks_join_short_thai_phrases() {
        let chunks = split_chunks("แก้ไข แล้ว ครับ ทดสอบ ผ่าน", Split::Space);
        assert_eq!(chunks, vec!["แก้ไข แล้ว ครับ ทดสอบ ผ่าน"]);
        let chunks =
            split_chunks("First sentence here is long enough to stand. Second.", Split::Punct);
        assert_eq!(chunks, vec!["First sentence here is long enough to stand.", "Second."]);
    }

    #[test]
    fn clean_speech_drops_marks_and_empty_leftovers() {
        assert_eq!(clean_speech("**Hello** , world"), "Hello, world");
        assert_eq!(clean_speech(" *** "), "");
    }
}
