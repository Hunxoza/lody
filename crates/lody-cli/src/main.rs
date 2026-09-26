//! `lody`: run Lody without its window, for trying things out.
//!
//!   lody watch          read Claude Code's replies aloud until Ctrl+C
//!   lody say TEXT       read TEXT aloud (`-t`: translate it from English first)

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use lody_core::engine::Engine;
use lody_core::locale::Locale;
use lody_core::reply;
use lody_core::settings::{Settings, config_dir};
use lody_core::speaker::{self, Job, Speaker};
use lody_core::translate::{Translator, Translators};
use lody_core::voice::Voices;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,lody_core=info"),
    )
    .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let settings = Settings::load().context("reading settings")?;
    let locale = Locale::load(&settings.locale, Some(&config_dir().join("locales")))?;
    let translators =
        Arc::new(Translators::new(Duration::from_secs(settings.timeout), &settings.translator));
    let translator: Arc<dyn Translator> = translators;
    let (player, _) =
        speaker::find_player().context("no audio player found: install mpv or ffmpeg")?;
    let voices = Voices::new(&settings.voices);
    voices.set_fallback(Some(locale.backup_voice()));
    let speaker = Speaker::new(Arc::new(voices), player);

    match args.first().map(String::as_str) {
        Some("watch") => {
            println!("Reading Claude Code replies aloud in {}. Ctrl+C to stop.", locale.name);
            Engine::new(settings, locale, translator, speaker).run(Duration::from_millis(300))
        }
        Some("say") => {
            let translate = args.get(1).is_some_and(|a| a == "-t");
            let text = args[if translate { 2 } else { 1 }..].join(" ");
            if text.trim().is_empty() {
                bail!("nothing to say");
            }
            let settings = Settings { translate, ..settings };
            let out = reply::process(&text, &settings, &locale, translator.as_ref());
            println!("{}", out.chunks.join(" | "));
            speaker.enqueue(Job {
                session: "say".into(),
                chunks: out.chunks,
                voice: out.voice,
                rate: settings.speech.rate_in(&locale),
                progress: false,
            });
            while speaker.busy() {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(())
        }
        _ => {
            eprintln!("usage: lody watch | lody say [-t] TEXT");
            std::process::exit(2);
        }
    }
}
