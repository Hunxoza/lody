//! Claude Code: replies come from the session logs it already writes, so nothing is installed
//! into Claude Code. Each session is `<projects>/<project>/<session>.jsonl`, one JSON object
//! per line. The format is Claude Code's own and undocumented: anything unexpected is skipped.
//!
//! - A finished reply: `type: "assistant"` with `message.stop_reason: "end_turn"` and text
//!   blocks in `message.content` (one line per content block; thinking lines carry no text).
//! - Work in progress: the same with `stop_reason: "tool_use"`; a text block is something said
//!   along the way, a `tool_use` block names the tool started.
//! - A new prompt from you: `type: "user"` whose content is text, not a tool result.
//! - Subagents (`isSidechain`) and Claude Code's own notes (`isMeta`) are ignored.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use super::{Event, Source};

/// `~/.claude/projects`, or `$CLAUDE_CONFIG_DIR/projects`.
pub fn default_root() -> PathBuf {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"));
    base.join("projects")
}

pub fn parse_line(line: &str) -> Option<Event> {
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry["isSidechain"].as_bool() == Some(true) || entry["isMeta"].as_bool() == Some(true) {
        return None;
    }
    let session = entry["sessionId"].as_str()?.to_string();
    let project = entry["cwd"]
        .as_str()
        .and_then(|cwd| Path::new(cwd).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let message = &entry["message"];
    match entry["type"].as_str()? {
        "assistant" if message["stop_reason"].as_str() == Some("end_turn") => {
            let text: Vec<&str> = message["content"]
                .as_array()?
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .filter(|t| !t.trim().is_empty())
                .collect();
            (!text.is_empty()).then(|| Event::Reply { session, project, text: text.join("\n\n") })
        }
        "assistant" if message["stop_reason"].as_str() == Some("tool_use") => {
            let block = message["content"].as_array()?.first()?;
            match block["type"].as_str()? {
                "text" => {
                    let text = block["text"].as_str().filter(|t| !t.trim().is_empty())?;
                    Some(Event::Progress { session, project, text: text.to_string() })
                }
                "tool_use" => {
                    let tool = block["name"].as_str()?.to_string();
                    let input = block["input"].clone();
                    Some(Event::Tool { session, project, tool, input })
                }
                _ => None,
            }
        }
        // Something you wrote, not a tool's result coming back.
        "user" => {
            let typed = match &message["content"] {
                Value::String(_) => true,
                Value::Array(blocks) => blocks.iter().any(|b| b["type"] == "text"),
                _ => false,
            };
            typed.then_some(Event::Prompt { session })
        }
        _ => None,
    }
}

struct Tail {
    offset: u64,
    partial: Vec<u8>,
}

/// Follows every session log under the root: only what is written after `new` is read, plus
/// sessions that start later.
pub struct ClaudeCode {
    root: PathBuf,
    started: SystemTime,
    files: HashMap<PathBuf, Tail>,
}

impl ClaudeCode {
    pub fn new(root: PathBuf) -> ClaudeCode {
        let mut source = ClaudeCode { root, started: SystemTime::now(), files: HashMap::new() };
        for (path, len, _) in source.logs() {
            source.files.insert(path, Tail { offset: len, partial: Vec::new() });
        }
        source
    }

    /// Session logs: (path, size, modified). Subagent logs sit deeper and are left out.
    fn logs(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(projects) = std::fs::read_dir(&self.root) else { return Vec::new() };
        projects
            .flatten()
            .filter_map(|p| std::fs::read_dir(p.path()).ok())
            .flat_map(|files| files.flatten())
            .filter(|f| f.path().extension().is_some_and(|e| e == "jsonl"))
            .filter_map(|f| {
                // Not `f.metadata()`: on Windows that is the size the folder listing remembers,
                // which stays behind while Claude Code still has the log open.
                let meta = std::fs::symlink_metadata(f.path()).ok().filter(|m| m.is_file())?;
                Some((f.path(), meta.len(), meta.modified().ok()?))
            })
            .collect()
    }
}

impl Source for ClaudeCode {
    fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for (path, len, modified) in self.logs() {
            let tail = match self.files.get_mut(&path) {
                Some(tail) => tail,
                // A session that began after Lody started is read from its start.
                None if modified >= self.started => self
                    .files
                    .entry(path.clone())
                    .or_insert(Tail { offset: 0, partial: Vec::new() }),
                None => continue,
            };
            if len < tail.offset {
                *tail = Tail { offset: 0, partial: Vec::new() }; // rewritten from scratch
            }
            if len == tail.offset {
                continue;
            }
            match read_from(&path, tail.offset) {
                Ok(bytes) => {
                    tail.offset += bytes.len() as u64;
                    tail.partial.extend_from_slice(&bytes);
                    // Only whole lines: the last one may still be being written.
                    while let Some(end) = tail.partial.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = tail.partial.drain(..=end).collect();
                        if let Some(event) = parse_line(&String::from_utf8_lossy(&line)) {
                            events.push(event);
                        }
                    }
                }
                Err(e) => log::debug!("can't read {}: {e}", path.display()),
            }
        }
        events
    }
}

