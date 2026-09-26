//! The speaking queue: one reply at a time across all sessions, never on top of each other.
//! The next chunk is synthesized while the current one plays; a chunk the voice fails on is
//! skipped, not the rest of the reply. A new prompt stops its session's reply. While held (you
//! are recording with Handy) nothing plays; the chunk that was cut off is read again after.
//! Progress (what the AI says while it works) gives way: a newer progress line or the reply
//! drops the session's progress still waiting, and progress that waited too long is skipped:
//! by then the AI is doing something else. When asked, the other programs are muted from the
//! first sentence played until nothing has been read for a moment.

use std::collections::VecDeque;
use std::io::Write;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::Error;
use crate::voice::Voice;

pub trait Player: Send + Sync {
    /// Play audio (MP3, WAV or AIFF); return early once `stop()` turns true.
    fn play(&self, audio: &[u8], stop: &dyn Fn() -> bool) -> Result<(), Error>;

    /// Mute (true) the other programs' sound, or give it back (false); only where it can.
    fn mute_others(&self, _mute: bool) {}
}

/// Unmute the other programs once nothing has been read for this long, so they do not come
/// back between a reply and the next one already waiting to be translated.
const UNMUTE_AFTER: Duration = Duration::from_millis(1500);

/// Wait this long after a hold ends before speaking again: Handy unmutes the sound a moment
/// after it stops recording.
const AFTER_HOLD: Duration = Duration::from_millis(500);

/// Progress that waited longer than this for its turn is out of date and skipped.
const PROGRESS_STALE: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub session: String,
    pub chunks: Vec<String>,
    pub voice: String,
    pub rate: String,
    /// Said while the AI works, not its reply.
    pub progress: bool,
}

#[derive(Default)]
struct State {
    /// Each with when it was queued.
    jobs: VecDeque<(Job, Instant)>,
    current: Option<String>,
    stop: Arc<AtomicBool>,
    muted: bool,
}

struct Inner {
    state: Mutex<State>,
    wake: Condvar,
    held: AtomicBool,
    mute_others: AtomicBool,
    voice: Arc<dyn Voice>,
    player: Box<dyn Player>,
}

#[derive(Clone)]
pub struct Speaker {
    inner: Arc<Inner>,
}

impl Speaker {
    /// Starts the speaking thread; it runs for the life of the program.
    pub fn new(voice: Arc<dyn Voice>, player: Box<dyn Player>) -> Speaker {
        let inner = Arc::new(Inner {
            state: Mutex::default(),
            wake: Condvar::new(),
            held: AtomicBool::new(false),
            mute_others: AtomicBool::new(false),
            voice,
            player,
        });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("lody-speaker".into())
            .spawn(move || run(&worker))
            .expect("start the speaking thread");
        Speaker { inner }
    }

    pub fn enqueue(&self, job: Job) {
        let mut state = self.inner.state.lock().unwrap();
        if state.muted || job.chunks.is_empty() {
            return;
        }
        state.jobs.retain(|(j, _)| !(j.progress && j.session == job.session));
        state.jobs.push_back((job, Instant::now()));
        self.inner.wake.notify_one();
    }

    /// Drop this session's queued replies and stop the one being read, if it is theirs.
    pub fn stop_session(&self, session: &str) {
        let mut state = self.inner.state.lock().unwrap();
        state.jobs.retain(|(j, _)| j.session != session);
        if state.current.as_deref() == Some(session) {
            state.stop.store(true, Ordering::SeqCst);
        }
    }

    pub fn stop_all(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.jobs.clear();
        state.stop.store(true, Ordering::SeqCst);
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.state.lock().unwrap().muted = muted;
        if muted {
            self.stop_all();
        }
    }

    /// Hold speech (true) while you are recording; the cut-off chunk is read again on release.
    pub fn set_held(&self, held: bool) {
        self.inner.held.store(held, Ordering::SeqCst);
    }

    /// Mute the other programs while reading (from the next sentence on).
    pub fn set_mute_others(&self, mute: bool) {
        self.inner.mute_others.store(mute, Ordering::SeqCst);
    }

    /// Whether anything is being read or waiting to be.
    pub fn busy(&self) -> bool {
        let state = self.inner.state.lock().unwrap();
        state.current.is_some() || !state.jobs.is_empty()
    }
}

fn run(inner: &Inner) {
    // Whether the other programs are muted now.
    let mut muted = false;
    loop {
        let next = {
            let mut state = inner.state.lock().unwrap();
            if state.jobs.is_empty() {
                state = if muted {
                    inner.wake.wait_timeout(state, UNMUTE_AFTER).unwrap().0
                } else {
                    inner.wake.wait(state).unwrap()
                };
            }
            match state.jobs.pop_front() {
                None => None,
                Some((job, queued)) if job.progress && queued.elapsed() > PROGRESS_STALE => {
                    log::debug!("skipped progress that waited too long");
                    continue;
                }
                Some((job, _)) => {
                    state.stop = Arc::new(AtomicBool::new(false));
                    state.current = Some(job.session.clone());
                    Some((job, state.stop.clone()))
                }
            }
        };
        let Some((job, stop)) = next else {
            // Nothing more to read: give the other programs their sound back.
            if muted {
                inner.player.mute_others(false);
                muted = false;
            }
            continue;
        };
        speak(inner, &job, &stop, &mut muted);
        inner.state.lock().unwrap().current = None;
    }
}

