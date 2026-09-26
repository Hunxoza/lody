//! Silence the other programs while Lody speaks, and give them their sound back after. Only
//! the programs that were not muted already are muted, and only those are unmuted again. Which
//! ones is also written down, so if Lody stops without unmuting them, the next start does it.
//! Windows' per-program volume (the volume mixer), on the default output.

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
use windows::core::{Interface, Result};

/// The programs Lody muted (Windows' id for each one's sound).
static MUTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record() -> PathBuf {
    crate::settings::state_dir().join("muted-programs.txt")
}

/// Mute (true) the other programs that are not muted, or unmute (false) the ones Lody muted.
pub fn others(mute: bool) {
    let mut muted = MUTED.lock().unwrap();
    if mute == !muted.is_empty() {
        return;
    }
    let result = if mute {
        on_com_thread(mute_all).map(|ids| *muted = ids)
    } else {
        let ids: HashSet<String> = muted.drain(..).collect();
        on_com_thread(move || unmute(&ids))
    };
    if let Err(e) = result {
        log::warn!("could not {} the other programs: {e}", if mute { "mute" } else { "unmute" });
    }
    let path = record();
    let _ = if muted.is_empty() {
        std::fs::remove_file(path)
    } else {
        std::fs::create_dir_all(path.parent().unwrap())
            .and_then(|_| std::fs::write(path, muted.join("\n")))
    };
}

/// Unmute what an earlier run muted and did not get to unmute (it was closed while speaking).
pub fn recover() {
    let Ok(text) = std::fs::read_to_string(record()) else { return };
    let ids: HashSet<String> = text.lines().map(str::to_string).collect();
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

fn mute_all() -> Result<Vec<String>> {
    let mut muted = vec![];
    for (control, volume) in sessions()? {
        unsafe {
            let ours = control.GetProcessId().is_ok_and(|pid| pid == std::process::id());
            if ours || control.IsSystemSoundsSession() == S_OK || volume.GetMute()?.as_bool() {
                continue;
            }
            let id = instance_id(&control)?;
            volume.SetMute(true, std::ptr::null())?;
            muted.push(id);
        }
    }
    Ok(muted)
}

fn unmute(ids: &HashSet<String>) -> Result<()> {
    for (control, volume) in sessions()? {
        if ids.contains(&instance_id(&control)?) {
            unsafe { volume.SetMute(false, std::ptr::null())? };
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

fn instance_id(control: &IAudioSessionControl2) -> Result<String> {
    unsafe {
        let text = control.GetSessionInstanceIdentifier()?;
        let id = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.0 as _));
        Ok(id)
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
}
