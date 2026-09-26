//! Watch the sources, turn each reply into speech, and stop a session's speech when you send
//! it a new prompt. While the AI works, its tool steps are counted and said in your language
//! when the speaker is free ("searched the web 3 times"); only the reply is translated.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::locale::Locale;
use crate::progress::{self, Detail, Steps};
use crate::reply;
use crate::settings::{Announce, Progress, Settings};
use crate::sources::{About, Event, PROGRAMS, Source};
use crate::speaker::{Job, Speaker};
use crate::translate::Translator;

/// A session counts as active this long after its last reply or prompt.
const ACTIVE_FOR: Duration = Duration::from_secs(10 * 60);

/// Told about each reply once it is translated, whether or not it is read aloud.
pub type OnReply = Arc<dyn Fn(&Reply) + Send + Sync>;

/// Where a reply comes from, shown with it: "Claude Code · terminal · shop (main)", and the
/// conversation's title.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Origin {
    /// The program's name, from `sources::PROGRAMS`.
    pub program: String,
    pub project: String,
    pub session: String,
    #[serde(flatten)]
    pub about: About,
}

/// A reply as Lody shows it: the original, its translation, and what is (or would be) read.
#[derive(Debug, Clone)]
pub struct Reply {
    pub origin: Origin,
    pub original: String,
    /// The whole reply in your language, code kept in place (empty with `display.enabled` off
    /// or when it was not translated).
    pub display: String,
    pub translated: bool,
    /// What is read aloud, ready to be queued again.
    pub job: Job,
}

pub struct Engine {
    settings: Settings,
    locale: Locale,
    translator: Arc<dyn Translator>,
    speaker: Speaker,
    /// The programs being read, by id.
    sources: BTreeMap<&'static str, Box<dyn Source>>,
    last_seen: HashMap<String, Instant>,
    /// Bumped by each prompt: a reply still being translated when you speak again is dropped.
    generation: Arc<Mutex<HashMap<String, u64>>>,
    /// Tool steps not said yet, per session, with its project.
    steps: HashMap<String, (String, Steps)>,
    /// When progress was last said, per session.
    last_progress: HashMap<String, Instant>,
    on_reply: Option<OnReply>,
}

impl Engine {
    pub fn new(
        settings: Settings,
        locale: Locale,
        translator: Arc<dyn Translator>,
        speaker: Speaker,
    ) -> Engine {
        let mut engine = Engine {
            settings,
            locale,
            translator,
            speaker,
            sources: BTreeMap::new(),
            last_seen: HashMap::new(),
            generation: Arc::default(),
            steps: HashMap::new(),
            last_progress: HashMap::new(),
            on_reply: None,
        };
        engine.start_sources();
        engine
    }

    pub fn on_reply(mut self, f: OnReply) -> Engine {
        self.on_reply = Some(f);
        self
    }

    /// Use changed settings from the next reply on. A program switched on is read from now on.
    pub fn apply(&mut self, settings: Settings, locale: Locale) {
        self.settings = settings;
        self.locale = locale;
        self.start_sources();
    }

    /// Start reading the programs switched on and not read yet; stop those switched off.
    fn start_sources(&mut self) {
        for program in PROGRAMS {
            if !self.settings.sources.is_on(program.id) {
                self.sources.remove(program.id);
            } else if !self.sources.contains_key(program.id) {
                self.sources.insert(program.id, (program.start)());
            }
        }
    }

