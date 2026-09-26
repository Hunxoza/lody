//! Where replies come from: one module per program, each needing nothing installed into it.
//! To add a program, write a module with a `Source` and list it in `PROGRAMS` (see
//! CONTRIBUTING.md); the engine, the settings and the app pick it up from there.

pub mod claude_code;

/// What a program did, as Lody needs to know it. `session` tells apart conversations running
/// at the same time; `project` is a short name for where it runs, said before the reply when
/// several are active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The AI finished a reply in this session.
    Reply { session: String, project: String, text: String },
    /// The AI wrote a line while still working ("Let me check the tests.").
    Progress { session: String, project: String, text: String },
    /// The AI started a tool; `tool` is the program's own name for it ("Bash", "WebSearch") and
    /// `input` what it was given (see `progress::detail` for what of it is ever said).
    Tool { session: String, project: String, tool: String, input: serde_json::Value },
    /// You sent a new prompt: whatever this session is reading stops. `text` is what you sent
    /// (compared with what Handy heard, see `corrections`).
    Prompt { session: String, project: String, text: String },
}

/// Follows one program: called a few times a second, on the engine's own thread.
pub trait Source: Send {
    /// Everything that happened since the last call. Started fresh, a source reports only what
    /// happens from then on, never what was said before Lody started.
    fn poll(&mut self) -> Vec<Event>;
}

/// A program Lody can read from.
pub struct Program {
    /// Its key under `[sources]` in the settings.
    pub id: &'static str,
    pub name: &'static str,
    /// Where it is read from, for the settings window ("Claude Code's session logs").
    pub how: &'static str,
    /// Whether it looks used on this computer (its logs exist).
    pub found: fn() -> bool,
    pub start: fn() -> Box<dyn Source>,
}

pub const PROGRAMS: &[Program] = &[Program {
    id: "claude_code",
    name: "Claude Code",
    how: "Reads Claude Code's own session logs (~/.claude/projects); nothing is installed into it.",
    found: || claude_code::default_root().exists(),
    start: || Box::new(claude_code::ClaudeCode::new(claude_code::default_root())),
}];