fn speak(inner: &Inner, job: &Job, stop: &AtomicBool, muted: &mut bool) {
    let synth = |text: String| {
        let (voice, name, rate) = (inner.voice.clone(), job.voice.clone(), job.rate.clone());
        std::thread::spawn(move || voice.synthesize(&text, &name, &rate))
    };
    let mut next = job.chunks.first().map(|c| synth(c.clone()));
    for i in 0..job.chunks.len() {
        let audio =
            next.take().map(|h| h.join().unwrap_or_else(|_| Err(Error::Voice("crashed".into()))));
        next = job.chunks.get(i + 1).map(|c| synth(c.clone()));
        match audio {
            Some(Ok(audio)) => loop {
                if !wait_while_held(inner, stop) {
                    return;
                }
                if !*muted && inner.mute_others.load(Ordering::SeqCst) {
                    inner.player.mute_others(true);
                    *muted = true;
                }
                let cut = || stop.load(Ordering::SeqCst) || inner.held.load(Ordering::SeqCst);
                if let Err(e) = inner.player.play(&audio, &cut) {
                    log::warn!("could not play speech: {e}");
                    return;
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                if !inner.held.load(Ordering::SeqCst) {
                    break;
                }
                // Cut off by a hold: read this chunk again once it ends.
            },
            Some(Err(e)) => log::warn!("skipped a sentence the voice could not read: {e}"),
            None => {}
        }
        if stop.load(Ordering::SeqCst) {
            return;
        }
    }
}

/// Wait until the hold ends (false: the reply was stopped meanwhile).
fn wait_while_held(inner: &Inner, stop: &AtomicBool) -> bool {
    if !inner.held.load(Ordering::SeqCst) {
        return true;
    }
    while inner.held.load(Ordering::SeqCst) {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(AFTER_HOLD);
    !stop.load(Ordering::SeqCst)
}

/// How this computer plays audio, and its name for the settings window: Windows' own sound
/// output there, elsewhere an installed program (mpv or ffplay).
pub fn find_player() -> Option<(Box<dyn Player>, String)> {
    #[cfg(windows)]
    {
        crate::mute::recover();
        Some((Box::new(SoundPlayer), "Windows".into()))
    }
    #[cfg(not(windows))]
    {
        CommandPlayer::find().map(|p| (Box::new(p) as Box<dyn Player>, "mpv or ffplay".into()))
    }
}

/// Plays through Windows' own sound output, where mpv is rarely installed.
#[cfg(windows)]
pub struct SoundPlayer;

#[cfg(windows)]
impl Player for SoundPlayer {
    fn play(&self, audio: &[u8], stop: &dyn Fn() -> bool) -> Result<(), Error> {
        let fail = |e: String| Error::Voice(format!("playing audio: {e}"));
        let mut device =
            rodio::DeviceSinkBuilder::open_default_sink().map_err(|e| fail(e.to_string()))?;
        device.log_on_drop(false);
        let track = rodio::Player::connect_new(device.mixer());
        let source = rodio::Decoder::new(std::io::Cursor::new(audio.to_vec()))
            .map_err(|e| fail(e.to_string()))?;
        track.append(source);
        while !track.empty() {
            if stop() {
                track.stop();
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn mute_others(&self, mute: bool) {
        crate::mute::others(mute);
    }
}

/// Plays through an installed program (mpv or ffplay), for builds without their own audio.
pub struct CommandPlayer {
    argv: Vec<String>,
}

impl CommandPlayer {
    pub fn find() -> Option<CommandPlayer> {
        let candidates: [&[&str]; 2] = [
            &["mpv", "--no-video", "--really-quiet"],
            &["ffplay", "-nodisp", "-autoexit", "-loglevel", "quiet"],
        ];
        candidates
            .iter()
            .find(|argv| crate::which(argv[0]).is_some())
            .map(|argv| CommandPlayer { argv: argv.iter().map(|s| s.to_string()).collect() })
    }
}

impl Player for CommandPlayer {
    fn play(&self, audio: &[u8], stop: &dyn Fn() -> bool) -> Result<(), Error> {
        let extension = match audio {
            [b'R', b'I', b'F', b'F', ..] => "wav",
            [b'F', b'O', b'R', b'M', ..] => "aiff",
            _ => "mp3",
        };
        let name = format!("lody-{}-{:x}.{extension}", std::process::id(), rand_id());
        let path = std::env::temp_dir().join(name);
        std::fs::File::create(&path)?.write_all(audio)?;
        let mut child = crate::background(&self.argv[0])
            .args(&self.argv[1..])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let result = loop {
            if stop() {
                let _ = child.kill();
                let _ = child.wait();
                break Ok(());
            }
            match child.try_wait() {
                Ok(Some(_)) => break Ok(()),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => break Err(e.into()),
            }
        };
        let _ = std::fs::remove_file(path);
        result
    }
}

fn rand_id() -> u64 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(std::time::SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    struct Echo;
    impl Voice for Echo {
        fn synthesize(&self, text: &str, _: &str, _: &str) -> Result<Vec<u8>, Error> {
            if text == "bad" {
                return Err(Error::Voice("no".into()));
            }
            Ok(text.as_bytes().to_vec())
        }
    }

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<String>>>);
    impl Player for Recorder {
        fn play(&self, audio: &[u8], stop: &dyn Fn() -> bool) -> Result<(), Error> {
            for _ in 0..10 {
                if stop() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            self.0.lock().unwrap().push(String::from_utf8_lossy(audio).into_owned());
            Ok(())
        }

        fn mute_others(&self, mute: bool) {
            self.0.lock().unwrap().push(if mute { "(mute)" } else { "(unmute)" }.into());
        }
    }

    fn job(session: &str, chunks: &[&str]) -> Job {
        Job {
            session: session.into(),
            chunks: chunks.iter().map(|c| c.to_string()).collect(),
            voice: "v".into(),
            rate: String::new(),
            progress: false,
        }
    }

    fn wait_idle(speaker: &Speaker) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while speaker.busy() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn replies_play_in_order_and_a_bad_sentence_is_skipped() {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        speaker.enqueue(job("a", &["one", "bad", "two"]));
        speaker.enqueue(job("b", &["three"]));
        std::thread::sleep(Duration::from_millis(20));
        wait_idle(&speaker);
        assert_eq!(*played.0.lock().unwrap(), vec!["one", "two", "three"]);
    }

    #[test]
    fn a_new_prompt_stops_only_its_own_session() {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        speaker.enqueue(job("a", &["a1", "a2", "a3"]));
        speaker.enqueue(job("b", &["b1"]));
        speaker.enqueue(job("a", &["a-later"]));
        std::thread::sleep(Duration::from_millis(10));
        speaker.stop_session("a");
        wait_idle(&speaker);
        let played = played.0.lock().unwrap();
        assert!(played.contains(&"b1".to_string()));
        assert!(!played.contains(&"a-later".to_string()) && !played.contains(&"a3".to_string()));
    }

    #[test]
    fn a_hold_cuts_the_sentence_and_reads_it_again_after() {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        speaker.enqueue(job("a", &["one", "two"]));
        std::thread::sleep(Duration::from_millis(20)); // "one" is playing (50 ms)
        speaker.set_held(true);
        std::thread::sleep(Duration::from_millis(100));
        assert!(played.0.lock().unwrap().is_empty(), "nothing plays while held");
        speaker.set_held(false);
        wait_idle(&speaker);
        assert_eq!(*played.0.lock().unwrap(), vec!["one", "two"]);
    }

    #[test]
    fn the_reply_drops_progress_still_waiting_but_not_other_sessions() {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        let progress = |session: &str, text: &str| Job { progress: true, ..job(session, &[text]) };
        speaker.enqueue(job("b", &["b-reply"]));
        speaker.enqueue(progress("a", "a-step1"));
        speaker.enqueue(progress("b", "b-step"));
        speaker.enqueue(progress("a", "a-step2")); // replaces a-step1
        speaker.enqueue(job("a", &["a-reply"])); // drops a-step2
        wait_idle(&speaker);
        assert_eq!(*played.0.lock().unwrap(), vec!["b-reply", "b-step", "a-reply"]);
    }

    #[test]
    fn progress_that_waited_too_long_is_skipped() {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        speaker.set_held(true); // nothing plays: "reply" waits, so does the progress behind it
        speaker.enqueue(job("a", &["reply"]));
        std::thread::sleep(Duration::from_millis(20));
        speaker.enqueue(Job { progress: true, ..job("b", &["old step"]) });
        speaker.inner.state.lock().unwrap().jobs[0].1 -= PROGRESS_STALE; // as if long ago
        speaker.set_held(false);
        wait_idle(&speaker);
        assert_eq!(*played.0.lock().unwrap(), vec!["reply"]);
    }

    #[test]
    fn other_programs_are_muted_across_replies_and_unmuted_after() {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        speaker.set_mute_others(true);
        speaker.enqueue(job("a", &["one"]));
        wait_idle(&speaker);
        speaker.enqueue(job("b", &["two"])); // soon after: still muted
        wait_idle(&speaker);
        std::thread::sleep(UNMUTE_AFTER + Duration::from_millis(200));
        assert_eq!(*played.0.lock().unwrap(), vec!["(mute)", "one", "two", "(unmute)"]);
    }

    #[test]
    fn muted_replies_are_not_queued() {
        let played = Recorder::default();
        let speaker = Speaker::new(Arc::new(Echo), Box::new(played.clone()));
        speaker.set_muted(true);
        speaker.enqueue(job("a", &["x"]));
        assert!(!speaker.busy());
    }
}