    /// Read what happened in the sources since the last call and act on it.
    pub fn step(&mut self) {
        // Only ask the sound server while something is being read.
        let hold =
            self.settings.speech.wait_for_handy && self.speaker.busy() && crate::handy::recording();
        self.speaker.set_held(hold);
        self.speaker.set_mute_others(self.settings.speech.mute_others);
        let events: Vec<(&'static str, Event)> = self
            .sources
            .iter_mut()
            .flat_map(|(id, source)| source.poll().into_iter().map(move |e| (*id, e)))
            .collect();
        for (program, event) in events {
            self.handle_from(program, event);
        }
        self.say_steps();
    }

    pub fn run(mut self, every: Duration) -> ! {
        loop {
            self.step();
            std::thread::sleep(every);
        }
    }

    /// Act on an event, as if from an unnamed program (what tests and `lody say` need).
    pub fn handle(&mut self, event: Event) {
        self.handle_from("", event);
    }

    /// Act on an event from the program `program` (its id in `PROGRAMS`).
    fn handle_from(&mut self, program: &str, event: Event) {
        match event {
            Event::Prompt { session } => {
                self.last_seen.insert(session.clone(), Instant::now());
                *self.generation.lock().unwrap().entry(session.clone()).or_default() += 1;
                self.next_turn(&session);
                self.speaker.stop_session(&session);
            }
            Event::Reply { session, project, text } => {
                self.last_seen.insert(session.clone(), Instant::now());
                self.next_turn(&session);
                let announce = self.announce();
                let voice_rate = self.settings.speech.rate_in(&self.locale);
                let (settings, locale, translator) =
                    (self.settings.clone(), self.locale.clone(), self.translator.clone());
                let (speaker, generation) = (self.speaker.clone(), self.generation.clone());
                let on_reply = self.on_reply.clone();
                let origin = Origin {
                    program: PROGRAMS
                        .iter()
                        .find(|p| p.id == program)
                        .map(|p| p.name.to_string())
                        .unwrap_or_default(),
                    project: project.clone(),
                    session: session.clone(),
                    about: self.sources.get(program).map(|s| s.about(&session)).unwrap_or_default(),
                };
                let started = generation.lock().unwrap().get(&session).copied().unwrap_or(0);
                // Translating takes a network round trip: off the watching thread.
                std::thread::spawn(move || {
                    let out = reply::process(&text, &settings, &locale, translator.as_ref());
                    // Nothing to read and no translation (speech off, translator down): still
                    // shown, as written, in the Read tab and the overlay.
                    if text.trim().is_empty() {
                        return;
                    }
                    let mut chunks = out.chunks;
                    if announce && !project.is_empty() {
                        chunks.insert(0, project.clone());
                    }
                    let job = Job {
                        session: session.clone(),
                        chunks,
                        voice: out.voice,
                        rate: voice_rate,
                        progress: false,
                    };
                    if let Some(f) = on_reply {
                        f(&Reply {
                            origin,
                            original: text,
                            display: out.display,
                            translated: out.translated,
                            job: job.clone(),
                        });
                    }
                    if !settings.speech.enabled || job.chunks.is_empty() {
                        return;
                    }
                    if generation.lock().unwrap().get(&session).copied().unwrap_or(0) != started {
                        return; // you sent a new prompt meanwhile
                    }
                    if out.failed
                        && let Some(notice) = locale.text.untranslated.clone()
                    {
                        // In your language's voice, before the English.
                        speaker.enqueue(Job {
                            session: session.clone(),
                            chunks: vec![notice],
                            voice: settings.speech.voice_in(&locale),
                            rate: job.rate.clone(),
                            progress: false,
                        });
                    }
                    let translated = if out.translated { "translated" } else { "as written" };
                    log::info!(
                        "reading a reply from {project} ({translated}): {}",
                        job.chunks.join(" ")
                    );
                    speaker.enqueue(job);
                });
            }
            // What it writes along the way is not read: translating it too sent Google so
            // many requests that it refused them. Only the reply is translated.
            Event::Progress { .. } => {}
            Event::Tool { session, project, tool, input } => {
                if !self.settings.speech.enabled || self.settings.speech.progress != Progress::All {
                    return;
                }
                self.last_seen.insert(session.clone(), Instant::now());
                if let Some(kind) = progress::kind(&tool) {
                    self.steps
                        .entry(session)
                        .or_insert_with(|| (project, Steps::default()))
                        .1
                        .add(kind, progress::detail(&tool, &input));
                }
            }
        }
    }

    /// A new prompt or the reply: progress not said yet is out of date.
    fn next_turn(&mut self, session: &str) {
        self.steps.remove(session);
        self.last_progress.remove(session);
    }

    /// Say what one session is doing, if the speaker is free and it's been a while: the latest
    /// step with what it's about when that's a name ("searching the web for ..."), otherwise
    /// the steps counted since last time.
    fn say_steps(&mut self) {
        if self.steps.is_empty() || self.speaker.busy() {
            return;
        }
        let Some(text) = self.locale.text.progress.clone() else {
            self.steps.clear();
            return;
        };
        let every = Duration::from_secs(self.settings.speech.progress_every);
        let due = self
            .steps
            .keys()
            .find(|session| self.last_progress.get(*session).is_none_or(|t| t.elapsed() >= every));
        let Some(session) = due.cloned() else { return };
        let (project, steps) = self.steps.remove(&session).unwrap();
        self.last_progress.insert(session.clone(), Instant::now());
        let announce = self.announce();
        let about = match steps.latest() {
            Some((_, Detail::Name(name))) => steps.say_latest(&text, name),
            // A command's description is English and isn't translated: the counts then.
            Some((_, Detail::Text(_))) | None => None,
        };
        let mut chunks = vec![about.unwrap_or_else(|| steps.say(&text))];
        if announce && !project.is_empty() {
            chunks.insert(0, project.clone());
        }
        log::info!("saying what {project} is doing: {}", chunks.join(" "));
        let (voice, rate) = (
            self.settings.speech.voice_in(&self.locale),
            self.settings.speech.rate_in(&self.locale),
        );
        self.speaker.enqueue(Job { session, chunks, voice, rate, progress: true });
    }

    fn announce(&self) -> bool {
        match self.settings.speech.announce_project {
            Announce::Always => true,
            Announce::Never => false,
            Announce::Auto => self.active_sessions() > 1,
        }
    }

    fn active_sessions(&self) -> usize {
        self.last_seen.values().filter(|t| t.elapsed() < ACTIVE_FOR).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speaker::Player;
    use crate::translate::tests::Fake;
    use crate::voice::Voice;

    struct Echo;
    impl Voice for Echo {
        fn synthesize(&self, text: &str, _: &str, _: &str) -> Result<Vec<u8>, crate::Error> {
            Ok(text.as_bytes().to_vec())
        }
    }

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<String>>>);
    impl Player for Recorder {
        fn play(&self, audio: &[u8], _: &dyn Fn() -> bool) -> Result<(), crate::Error> {
            self.0.lock().unwrap().push(String::from_utf8_lossy(audio).into_owned());
            Ok(())
        }
    }

    fn engine() -> (Engine, Recorder) {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        let mut settings = Settings::default();
        settings.sources.set("claude_code", false);
        let locale = Locale::load("th", None).unwrap();
        (Engine::new(settings, locale, Arc::new(Fake::default()), speaker), played)
    }

    fn tool(name: &str) -> Event {
        tool_with(name, serde_json::json!({}))
    }

    fn tool_with(name: &str, input: serde_json::Value) -> Event {
        Event::Tool { session: "s".into(), project: "lody".into(), tool: name.into(), input }
    }

    /// Wait until `n` things were played (or a while, when fewer are expected).
    fn played(recorder: &Recorder, n: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(if n == 0 { 0 } else { 5 });
        while recorder.0.lock().unwrap().len() < n && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(100)); // nothing more should come
        recorder.0.lock().unwrap().clone()
    }

