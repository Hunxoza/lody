//! Work in progress: which tool steps are worth saying, what about them is said, and how
//! several are said at once ("searched the web 3 times") so a busy turn doesn't talk over itself.

use serde_json::Value;

use crate::locale::ProgressText;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Shell,
    Read,
    Edit,
    Search,
    WebSearch,
    WebFetch,
    Agent,
    Other,
}

/// What kind of step a tool is; `None` for the program's own bookkeeping, which isn't said.
pub fn kind(tool: &str) -> Option<Kind> {
    Some(match tool {
        "Bash" | "BashOutput" | "PowerShell" => Kind::Shell,
        "Read" | "NotebookRead" => Kind::Read,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => Kind::Edit,
        "Grep" | "Glob" | "LS" => Kind::Search,
        "WebSearch" => Kind::WebSearch,
        "WebFetch" => Kind::WebFetch,
        "Agent" | "Task" => Kind::Agent,
        "TodoWrite" | "ToolSearch" | "EnterPlanMode" | "ExitPlanMode" | "KillShell"
        | "TaskStop" | "TaskOutput" | "ListAgents" | "ScheduleWakeup" => return None,
        _ => Kind::Other,
    })
}

/// What a step is about, taken from the tool's input.
#[derive(Debug, Clone, PartialEq)]
pub enum Detail {
    /// The AI's own words for it, in English: translated like its messages. A command's
    /// description ("Check the installed version"), never the command; a search's query.
    Text(String),
    /// A name said as is and never sent anywhere: a file's name (not its folder), a site.
    Name(String),
}

/// The part of a tool's input worth saying, if any. Commands, file contents, folders, full
/// web addresses and search patterns in code are never taken.
pub fn detail(tool: &str, input: &Value) -> Option<Detail> {
    let field = |key: &str| input[key].as_str().map(str::trim).filter(|s| !s.is_empty());
    let text = |s: &str| Detail::Text(first_sentence(s).chars().take(160).collect());
    match tool {
        "Bash" | "PowerShell" | "Agent" | "Task" => field("description").map(text),
        "WebSearch" => field("query").map(text),
        "WebFetch" => field("url").and_then(host).map(|h| Detail::Name(h.to_string())),
        "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => field("file_path")
            .or_else(|| field("notebook_path"))
            .and_then(|p| p.rsplit(['/', '\\']).next())
            .filter(|name| !name.is_empty())
            .map(|name| Detail::Name(name.to_string())),
        _ => None,
    }
}

/// `https://www.example.com/a/b?c` -> `example.com`.
fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#', ':']).next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then_some(host)
}

/// The first sentence of what the AI writes along the way: the rest is detail, and by the
/// time it would be read the AI has moved on.
pub fn first_sentence(text: &str) -> &str {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let end = line
        .char_indices()
        .find(|&(i, c)| {
            matches!(c, '.' | '!' | '?' | ':')
                && line[i + c.len_utf8()..].chars().next().is_none_or(char::is_whitespace)
        })
        .map_or(line.len(), |(i, c)| i + c.len_utf8());
    &line[..end]
}

/// Steps not said yet, in the order they first came, counted by kind, and the latest one.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Steps {
    counts: Vec<(Kind, usize)>,
    latest: Option<(Kind, Option<Detail>)>,
}

impl Steps {
    pub fn add(&mut self, kind: Kind, detail: Option<Detail>) {
        match self.counts.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => *n += 1,
            None => self.counts.push((kind, 1)),
        }
        self.latest = Some((kind, detail));
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// The step being done now and what it's about, when that is known: said instead of the
    /// counts, since the earlier steps are done.
    pub fn latest(&self) -> Option<(Kind, &Detail)> {
        self.latest.as_ref().and_then(|(kind, detail)| Some((*kind, detail.as_ref()?)))
    }

    /// The latest step with what it's about, `spoken` being the detail as it is read (its
    /// translation, for `Detail::Text`); `None` when the locale has no phrase for it.
    pub fn say_latest(&self, text: &ProgressText, spoken: &str) -> Option<String> {
        let (kind, detail) = self.latest()?;
        let template = match (kind, detail) {
            (Kind::Shell | Kind::Agent, Detail::Text(_)) => text.doing.as_ref()?,
            _ => text.about.as_ref()?,
        };
        Some(template.replace("{step}", text.step(kind)).replace("{detail}", spoken))
    }

    /// All of them in one line of speech.
    pub fn say(&self, text: &ProgressText) -> String {
        self.counts
            .iter()
            .map(|&(kind, n)| {
                let step = text.step(kind);
                if n == 1 {
                    text.one.replace("{step}", step)
                } else {
                    text.many.replace("{step}", step).replace("{n}", &n.to_string())
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locale::Locale;

    #[test]
    fn steps_are_counted_by_kind_and_said_in_thai() {
        let th = Locale::load("th", None).unwrap();
        let mut steps = Steps::default();
        for tool in ["WebSearch", "WebSearch", "TodoWrite", "WebFetch", "WebSearch"] {
            if let Some(kind) = kind(tool) {
                steps.add(kind, None);
            }
        }
        assert_eq!(steps.say(th.text.progress.as_ref().unwrap()), "ค้นเว็บ 3 ครั้ง กำลังเปิดหน้าเว็บ");
    }

    #[test]
    fn only_the_first_sentence_of_progress_is_kept() {
        assert_eq!(
            first_sentence("Let me check main.rs first. Then the tests."),
            "Let me check main.rs first."
        );
        assert_eq!(first_sentence("\nI timed each step:\n\n| a | b |"), "I timed each step:");
        assert_eq!(first_sentence("No full stop"), "No full stop");
    }

    #[test]
    fn what_a_step_is_about_never_includes_commands_folders_or_full_addresses() {
        let bash =
            serde_json::json!({"command": "rm -rf ~/secret", "description": "Clean the build."});
        assert_eq!(detail("Bash", &bash), Some(Detail::Text("Clean the build.".into())));
        assert_eq!(detail("Bash", &serde_json::json!({"command": "ls"})), None);
        let read = serde_json::json!({"file_path": "/home/a/private/notes.md"});
        assert_eq!(detail("Read", &read), Some(Detail::Name("notes.md".into())));
        let fetch = serde_json::json!({"url": "https://www.example.com/a?token=x"});
        assert_eq!(detail("WebFetch", &fetch), Some(Detail::Name("example.com".into())));
        assert_eq!(detail("Grep", &serde_json::json!({"pattern": "password"})), None);
    }

    #[test]
    fn unknown_tools_are_just_tools() {
        assert_eq!(kind("mcp__github__create_issue"), Some(Kind::Other));
        assert_eq!(kind("ToolSearch"), None);
    }
}
