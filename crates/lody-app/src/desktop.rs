//! Fitting into the desktop: an entry in the app menu, starting at login, opening links.
//! Linux (freedesktop) and Windows; on macOS these report "off" and do nothing.

use std::path::PathBuf;

const ICON: &[u8] = include_bytes!("../icons/icon.png");

fn menu_entry() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("applications/lody.desktop"))
}

#[cfg(not(windows))]
fn login_entry() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("autostart/lody.desktop"))
}

fn icon_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("icons/hicolor/512x512/apps/lody.png"))
}

fn entry(hidden: bool) -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    let flag = if hidden { " --hidden" } else { "" };
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=Lody\n\
         Comment=Let your programs talk to you, in your own language\n\
         Exec=\"{}\"{flag}\nIcon=lody\nTerminal=false\nCategories=Utility;Accessibility;\n",
        exe.display()
    ))
}

fn write(path: Option<PathBuf>, text: &str) -> std::io::Result<()> {
    let path = path.ok_or_else(|| std::io::Error::other("no home folder"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, text)
}

fn remove(path: Option<PathBuf>) -> std::io::Result<()> {
    match path {
        Some(p) if p.exists() => std::fs::remove_file(p),
        _ => Ok(()),
    }
}

/// Where Windows' installers (.exe, .msi) put Lody in the Start menu.
#[cfg(windows)]
fn start_menu_entries() -> Vec<PathBuf> {
    let programs =
        |base: Option<PathBuf>| base.map(|b| b.join(r"Microsoft\Windows\Start Menu\Programs"));
    [programs(dirs::data_dir()), programs(std::env::var_os("ProgramData").map(PathBuf::from))]
        .into_iter()
        .flatten()
        .flat_map(|dir| [dir.join("Lody.lnk"), dir.join(r"Lody\Lody.lnk")])
        .collect()
}

/// Windows starts what this registry value names at login.
#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Installed from a package (.rpm, .deb, or Windows' installer), which puts Lody in the app
/// menu itself.
pub fn packaged() -> bool {
    #[cfg(windows)]
    return start_menu_entries().iter().any(|p| p.exists());
    #[cfg(not(windows))]
    ["/usr/share/applications/Lody.desktop", "/usr/share/applications/lody-app.desktop"]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
}

pub fn in_menu() -> bool {
    if cfg!(windows) {
        return packaged();
    }
    cfg!(target_os = "linux") && (packaged() || menu_entry().is_some_and(|p| p.exists()))
}

pub fn at_login() -> bool {
    #[cfg(windows)]
    return lody_core::background("reg")
        .args(["query", RUN_KEY, "/v", "Lody"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    #[cfg(not(windows))]
    return cfg!(target_os = "linux") && login_entry().is_some_and(|p| p.exists());
}

pub fn set_in_menu(on: bool) -> std::io::Result<()> {
    if cfg!(windows) {
        return Err(std::io::Error::other("install Lody to add it to the Start menu"));
    }
    if !cfg!(target_os = "linux") {
        return Err(std::io::Error::other("only on Linux for now"));
    }
    if on {
        let icon = icon_path().ok_or_else(|| std::io::Error::other("no home folder"))?;
        std::fs::create_dir_all(icon.parent().unwrap())?;
        std::fs::write(icon, ICON)?;
        write(menu_entry(), &entry(false)?)
    } else {
        remove(menu_entry())
    }
}

#[cfg(not(windows))]
pub fn set_at_login(on: bool) -> std::io::Result<()> {
    if !cfg!(target_os = "linux") {
        return Err(std::io::Error::other("only on Linux for now"));
    }
    if on { write(login_entry(), &entry(true)?) } else { remove(login_entry()) }
}

#[cfg(windows)]
pub fn set_at_login(on: bool) -> std::io::Result<()> {
    let mut command = lody_core::background("reg");
    if on {
        let line = format!("\"{}\" --hidden", std::env::current_exe()?.display());
        command.args(["add", RUN_KEY, "/v", "Lody", "/t", "REG_SZ", "/d", &line, "/f"]);
    } else {
        command.args(["delete", RUN_KEY, "/v", "Lody", "/f"]);
    }
    let out = command.output()?;
    // Deleting a value that is not there fails, and is fine.
    if on && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(std::io::Error::other(err.trim().to_string()));
    }
    Ok(())
}

pub fn open(url: &str) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    // explorer, not `cmd /c start`: cmd would cut a link at its first `&`.
    let mut command = if cfg!(windows) {
        std::process::Command::new("explorer")
    } else {
        std::process::Command::new(program)
    };
    command.arg(url).spawn()?;
    Ok(())
}