    #[test]
    fn tool_steps_are_counted_said_at_most_so_often_and_dropped_by_the_reply() {
        let (mut engine, recorder) = engine();
        for name in ["WebSearch", "WebSearch", "WebSearch"] {
            engine.handle(tool(name));
        }
        engine.say_steps();
        assert_eq!(played(&recorder, 1), vec!["ค้นเว็บ 3 ครั้ง"]);

        engine.handle(tool("Read"));
        engine.say_steps(); // too soon after the last one: counted, not said
        assert_eq!(played(&recorder, 0).len(), 1);

        let reply =
            Event::Reply { session: "s".into(), project: "lody".into(), text: "Done.".into() };
        engine.handle(reply);
        assert!(engine.steps.is_empty(), "the reply makes pending steps out of date");
        assert_eq!(played(&recorder, 2).last().unwrap(), "DONE.");
    }

    #[test]
    fn a_command_description_is_not_translated_the_steps_are_counted() {
        let (mut engine, recorder) = engine();
        engine.handle(tool("Read"));
        let bash =
            serde_json::json!({"command": "rpm -q lody", "description": "Check the version"});
        engine.handle(tool_with("Bash", bash));
        engine.say_steps();
        assert_eq!(played(&recorder, 1), vec!["กำลังอ่านไฟล์ กำลังรันคำสั่ง"]);
    }

    #[test]
    fn what_it_writes_along_the_way_is_not_read_or_translated() {
        let translator = Arc::new(Fake::default());
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        let mut settings = Settings::default();
        settings.sources.set("claude_code", false);
        let locale = Locale::load("th", None).unwrap();
        let mut engine = Engine::new(settings, locale, translator.clone(), speaker);
        engine.handle(Event::Progress {
            session: "s".into(),
            project: "lody".into(),
            text: "Let me check.".into(),
        });
        assert!(self::played(&played, 0).is_empty());
        assert!(translator.calls.lock().unwrap().is_empty(), "nothing sent to the translator");
    }
}
