//! The island window covers the whole primary monitor and is click-through, except over its glass surfaces.
//!
//! The UI reports the rectangles of its interactive surfaces (`set_hit_regions`). A small thread watches the
//! mouse and switches click-through off only while the pointer is over one of them, so clicks anywhere else
//! reach the apps underneath. (Electron had `forward: true` for this; Tauri needs the explicit check.)

use serde::Deserialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, PhysicalPosition, Runtime, WebviewWindow};

pub const ISLAND: &str = "island";
const POLL: Duration = Duration::from_millis(25);
/// After the agent clicks, the island stays click-through this long even under the pointer: the agent's next
/// click often comes at the same spot, where its pointer still rests.
const AFTER_AGENT_CLICK: Duration = Duration::from_millis(1500);

/// A rectangle in CSS pixels relative to the island window, as measured by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Is the point (CSS px, window relative) over any interactive surface? A small margin makes edges easy to hit.
pub fn hit_test(regions: &[Rect], x: f64, y: f64, margin: f64) -> bool {
    regions.iter().any(|r| {
        Rect { x: r.x - margin, y: r.y - margin, width: r.width + 2.0 * margin, height: r.height + 2.0 * margin }
            .contains(x, y)
    })
}

#[derive(Default)]
pub struct IslandState {
    pub regions: Mutex<Vec<Rect>>,
    /// While true (text box open) the whole island takes mouse input.
    pub capture_all: AtomicBool,
    /// Current click-through setting, to avoid calling the OS every tick.
    pub passing_through: AtomicBool,
    /// A desktop task is driving the mouse. The island (its Stop button) answers only the user's pointer; every
    /// click of the agent goes through to the app below.
    pub agent_working: AtomicBool,
    /// When the agent last clicked or scrolled.
    pub agent_clicked_at: Mutex<Option<Instant>>,
    /// A confirmation card is open: its buttons must be clickable even during a desktop task.
    pub asking: AtomicBool,
    /// The island page has loaded. Until then the window stays hidden: a page that failed to load would
    /// otherwise cover the whole screen with an error page.
    pub ready: AtomicBool,
}

/// Places the island over the primary monitor.
pub fn fit_to_primary<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    if let Some(monitor) = window.primary_monitor()? {
        window.set_position(*monitor.position())?;
        window.set_size(*monitor.size())?;
    }
    Ok(())
}

/// Starts the pointer watcher. It only calls into the OS when the decision changes.
pub fn start_pointer_watch<R: Runtime>(app: AppHandle<R>) {
    // The agent's clicks always go through: if the user's pointer rests on the island, it lets go first.
    let hook = app.clone();
    anytask_control::before_mouse(move || {
        let state = hook.state::<IslandState>();
        *state.agent_clicked_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        if !state.passing_through.swap(true, Ordering::Relaxed)
            && let Some(window) = hook.get_webview_window(ISLAND)
        {
            let _ = window.set_ignore_cursor_events(true);
            // The window style changes on the UI thread; give it a moment before the click.
            std::thread::sleep(Duration::from_millis(60));
        }
    });
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(POLL);
            let Some(window) = app.get_webview_window(ISLAND) else { continue };
            if !window.is_visible().unwrap_or(false) {
                continue;
            }
            let state = app.state::<IslandState>();
            let hands_off = state.agent_working.load(Ordering::Relaxed) && !state.asking.load(Ordering::Relaxed);
            let agent_just_clicked =
                state.agent_clicked_at.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() < AFTER_AGENT_CLICK);
            let over = if state.capture_all.load(Ordering::Relaxed) {
                true
            } else if hands_off && agent_just_clicked {
                false
            } else {
                match (app.cursor_position(), window.outer_position(), window.scale_factor()) {
                    (Ok(cursor), Ok(origin), Ok(scale)) => pointer_over(&state, cursor, origin, scale),
                    _ => false,
                }
            };
            let pass = !over;
            if state.passing_through.swap(pass, Ordering::Relaxed) != pass {
                let _ = window.set_ignore_cursor_events(pass);
            }
        }
    });
}

/// While a desktop task runs, the island is left out of the agent's screenshots (it would cover the top of the
/// screen, where search boxes and address bars are) and lets the agent's clicks through.
pub fn follow_desktop_tasks(app: AppHandle) {
    use anytask_core::CoreEvent;
    let mut rx = app.state::<crate::brain::Brain>().agent.events().subscribe();
    tauri::async_runtime::spawn(async move {
        let mut running = 0usize;
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let before = running;
            match &event {
                CoreEvent::ToolStarted { name, .. } if name == "desktop_task" => running += 1,
                CoreEvent::ToolFinished { name, .. } if name == "desktop_task" => running = running.saturating_sub(1),
                CoreEvent::Cancelled { .. } | CoreEvent::Answer { .. } | CoreEvent::Error { .. } => running = 0,
                _ => continue,
            }
            if (before > 0) == (running > 0) {
                continue;
            }
            let working = running > 0;
            app.state::<IslandState>().agent_working.store(working, Ordering::Relaxed);
            if let Some(window) = app.get_webview_window(ISLAND)
                && let Ok(hwnd) = window.hwnd()
            {
                let _ = anytask_control::exclude_from_capture(hwnd.0 as isize, working);
            }
        }
    });
}

/// Makes the whole island click-through; called once at start so the initial state matches `passing_through`.
pub fn start_click_through<R: Runtime>(window: &WebviewWindow<R>, state: &IslandState) -> tauri::Result<()> {
    state.passing_through.store(true, Ordering::Relaxed);
    window.set_ignore_cursor_events(true)
}

fn pointer_over(state: &IslandState, cursor: PhysicalPosition<f64>, origin: PhysicalPosition<i32>, scale: f64) -> bool {
    let x = (cursor.x - f64::from(origin.x)) / scale;
    let y = (cursor.y - f64::from(origin.y)) / scale;
    let regions = state.regions.lock().map(|r| r.clone()).unwrap_or_default();
    hit_test(&regions, x, y, 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PILL: Rect = Rect { x: 100.0, y: 10.0, width: 168.0, height: 44.0 };

    #[test]
    fn inside_and_outside() {
        assert!(hit_test(&[PILL], 150.0, 30.0, 0.0));
        assert!(!hit_test(&[PILL], 50.0, 30.0, 0.0));
        assert!(!hit_test(&[], 150.0, 30.0, 0.0));
    }

    #[test]
    fn margin_makes_edges_easier() {
        assert!(!hit_test(&[PILL], 99.0, 30.0, 0.0));
        assert!(hit_test(&[PILL], 99.0, 30.0, 2.0));
    }

    #[test]
    fn any_region_counts() {
        let card = Rect { x: 400.0, y: 400.0, width: 50.0, height: 50.0 };
        assert!(hit_test(&[PILL, card], 420.0, 420.0, 0.0));
    }

    #[test]
    fn pointer_is_converted_from_physical_pixels() {
        let state = IslandState::default();
        *state.regions.lock().unwrap() = vec![PILL];
        // Window at (0,0) on a 150% display: physical (225, 45) is CSS (150, 30), inside the pill.
        assert!(pointer_over(&state, PhysicalPosition::new(225.0, 45.0), PhysicalPosition::new(0, 0), 1.5));
        // Same physical point on a 100% display is CSS (225, 45): still inside; (500, 45) is outside.
        assert!(!pointer_over(&state, PhysicalPosition::new(500.0, 45.0), PhysicalPosition::new(0, 0), 1.0));
    }
}
