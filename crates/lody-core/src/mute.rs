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
use std::time::Duration;

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

/// Muted by this run while speaking.
static MUTED: Mutex<Vec<Muted>> = Mutex::new(Vec::new());
/// Sounds already muted when Lody muted the rest: muted by you, so never unmuted by Lody,
/// even when another sound of the same program was Lody's.
static YOURS: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Muted by an earlier run that stopped first, and not found since: a program has its sound
/// only while it plays, and Windows brings it back muted when it plays again.
static LEFT: Mutex<Vec<Muted>> = Mutex::new(Vec::new());

/// Look for the programs left muted this often, until all are found.
const LOOK_AGAIN: Duration = Duration::from_secs(5);

fn record() -> PathBuf {
    crate::settings::state_dir().join("muted-programs.txt")
}

/// Write down every program still muted by Lody, one line each: "<sound>\t<program>".
/// `muted` is this run's, already locked (locks are taken in that order: MUTED, then LEFT).
fn write_down(muted: &[Muted]) {
    let left = LEFT.lock().unwrap();
    let lines: Vec<String> =
        muted.iter().chain(left.iter()).map(|m| format!("{}\t{}", m.sound, m.program)).collect();
    let path = record();
    let _ = if lines.is_empty() {
        std::fs::remove_file(path)
    } else {
        std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|_| std::fs::write(path, lines.join("\n")))
    };
}

/// Both ids of each, for matching.
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
        on_com_thread(mute_all).map(|(found, yours)| {
            *muted = found;
            *YOURS.lock().unwrap() = Some(yours);
        })
    } else {
        let ids = ids(&muted);
        let yours = YOURS.lock().unwrap().take().unwrap_or_default();
        muted.clear();
        on_com_thread(move || unmute(&ids, &yours).map(|_| ()))
    };
    if let Err(e) = result {
        log::warn!("could not {} the other programs: {e}", if mute { "mute" } else { "unmute" });
    }
    write_down(&muted);
}

/// Unmute what an earlier run muted and did not get to unmute (it stopped while speaking).
/// A program not playing now is looked for again every few seconds, and unmuted when it is.
pub fn recover() {
    let Ok(text) = std::fs::read_to_string(record()) else { return };
    // Older runs wrote only the sound's id.
    let left: Vec<Muted> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (sound, program) = line.split_once('\t').unwrap_or((line, ""));
            Muted { sound: sound.to_string(), program: program.to_string() }
        })
        .collect();
    *LEFT.lock().unwrap() = left;
    std::thread::spawn(|| {
        loop {
            let wanted = ids(&LEFT.lock().unwrap());
            // The first time one is seen again it is still the mute Lody left; after that it
            // is off the list, so muting it yourself later is kept.
            match on_com_thread(move || unmute(&wanted, &HashSet::new())) {
                Ok(found) => LEFT
                    .lock()
                    .unwrap()
                    .retain(|m| !found.contains(&m.sound) && !found.contains(&m.program)),
                Err(e) => log::warn!("could not unmute the programs muted last time: {e}"),
            }
            write_down(&MUTED.lock().unwrap());
            if LEFT.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(LOOK_AGAIN);
        }
    });
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

/// Mute each program playing through the default output; with the sounds that were muted
/// already (yours). One Windows can't answer about (a sound just closing) is skipped: it never
/// costs the list of those already muted, which would then stay muted with nothing to unmute them.
fn mute_all() -> Result<(Vec<Muted>, HashSet<String>)> {
    let (mut muted, mut yours) = (Vec::new(), HashSet::new());
    for (control, volume) in sessions()? {
        match mute_one(&control, &volume) {
            Some(Ok(found)) => muted.push(found),
            Some(Err(sound)) => {
                yours.insert(sound);
            }
            None => {}
        }
    }
    Ok((muted, yours))
}

/// Mute one sound: `Ok` with what Lody muted, `Err` with the sound's id when it was muted
/// already, `None` when it is Lody's own, the system's, or Windows can't answer.
fn mute_one(
    control: &IAudioSessionControl2,
    volume: &ISimpleAudioVolume,
) -> Option<std::result::Result<Muted, String>> {
    unsafe {
        let ours = control.GetProcessId().is_ok_and(|pid| pid == std::process::id());
        if ours || control.IsSystemSoundsSession() == S_OK {
            return None;
        }
        let sound = text(control.GetSessionInstanceIdentifier().ok()?);
        if volume.GetMute().ok()?.as_bool() {
            return Some(Err(sound));
        }
        let program = control.GetSessionIdentifier().map(text).unwrap_or_default();
        volume.SetMute(true, std::ptr::null()).ok()?;
        Some(Ok(Muted { sound, program }))
    }
}

/// Unmute the sounds with one of `ids`: the same sound, or the same program since restarted,
/// but never one of `yours`. Each on its own, like muting. Returns the ids of those found,
/// muted or not (you may have unmuted them already): a program not playing now isn't among them.
fn unmute(ids: &HashSet<String>, yours: &HashSet<String>) -> Result<HashSet<String>> {
    let mut found = HashSet::new();
    for (control, volume) in sessions()? {
        let sound = unsafe { control.GetSessionInstanceIdentifier() }.map(text).unwrap_or_default();
        let program = unsafe { control.GetSessionIdentifier() }.map(text).unwrap_or_default();
        if yours.contains(&sound) || (!ids.contains(&sound) && !ids.contains(&program)) {
            continue;
        }
        let muted = unsafe { volume.GetMute() }.is_ok_and(|m| m.as_bool());
        match muted.then(|| unsafe { volume.SetMute(false, std::ptr::null()) }) {
            Some(Err(e)) => log::warn!("could not unmute a program: {e}"),
            _ => found.extend([sound, program]),
        }
    }
    Ok(found)
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
        // It unmutes in the background: wait until it has found them all.
        for _ in 0..50 {
            if super::LEFT.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
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

    /// What you muted yourself stays muted when Lody gives the sound back. Run by hand like the
    /// above; everything it mutes is unmuted at the end.
    #[test]
    #[ignore]
    fn a_program_you_muted_stays_muted() {
        if std::env::var_os("LODY_TEST_MUTE").is_none() {
            return;
        }
        // You mute what's playing, before Lody speaks.
        let yours = super::on_com_thread(|| {
            let mut yours = Vec::new();
            for (control, volume) in super::sessions()? {
                if unsafe { control.IsSystemSoundsSession() } == super::S_OK {
                    continue;
                }
                unsafe { volume.SetMute(true, std::ptr::null())? };
                yours.push(super::text(unsafe { control.GetSessionInstanceIdentifier()? }));
            }
            Ok(yours)
        })
        .unwrap();
        assert!(!yours.is_empty(), "play something first");

        super::others(true);
        super::others(false);
        let still = super::on_com_thread({
            let yours = yours.clone();
            move || {
                let mut still = 0;
                for (control, volume) in super::sessions()? {
                    let sound = super::text(unsafe { control.GetSessionInstanceIdentifier()? });
                    if yours.contains(&sound) {
                        still += unsafe { volume.GetMute()?.as_bool() } as usize;
                        unsafe { volume.SetMute(false, std::ptr::null())? }; // yours back
                    }
                }
                Ok(still)
            }
        })
        .unwrap();
        assert_eq!(still, yours.len(), "Lody unmuted what you had muted");
    }
}
