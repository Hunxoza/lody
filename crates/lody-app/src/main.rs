//! Lody's desktop app: reads your programs aloud in the background, with a tray menu and a
//! settings window for Lody itself and for Handy (the app that listens).
//!
//!   lody-app            start and show the settings window
//!   lody-app --hidden   start in the background (used when starting at login)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod desktop;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lody_core::engine::Engine;
use lody_core::handy::{self, Config as HandyConfig};
use lody_core::locale::Locale;
use lody_core::reply;
use lody_core::settings::{Settings, config_dir};
use lody_core::speaker::{self, Job, Player, Speaker};
use lody_core::translate::{self, Translator, Translators};
use lody_core::voice::{self, Voices, catalog};
use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, Wry};

struct Lody {
    settings: Mutex<Settings>,
    locale: Mutex<Locale>,
    engine: Mutex<Sender<(Settings, Locale)>>,
    speaker: Speaker,
    /// The engines behind the speaker, told when their settings change.
    voices: Arc<Voices>,
    /// Edge's live voice list, fetched once in the background (empty until then or offline).
    edge_voices: Arc<Mutex<Vec<voice::EdgeVoice>>>,
    translator: Arc<dyn Translator>,
    /// The same translators, told when their settings change.
    translators: Arc<Translators>,
    player: Option<String>,
    paused: AtomicBool,
    pause_item: Mutex<Option<CheckMenuItem<Wry>>>,
    history: Arc<Mutex<VecDeque<Shown>>>,
}

/// How many replies the Read tab keeps (in memory only).
const HISTORY: usize = 50;

/// A reply in the Read tab.
#[derive(Clone, Serialize)]
struct Shown {
    id: u64,
    /// Seconds since 1970.
    at: u64,
    project: String,
    original: String,
    /// The whole reply in your language; empty when only the spoken part was translated.
    display: String,
    /// What is read aloud.
    spoken: String,
    translated: bool,
    #[serde(skip)]
    job: Job,
}

/// Stands in when neither mpv nor ffplay is installed, so the rest of Lody still works.
struct NoPlayer;

impl Player for NoPlayer {
    fn play(&self, _: &[u8], _: &dyn Fn() -> bool) -> Result<(), lody_core::Error> {
        Err(lody_core::Error::Config("no audio player: install mpv".into()))
    }
}

type Answer<T> = Result<T, String>;

fn fail(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn locales_folder() -> std::path::PathBuf {
    config_dir().join("locales")
}

// --- Lody's own settings ---

/// A program Lody can read from (`sources::PROGRAMS`).
#[derive(Serialize)]
struct ProgramView {
    id: &'static str,
    name: &'static str,
    how: &'static str,
    found: bool,
}

#[derive(Serialize)]
struct LocaleView {
    code: String,
    name: String,
    native_name: String,
    /// Its default voice, as `engine:name`.
    voice: String,
    voices: Vec<catalog::Entry>,
    rate: String,
}

#[derive(Serialize)]
struct StateView {
    settings: Settings,
    locales: Vec<LocaleView>,
    player: Option<String>,
    programs: Vec<ProgramView>,
    translators: Vec<translate::Entry>,
    paused: bool,
    history: Vec<Shown>,
    config_path: String,
    in_menu: bool,
    /// Installed from a package, which already puts Lody in the app menu.
    packaged: bool,
    at_login: bool,
}

#[tauri::command]
fn state(lody: State<Lody>) -> StateView {
    let edge = lody.edge_voices.lock().unwrap().clone();
    let locales = Locale::available(Some(&locales_folder()))
        .into_iter()
        .map(|l| LocaleView {
            voices: catalog::for_language(&l.code, &edge),
            voice: voice::full_id(&l.tts.voice),
            code: l.code,
            name: l.name,
            native_name: l.native_name,
            rate: l.tts.rate,
        })
        .collect();
    StateView {
        settings: lody.settings.lock().unwrap().clone(),
        locales,
        player: lody.player.clone(),
        programs: lody_core::sources::PROGRAMS
            .iter()
            .map(|p| ProgramView { id: p.id, name: p.name, how: p.how, found: (p.found)() })
            .collect(),
        translators: translate::catalog(),
        paused: lody.paused.load(Ordering::SeqCst),
        history: lody.history.lock().unwrap().iter().cloned().collect(),
        config_path: Settings::path().display().to_string(),
        in_menu: desktop::in_menu(),
        packaged: desktop::packaged(),
        at_login: desktop::at_login(),
    }
}

#[tauri::command]
fn save_settings(lody: State<Lody>, settings: Settings) -> Answer<()> {
    let locale = Locale::load(&settings.locale, Some(&locales_folder())).map_err(fail)?;
    settings.save_to(&Settings::path()).map_err(fail)?;
    lody.voices.configure(&settings.voices);
    lody.voices.set_fallback(Some(locale.backup_voice()));
    let old = lody.settings.lock().unwrap().clone();
    if (old.timeout, &old.translator) != (settings.timeout, &settings.translator) {
        let timeout = Duration::from_secs(settings.timeout);
        lody.translators.configure(timeout, &settings.translator);
    }
    *lody.settings.lock().unwrap() = settings.clone();
    *lody.locale.lock().unwrap() = locale.clone();
    lody.engine.lock().unwrap().send((settings, locale)).map_err(fail)?;
    Ok(())
}

/// Read a sample sentence the way a reply would be read, with the settings on screen.
#[tauri::command]
async fn test_voice(app: AppHandle, settings: Settings) -> Answer<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let lody = app.state::<Lody>();
        let locale = Locale::load(&settings.locale, Some(&locales_folder())).map_err(fail)?;
        let sample = "Hello! This is how Lody reads the replies from your AI.";
        lody.voices.configure(&settings.voices);
        let out = reply::process(sample, &settings, &locale, lody.translator.as_ref());
        let rate = settings.speech.rate_in(&locale);
        let text = out.chunks.join(" ");
        lody.speaker.stop_session("test");
        let job = Job {
            session: "test".into(),
            chunks: out.chunks,
            voice: out.voice,
            rate,
            progress: false,
        };
        lody.speaker.enqueue(job);
        Ok(text)
    })
    .await
    .map_err(fail)?
}