fn read_from(path: &Path, offset: u64) -> std::io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn entry(text: &str) -> Value {
        serde_json::json!({
            "type": "assistant", "sessionId": "s1", "cwd": "/home/a/Desktop/lody", "isSidechain": false,
            "message": {"role": "assistant", "stop_reason": "end_turn",
                        "content": [{"type": "text", "text": text}]}
        })
    }

    fn reply(text: &str) -> String {
        entry(text).to_string()
    }

    #[test]
    fn a_finished_reply_is_read_with_its_project() {
        let event = parse_line(&reply("All tests pass.")).unwrap();
        assert_eq!(
            event,
            Event::Reply {
                session: "s1".into(),
                project: "lody".into(),
                text: "All tests pass.".into()
            }
        );
    }

    #[test]
    fn work_in_progress_is_seen_as_what_was_said_and_which_tool() {
        let mut said = entry("Let me check.");
        said["message"]["stop_reason"] = "tool_use".into();
        assert_eq!(
            parse_line(&said.to_string()),
            Some(Event::Progress {
                session: "s1".into(),
                project: "lody".into(),
                text: "Let me check.".into()
            })
        );
        let mut tool = said.clone();
        tool["message"]["content"] = serde_json::json!([{"type": "tool_use", "id": "t1",
            "name": "WebSearch", "input": {"query": "secret plans"}}]);
        assert_eq!(
            parse_line(&tool.to_string()),
            Some(Event::Tool {
                session: "s1".into(),
                project: "lody".into(),
                tool: "WebSearch".into(),
                input: serde_json::json!({"query": "secret plans"}),
            })
        );
        let mut thinking = said;
        thinking["message"]["content"] = serde_json::json!([{"type": "thinking", "thinking": "x"}]);
        assert_eq!(parse_line(&thinking.to_string()), None);
    }

    #[test]
    fn thinking_subagents_and_notes_are_skipped() {
        let mut thinking = entry("x");
        thinking["message"]["content"] = serde_json::json!([{"type": "thinking", "thinking": "x"}]);
        assert_eq!(parse_line(&thinking.to_string()), None);
        let mut subagent = entry("x");
        subagent["isSidechain"] = true.into();
        assert_eq!(parse_line(&subagent.to_string()), None);
        let tool_result = r#"{"type":"user","sessionId":"s1","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#;
        assert_eq!(parse_line(tool_result), None);
        let meta = r#"{"type":"user","sessionId":"s1","isMeta":true,"message":{"content":"note"}}"#;
        assert_eq!(parse_line(meta), None);
        assert_eq!(parse_line(r#"{"type":"mode","sessionId":"s1"}"#), None);
        assert_eq!(parse_line("not json"), None);
    }

    #[test]
    fn a_new_prompt_is_seen() {
        let prompt =
            r#"{"type":"user","sessionId":"s1","cwd":"/p/x","message":{"content":"fix it"}}"#;
        assert_eq!(parse_line(prompt), Some(Event::Prompt { session: "s1".into() }));
    }

    #[test]
    fn only_new_lines_are_read_and_half_written_lines_wait() {
        let root = std::env::temp_dir().join(format!("lody-cc-{}", std::process::id()));
        let project = root.join("-home-a-lody");
        std::fs::create_dir_all(&project).unwrap();
        let log = project.join("s1.jsonl");
        std::fs::write(&log, format!("{}\n", reply("old"))).unwrap();

        let mut source = ClaudeCode::new(root.clone());
        assert!(source.poll().is_empty()); // history is not read aloud

        let line = reply("new");
        let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        file.write_all(&line.as_bytes()[..10]).unwrap();
        assert!(source.poll().is_empty());
        file.write_all(format!("{}\n", &line[10..]).as_bytes()).unwrap();
        let events = source.poll();
        assert!(matches!(&events[..], [Event::Reply { text, .. }] if text == "new"));

        let later = project.join("s2.jsonl"); // a session started after Lody
        std::fs::write(&later, format!("{}\n", reply("hello"))).unwrap();
        assert_eq!(source.poll().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
