//! Silence the other programs while Lody speaks, and give them their sound back after. Only
//! the programs that were not muted already are muted, and only those are unmuted again. Which
//! ones is also written down, so if Lody stops without unmuting them, the next start does it.
//! Windows' per-program volume (the volume mixer), on the default output.
//!
//! Each is known two ways: its sound this run (gone once the program restarts its audio), and
//! the program itself (Windows keeps a program's mute across restarts, so a restarted browser
//! comes back muted; that is how it is found then).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::{
    IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::core::{Interface, PWSTR, Result};

/// A program Lody muted: Windows' id for its sound this run, and for the program itself.
#[derive(Debug, Clone)]
struct Muted {
    sound: String,
    program: String,
}

static MUTED: Mutex<Vec<Muted>> = Mutex::new(Vec::new());

fn record() -> PathBuf {
    crate::settings::state_dir().join("muted-programs.txt")
}

/// Both ids of each, for matching: one line per program, "<sound>\t<program>".
fn ids(muted: &[Muted]) -> HashSet<String> {
    muted
        .iter()
        .flat_map(|m| [m.sound.clone(), m.program.clone()])
        .filter(|id| !id.is_empty())
        .collect()
}

/// Mute (true) the other programs that are not muted, or unmute (false) the ones Lody muted.
pub fn others(mute: bool) {
    let mut muted = MUTED.lock().unwrap();
    if mute == !muted.is_empty() {
        return;
    }
    let result = if mute {
        on_com_thread(mute_all).map(|found| *muted = found)
    } else {
        let ids = ids(&muted);
        muted.clear();
        on_com_thread(move || unmute(&ids))
    };
    if let Err(e) = result {
        log::warn!("could not {} the other programs: {e}", if mute { "mute" } else { "unmute" });
    }
    let path = record();
    let lines: Vec<String> = muted.iter().map(|m| format!("{}\t{}", m.sound, m.program)).collect();
    let _ = if muted.is_empty() {
        std::fs::remove_file(path)
    } else {
        std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|_| std::fs::write(path, lines.join("\n")))
    };
}

/// Unmute what an earlier run muted and did not get to unmute (it was closed while speaking).
pub fn recover() {
    let Ok(text) = std::fs::read_to_string(record()) else { return };
    // Older runs wrote only the sound's id.
    let ids: HashSet<String> = text
        .lines()
        .flat_map(|line| line.split('\t'))
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    if let Err(e) = on_com_thread(move || unmute(&ids)) {
        log::warn!("could not unmute the programs muted last time: {e}");
    }
    let _ = std::fs::remove_file(record());
}

/// COM work on a thread of its own, so it cannot clash with how the audio output set COM up.
fn on_com_thread<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = f();
        CoUninitialize();
        result
    })
    .join()
    .unwrap_or_else(|_| Err(windows::core::Error::empty()))
}

/// Mute each program playing through the default output. One Windows can't answer about
/// (a sound just closing) is skipped: it never costs the list of those already muted, which
/// would then stay muted with nothing to unmute them.
fn mute_all() -> Result<Vec<Muted>> {
    Ok(sessions()?.iter().filter_map(|(control, volume)| mute_one(control, volume)).collect())
}

fn mute_one(control: &IAudioSessionControl2, volume: &ISimpleAudioVolume) -> Option<Muted> {
    unsafe {
        let ours = control.GetProcessId().is_ok_and(|pid| pid == std::process::id());
        if ours || control.IsSystemSoundsSession() == S_OK || volume.GetMute().ok()?.as_bool() {
            return None;
        }
        let found = Muted {
            sound: text(control.GetSessionInstanceIdentifier().ok()?),
            program: control.GetSessionIdentifier().map(text).unwrap_or_default(),
        };
        volume.SetMute(true, std::ptr::null()).ok()?;
        Some(found)
    }
}

/// Unmute the muted sounds with one of `ids`: the same sound, or the same program since
/// restarted. Each on its own, like muting.
fn unmute(ids: &HashSet<String>) -> Result<()> {
    for (control, volume) in sessions()? {
        let theirs = || unsafe {
            if !volume.GetMute().ok()?.as_bool() {
                return None;
            }
            let sound = control.GetSessionInstanceIdentifier().map(text).unwrap_or_default();
            let program = control.GetSessionIdentifier().map(text).unwrap_or_default();
            Some(ids.contains(&sound) || ids.contains(&program))
        };
        if theirs() == Some(true)
            && let Err(e) = unsafe { volume.SetMute(false, std::ptr::null()) }
        {
            log::warn!("could not unmute a program: {e}");
        }
    }
    Ok(())
}

/// Each program's sound on the default output.
fn sessions() -> Result<Vec<(IAudioSessionControl2, ISimpleAudioVolume)>> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let list = manager.GetSessionEnumerator()?;
        (0..list.GetCount()?)
            .map(|i| {
                let control = list.GetSession(i)?;
                Ok((control.cast()?, control.cast()?))
            })
            .collect()
    }
}

/// A string Windows hands over, freed after.
fn text(from: PWSTR) -> String {
    unsafe {
        let text = from.to_string().unwrap_or_default();
        CoTaskMemFree(Some(from.0 as _));
        text
    }
}

#[cfg(test)]
mod tests {
    /// Mutes the programs playing now for two seconds: run by hand with something playing,
    /// `LODY_TEST_MUTE=1 cargo test -p lody-core mute -- --ignored`. Without the variable it
    /// does nothing, so `cargo test -- --ignored` never silences anyone's music.
    #[test]
    #[ignore]
    fn mutes_and_unmutes_what_is_playing() {
        if std::env::var_os("LODY_TEST_MUTE").is_none() {
            return;
        }
        super::others(true);
        let muted = super::MUTED.lock().unwrap().clone();
        println!("muted {} programs: {muted:#?}", muted.len());
        std::thread::sleep(std::time::Duration::from_secs(2));
        super::others(false);
        assert!(super::MUTED.lock().unwrap().is_empty());
        assert!(!super::record().exists());
    }

    /// Lody closed while it had them muted, and they restarted their sound since (so only the
    /// program's id still matches): the next start unmutes them. Run by hand like the above.
    #[test]
    #[ignore]
    fn the_next_start_unmutes_programs_that_restarted() {
        if std::env::var_os("LODY_TEST_MUTE").is_none() {
            return;
        }
        super::others(true);
        let muted = std::mem::take(&mut *super::MUTED.lock().unwrap());
        assert!(!muted.is_empty(), "play something first");
        let lines: Vec<String> = muted.iter().map(|m| format!("gone\t{}", m.program)).collect();
        std::fs::write(super::record(), lines.join("\n")).unwrap();

        super::recover();
        let still = super::on_com_thread(|| {
            let mut muted = 0;
            for (_, volume) in super::sessions()? {
                muted += unsafe { volume.GetMute()?.as_bool() } as usize;
            }
            Ok(muted)
        })
        .unwrap();
        assert_eq!(still, 0, "programs left muted");
        assert!(!super::record().exists());
    }
}