/// Read a reply from the Read tab aloud again.
#[tauri::command]
fn read_again(lody: State<Lody>, id: u64) -> Answer<()> {
    let job = lody
        .history
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.job.clone())
        .ok_or("that reply is no longer kept")?;
    lody.speaker.stop_all();
    lody.speaker.enqueue(Job { session: "again".into(), ..job });
    Ok(())
}

#[tauri::command]
fn stop_speaking(lody: State<Lody>) {
    lody.speaker.stop_all();
}

#[tauri::command]
fn set_paused(lody: State<Lody>, paused: bool) {
    pause(&lody, paused);
}

fn pause(lody: &Lody, paused: bool) {
    lody.paused.store(paused, Ordering::SeqCst);
    lody.speaker.set_muted(paused);
    if let Some(item) = lody.pause_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(paused);
    }
}

#[tauri::command]
fn set_in_menu(on: bool) -> Answer<()> {
    desktop::set_in_menu(on).map_err(fail)
}

#[tauri::command]
fn set_at_login(on: bool) -> Answer<()> {
    desktop::set_at_login(on).map_err(fail)
}

#[tauri::command]
fn open_link(url: String) -> Answer<()> {
    if !url.starts_with("https://") {
        return Err("only web links open".into());
    }
    desktop::open(&url).map_err(fail)
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

// --- Handy ---

#[derive(Serialize)]
struct ModelView {
    repo: String,
    name: String,
    description: String,
    size_mb: u64,
    translate: bool,
    speed: u32,
    accuracy: u32,
    downloaded: bool,
    /// The one Handy uses now.
    chosen: bool,
    /// The language's suggested model (its locale's `whisper_model`).
    suggested: bool,
}

#[derive(Serialize)]
struct HandyView {
    program: Option<String>,
    running: bool,
    config: Option<HandyConfig>,
    /// Name of the model Handy uses, when Lody knows it.
    model_name: Option<String>,
    language: String,
    language_name: String,
    models: Vec<ModelView>,
    problems: Vec<String>,
    /// Who turns what you say into English, in plain words.
    who_translates: String,
    can_install: bool,
    release: String,
    releases_url: String,
}

/// Handy's setup as it is right now: Lody only reads it; changes are made in Handy.
#[tauri::command]
fn handy_state(lody: State<Lody>) -> HandyView {
    let locale = lody.locale.lock().unwrap().clone();
    let language = locale.stt.whisper_language.clone();
    let config = handy::read_config(&handy::settings_path()).ok().flatten();
    let chosen = config.as_ref().and_then(|c| handy::model_by_id(&c.model));
    let suggested = format!("whisper-{}-gguf", locale.stt.whisper_model);
    let models = handy::models_for(&language)
        .into_iter()
        .map(|m| ModelView {
            repo: m.repo.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
            size_mb: m.size / 1_000_000,
            translate: m.translate,
            speed: m.speed,
            accuracy: m.accuracy,
            downloaded: handy::downloaded_file(m).is_some(),
            chosen: chosen.is_some_and(|c| c.repo == m.repo),
            suggested: m.repo.ends_with(&suggested),
        })
        .collect();
    let problems =
        config.as_ref().map(|c| handy::problems(c, &language, &locale.name)).unwrap_or_default();
    let who_translates = match &config {
        None => "Handy has not been started yet: open it once to set it up.".to_string(),
        Some(c) if c.translate_to_english && chosen.is_some_and(|m| m.translate) => {
            format!("Handy turns what you say in {} into English text.", locale.name)
        }
        Some(_) => {
            format!("Handy types what you say in {0}; the AI reads {0} directly.", locale.name)
        }
    };
    HandyView {
        program: handy::find_program().map(|p| p.display().to_string()),
        running: handy::running(),
        model_name: chosen.map(|m| m.name.clone()),
        config,
        language,
        language_name: locale.name.clone(),
        models,
        problems,
        who_translates,
        can_install: handy::install_command().is_some(),
        release: handy::RELEASE.into(),
        releases_url: handy::RELEASES_URL.into(),
    }
}

#[tauri::command]
async fn handy_install() -> Answer<()> {
    tauri::async_runtime::spawn_blocking(|| {
        let argv = handy::install_command().ok_or("install Handy from its download page")?;
        let out = lody_core::background(&argv[0]).args(&argv[1..]).output().map_err(fail)?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            Err(format!("installing Handy failed: {}", err.trim().lines().last().unwrap_or("")))
        }
    })
    .await
    .map_err(fail)?
}

