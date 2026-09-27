//! The overlay: a small window with the latest reply in your language, above every other
//! window while you work in them. No title bar, not in the taskbar, and it doesn't take the
//! focus when it appears; it is dragged by its text and resized from its corner (`ui/overlay.js`).
//! Where it was left is remembered in `<state>/overlay.json`.
//!
//! Staying on top is up to each system: Windows and macOS keep it there (macOS on every desktop
//! too); on Linux X11 does, Wayland lets no program do it, so Lody runs under XWayland there
//! (see `main`).

use std::sync::{Mutex, Once};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

const LABEL: &str = "overlay";

/// Where the overlay was left, in the screen's own pixels.
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Place {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

/// Show (true) or close (false) the overlay.
pub fn set(app: &AppHandle, on: bool) -> tauri::Result<()> {
    match (on, app.get_webview_window(LABEL)) {
        (true, Some(window)) => window.show(),
        (true, None) => open(app),
        (false, Some(window)) => window.destroy(),
        (false, None) => Ok(()),
    }
}

fn open(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
        .title("Lody")
        .inner_size(520.0, 150.0)
        .min_inner_size(240.0, 80.0)
        .decorations(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()?;
    place(app, &window);
    window.on_window_event({
        let window = window.clone();
        move |event| {
            if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_))
                && let (Ok(pos), Ok(size)) = (window.outer_position(), window.inner_size())
            {
                remember(Place { x: pos.x, y: pos.y, width: size.width, height: size.height });
            }
        }
    });
    window.show()
}

/// Where it was left, if that is still on a screen; else low in the middle of the main screen.
fn place(app: &AppHandle, window: &WebviewWindow) {
    let saved = std::fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str::<Place>(&text).ok())
        .filter(|p| on_a_screen(app, p));
    if let Some(p) = saved {
        // Moved first: on a screen with another scale the system resizes a window moved onto
        // it, which would undo a size set before.
        let _ = window.set_position(PhysicalPosition::new(p.x, p.y));
        let _ = window.set_size(PhysicalSize::new(p.width, p.height));
    } else if let Ok(Some(screen)) = app.primary_monitor() {
        let area = screen.work_area();
        let size = window.outer_size().unwrap_or(PhysicalSize::new(520, 150));
        let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
        let y = area.position.y + area.size.height as i32 - size.height as i32 - 48;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
}

/// Whether the overlay's top edge would be on one of the screens now attached.
fn on_a_screen(app: &AppHandle, p: &Place) -> bool {
    app.available_monitors().unwrap_or_default().iter().any(|m| {
        let (pos, size) = (m.position(), m.size());
        p.x + 40 > pos.x
            && p.x < pos.x + size.width as i32 - 40
            && p.y >= pos.y
            && p.y < pos.y + size.height as i32 - 20
    })
}

fn path() -> std::path::PathBuf {
    lody_core::settings::state_dir().join("overlay.json")
}

static PENDING: Mutex<Option<Place>> = Mutex::new(None);

/// Keep the latest place and write it a moment later: dragging moves it many times a second.
fn remember(place: Place) {
    *PENDING.lock().unwrap() = Some(place);
    static WRITER: Once = Once::new();
    WRITER.call_once(|| {
        std::thread::spawn(|| {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(place) = PENDING.lock().unwrap().take() else { continue };
                let path = path();
                let written = std::fs::create_dir_all(path.parent().unwrap()).and_then(|_| {
                    std::fs::write(&path, serde_json::to_string(&place).unwrap_or_default())
                });
                if let Err(e) = written {
                    log::warn!("could not remember where the overlay is: {e}");
                }
            }
        });
    });
}
