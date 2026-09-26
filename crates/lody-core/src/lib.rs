//! Lody's core: where replies come from, how they are filtered, translated and read aloud.
//! No window or tray here, so it builds and tests anywhere; the app and the `lody` command
//! both drive it through `Engine`.

pub mod corrections;
pub mod engine;
pub mod filter;
pub mod handy;
pub mod locale;
#[cfg(windows)]
pub mod mute;
pub mod progress;
pub mod reply;
pub mod settings;
pub mod sources;
pub mod speaker;
pub mod translate;
pub mod voice;

/// Pick rustls's crypto backend once: two dependencies each enable a different one, and rustls
/// refuses to guess. Called before any network use.
pub fn init_tls() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A command for a program run in the background: on Windows it gets no console window, which
/// would otherwise flash up each time the app (which has no console) runs one.
pub fn background(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    }
    command
}

/// Where `name` is on the `PATH`, trying Windows' program extensions (`.exe`, `.cmd`) there.
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    let extensions: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat"] } else { &[""] };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| extensions.iter().map(move |ext| dir.join(format!("{name}{ext}"))))
        .find(|p| p.is_file())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Config(String),
    #[error("translation failed: {0}")]
    Translate(String),
    #[error("speech failed: {0}")]
    Voice(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