/// Show Handy's window (starting Handy if it is not running).
#[tauri::command]
fn handy_open() -> Answer<()> {
    handy::open().map_err(fail)
}

// --- Start-up ---

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Lody", true, None::<&str>)?;
    let paused = CheckMenuItem::with_id(app, "pause", "Pause reading", true, false, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "Stop reading this reply", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Lody", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &paused, &stop, &separator, &quit])?;
    *app.state::<Lody>().pause_item.lock().unwrap() = Some(paused.clone());
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    TrayIconBuilder::with_id("lody")
        .icon(icon)
        .tooltip("Lody")
        .menu(&menu)
        .on_menu_event(move |app, event| {
            let lody = app.state::<Lody>();
            match event.id().as_ref() {
                "open" => show_window(app),
                "pause" => {
                    let now = !lody.paused.load(Ordering::SeqCst);
                    pause(&lody, now);
                    let _ = app.emit("paused", now);
                }
                "stop" => lody.speaker.stop_all(),
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,lody_core=info,lody_app=info"),
    )
    .init();
    let hidden = std::env::args().any(|a| a == "--hidden");
    let settings = Settings::load().unwrap_or_else(|e| {
        log::warn!("{e}; using the default settings");
        Settings::default()
    });
    let locale = Locale::load(&settings.locale, Some(&locales_folder()))
        .or_else(|_| Locale::load("th", None))?;
    let translators =
        Arc::new(Translators::new(Duration::from_secs(settings.timeout), &settings.translator));
    let translator: Arc<dyn Translator> = translators.clone();
    let (player, player_name): (Box<dyn Player>, _) = match speaker::find_player() {
        Some((p, name)) => (p, Some(name)),
        None => (Box::new(NoPlayer), None),
    };
    let voices = Arc::new(Voices::new(&settings.voices));
    voices.set_fallback(Some(locale.backup_voice()));
    let speaker = Speaker::new(voices.clone(), player);
    let edge_voices = Arc::new(Mutex::new(Vec::new()));
    let fetched = edge_voices.clone();
    std::thread::spawn(move || match voice::edge_voices() {
        Ok(list) => *fetched.lock().unwrap() = list,
        Err(e) => log::warn!("could not fetch Edge's voice list: {e}"),
    });
    let (engine_tx, engine_rx) = mpsc::channel::<(Settings, Locale)>();
    let history = Arc::new(Mutex::new(VecDeque::new()));

    let lody = Lody {
        settings: Mutex::new(settings.clone()),
        locale: Mutex::new(locale.clone()),
        engine: Mutex::new(engine_tx),
        speaker: speaker.clone(),
        voices,
        edge_voices,
        translator: translator.clone(),
        translators,
        player: player_name,
        paused: AtomicBool::new(false),
        pause_item: Mutex::new(None),
        history: history.clone(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_window(app)))
        .manage(lody)
        .invoke_handler(tauri::generate_handler![
            state,
            save_settings,
            test_voice,
            stop_speaking,
            read_again,
            set_paused,
            set_in_menu,
            set_at_login,
            open_link,
            quit,
            handy_state,
            handy_install,
            handy_open,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            if let Err(e) = setup_tray(&handle) {
                log::warn!("no tray icon: {e}");
            }
            // The engine watches the AI's logs on its own thread; new settings arrive by channel.
            std::thread::Builder::new().name("lody-engine".into()).spawn(move || {
                let next_id = std::sync::atomic::AtomicU64::new(1);
                let on_reply: lody_core::engine::OnReply = Arc::new(move |reply| {
                    let shown = Shown {
                        id: next_id.fetch_add(1, Ordering::SeqCst),
                        at: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs()),
                        project: reply.project.clone(),
                        original: reply.original.clone(),
                        display: reply.display.clone(),
                        spoken: reply.job.chunks.join(" "),
                        translated: reply.translated,
                        job: reply.job.clone(),
                    };
                    let mut history = history.lock().unwrap();
                    history.push_front(shown.clone());
                    history.truncate(HISTORY);
                    drop(history);
                    let _ = handle.emit("reply", shown);
                });
                let mut engine =
                    Engine::new(settings, locale, translator, speaker).on_reply(on_reply);
                loop {
                    while let Ok((settings, locale)) = engine_rx.try_recv() {
                        engine.apply(settings, locale);
                    }
                    engine.step();
                    std::thread::sleep(Duration::from_millis(300));
                }
            })?;
            if !hidden {
                show_window(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps Lody reading in the background.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
