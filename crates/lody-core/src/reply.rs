//! One reply: filter -> translate -> the text to show and the chunks to speak.

use std::collections::{HashMap, HashSet};

use fancy_regex::Regex;

use crate::filter::{self, Line};
use crate::locale::{Locale, Split};
use crate::settings::Settings;
use crate::translate::{Translator, translate_sentences};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    /// The whole reply translated (only with `display.enabled`).
    pub display: String,
    pub chunks: Vec<String>,
    pub voice: String,
    pub translated: bool,
    /// Translation failed: the chunks are the English, for `voice` (the fallback voice).
    pub failed: bool,
}

/// A test for "this sentence is already in the locale's language", if the locale has a script.
fn native_check(locale: &Locale) -> Option<impl Fn(&str) -> bool> {
    let letter = Regex::new(&format!("[{}]", locale.text.native_script.as_ref()?)).ok()?;
    Some(move |sentence: &str| {
        // A third is enough: Thai replies mix in English terms ("แก้ handy.py แล้ว"), and Thai
        // vowel and tone marks are not letters, so Thai words count short.
        let letters: Vec<char> = sentence.chars().filter(|c| c.is_alphabetic()).collect();
        let native =
            letters.iter().filter(|c| letter.is_match(&c.to_string()).unwrap_or(false)).count();
        !letters.is_empty() && 3 * native >= letters.len()
    })
}

pub fn process(
    text: &str,
    settings: &Settings,
    locale: &Locale,
    engine: &dyn Translator,
) -> Output {
    let speech = &settings.speech;
    let want_display = settings.display.enabled && settings.translate;
    let prepared = filter::prepare(text, &locale.text.code_omitted, &locale.text.table_omitted);
    let speak_lines: HashSet<usize> =
        filter::select_lines(&prepared, speech.scope).into_iter().collect();
    let sentences_of = |i: usize| match &prepared.lines[i] {
        Line::Text { sentences, .. } => sentences.clone(),
        _ => Vec::new(),
    };
    let needed: Vec<usize> = (0..prepared.lines.len())
        .filter(|i| matches!(prepared.lines[*i], Line::Text { .. }))
        .filter(|i| want_display || speak_lines.contains(i))
        .collect();
    let voice = speech.voice_in(locale);
    let mut out = Output { voice: voice.clone(), ..Output::default() };

    let mut translated: HashMap<usize, Vec<String>> =
        needed.iter().map(|&i| (i, sentences_of(i))).collect();
    if settings.translate && !needed.is_empty() {
        // Sentences already in the locale's language are kept and never sent out.
        let is_native = native_check(locale);
        let todo: Vec<(usize, usize)> = needed
            .iter()
            .flat_map(|&i| (0..translated[&i].len()).map(move |j| (i, j)))
            .filter(|&(i, j)| !is_native.as_ref().is_some_and(|f| f(&translated[&i][j])))
            .collect();
        let batch: Vec<String> = todo.iter().map(|&(i, j)| translated[&i][j].clone()).collect();
        match translate_sentences(engine, &batch, "en", &locale.translate.google) {
            Ok(result) => {
                for (&(i, j), sentence) in todo.iter().zip(result) {
                    translated.get_mut(&i).unwrap()[j] = sentence;
                }
                out.translated = !todo.is_empty();
            }
            Err(e) => {
                log::warn!("translation failed, reading the original: {e}");
                translated = needed.iter().map(|&i| (i, sentences_of(i))).collect();
                out.voice = speech.fallback_voice.clone();
                out.failed = true;
            }
        }
    }

    if want_display && out.translated {
        let rendered: Vec<String> = prepared
            .lines
            .iter()
            .enumerate()
            .map(|(i, line)| match line {
                Line::Blank => String::new(),
                Line::Block { slot } => prepared.slots[*slot].0.clone(),
                Line::Text { prefix, .. } => {
                    format!("{prefix}{}", prepared.restore(&translated[&i].join(" "), false))
                }
            })
            .collect();
        let display = rendered.join("\n").trim().to_string();
        let limit = settings.display.max_chars;
        out.display = if display.chars().count() <= limit {
            display
        } else {
            display.chars().take(limit.saturating_sub(1)).collect::<String>() + "…"
        };
    }

    if speech.enabled {
        // The English fallback is split on punctuation, whatever the locale does.
        let split = if out.voice == voice { locale.text.sentence_split } else { Split::Punct };
        let mut budget = speech.max_chars as isize;
        for (i, line) in prepared.lines.iter().enumerate() {
            if !speak_lines.contains(&i) || budget <= 0 {
                continue;
            }
            let spoken: Vec<String> = match line {
                Line::Block { slot } => vec![prepared.slots[*slot].1.clone()],
                Line::Text { .. } => translated[&i]
                    .iter()
                    .map(|s| filter::clean_speech(&prepared.restore(s, true)))
                    .collect(),
                Line::Blank => Vec::new(),
            };
            for sentence in spoken.iter().filter(|s| !s.is_empty()) {
                for chunk in filter::split_chunks(sentence, split) {
                    if budget <= 0 {
                        break;
                    }
                    budget -= chunk.chars().count() as isize;
                    out.chunks.push(chunk);
                }
            }
        }
        if out.chunks.iter().map(|c| c.chars().count()).sum::<usize>() < speech.min_chars {
            out.chunks.clear();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::tests::Fake;

    fn thai() -> Locale {
        Locale::load("th", None).unwrap()
    }

    #[test]
    fn english_is_translated_and_code_is_announced_not_read() {
        let fake = Fake::default();
        let reply = "Fixed the bug in `main.rs`.\n\n```rust\nfn main() {}\n```";
        let out = process(reply, &Settings::default(), &thai(), &fake);
        assert!(out.translated);
        assert_eq!(out.voice, "th-TH-PremwadeeNeural");
        let spoken = out.chunks.join(" ");
        assert!(spoken.contains("FIXED THE BUG IN main.rs."), "{spoken}");
        assert!(spoken.contains("มีโค้ดหนึ่งส่วน"));
        assert!(!spoken.contains("fn main"));
        // only prose was sent out, never the code
        assert!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .all(|c| !c.contains("fn main") && !c.contains("main.rs"))
        );
    }

    #[test]
    fn thai_sentences_are_not_sent_to_the_translator() {
        let fake = Fake::default();
        process("แก้บั๊กแล้วครับ", &Settings::default(), &thai(), &fake);
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_translation_reads_the_english_with_the_fallback_voice() {
        let fake = Fake { fail: true, ..Default::default() };
        let out = process("All tests pass.", &Settings::default(), &thai(), &fake);
        assert!(!out.translated && out.failed);
        assert_eq!(out.voice, "en-US-AvaMultilingualNeural");
        assert_eq!(out.chunks, vec!["All tests pass."]);
    }

    #[test]
    fn translate_off_reads_the_reply_as_is() {
        let fake = Fake::default();
        let settings = Settings { translate: false, ..Settings::default() };
        let out = process("Done.", &settings, &thai(), &fake);
        assert_eq!(out.chunks, vec!["Done."]);
        assert!(fake.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn display_keeps_the_layout_with_code_in_place() {
        let fake = Fake::default();
        let mut settings = Settings::default();
        settings.display.enabled = true;
        let out = process("- one.\n\n```\nx = 1\n```", &settings, &thai(), &fake);
        assert_eq!(out.display, "- ONE.\n\n```\nx = 1\n```");
    }
}
