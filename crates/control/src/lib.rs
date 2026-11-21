//! The real operating-system side of the assistant: clipboard, media keys, the speakers' volume, opening things
//! (SystemOps) and desktop control: screenshots with the controls on them, mouse, keyboard and starting apps (DesktopOps).

pub mod apps;
pub mod input;
pub mod inventory;
pub mod machine;
pub mod marks;
pub mod media_session;
pub mod net;
pub mod ocr;
pub mod screen;
pub mod uia;
pub mod volume;

use anytask_core::tools::desktop::{DesktopOps, Look, MouseButton};
use anytask_core::tools::system::{BrowserTab, MediaKey, NowPlaying, SystemOps};
use std::time::Duration;

/// Screenshots sent to the model are at most this wide: enough to read text, small enough to be fast.
const SHOT_WIDTH: u32 = 1024;
const JPEG_QUALITY: u8 = 60;
const OCR_TIMEOUT: Duration = Duration::from_millis(2500);
const UIA_TIMEOUT: Duration = Duration::from_millis(4000);
const FINGERPRINT_WIDTH: u32 = 320;
/// Fewer controls than this in a Chromium window means its accessibility tree was not built yet.
const CHROMIUM_FEW: usize = 8;
/// Below this many controls a Chromium window may still be building its page: asked again until the number holds.
const CHROMIUM_SETTLED: usize = 40;
const CHROMIUM_POLL: Duration = Duration::from_millis(200);
/// The window title and control count of the last look.
static LAST_LOOK: std::sync::Mutex<(String, usize)> = std::sync::Mutex::new((String::new(), 0));
const CHROMIUM_WAKE: Duration = Duration::from_millis(500);
/// How long to keep asking a nearly empty Chromium window for its controls.
/// 11-10: Spotify started by a spotify: link still had 4 controls after 2.5 s (62 on the next look), and the model
/// opened the search again. A window seen with the same count before is not waited for again (LAST_LOOK).
const CHROMIUM_PATIENCE: Duration = Duration::from_millis(4500);
/// How long to keep asking a browser for its address bar (it builds its accessibility tree on the first ask).
const BROWSER_PATIENCE: Duration = Duration::from_millis(3000);

/// The controls of a window, without its text lines: what tells whether a Chromium page has built its tree.
fn controls(window: &uia::Window) -> usize {
    window.elements.iter().filter(|e| e.kind != "text").count()
}

/// Starts reading the words of the whole screen at full size (small text is lost in the scaled-down screenshot) on
/// its own thread; the lines and the picture size arrive on the channel, nothing when this system cannot read.
fn read_text() -> std::sync::mpsc::Receiver<(Vec<ocr::TextLine>, (u32, u32))> {
    let (sent, got) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((full, _)) = screen::capture_primary(u32::MAX)
            && let Ok(lines) = ocr::read(&full, OCR_TIMEOUT)
        {
            let _ = sent.send((lines, (full.width, full.height)));
        }
    });
    got
}

#[derive(Default)]
pub struct NativeDesktop;

/// Windows' own notification sound: heard even when the spoken voice cannot be reached. A hidden PowerShell plays
/// it (rare: only for a reminder that could not be spoken).
#[cfg(windows)]
pub fn chime() {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "[System.Media.SystemSounds]::Asterisk.Play(); Start-Sleep -Milliseconds 800"])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

#[cfg(not(windows))]
pub fn chime() {}

/// Leaves a window out of screenshots and screen recordings (it stays visible on the monitor), or puts it back.
/// Used for the island while the desktop agent looks at the screen, so it never covers what the agent must see.
#[cfg(windows)]
pub fn exclude_from_capture(hwnd: isize, exclude: bool) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};
    let affinity = if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    // SAFETY: the handle comes from a live window of this process; the call only changes how it is captured.
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut std::ffi::c_void), affinity) }.map_err(|e| e.to_string())
}

/// Runs before every mouse action the assistant makes; the island uses it to let the click through even when the
/// user's pointer happens to rest on it.
static BEFORE_MOUSE: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

pub fn before_mouse(hook: impl Fn() + Send + Sync + 'static) {
    let _ = BEFORE_MOUSE.set(Box::new(hook));
}

fn mouse_coming() {
    if let Some(hook) = BEFORE_MOUSE.get() {
        hook();
    }
}

#[cfg(not(windows))]
pub fn exclude_from_capture(_hwnd: isize, _exclude: bool) -> Result<(), String> {
    Ok(())
}

impl DesktopOps for NativeDesktop {
    fn look(&self) -> Result<Look, String> {
        let (mut image, screen_size) = screen::capture_primary(SHOT_WIDTH)?;
        // The words on the screen are read on every look, while UI Automation is asked, so the model never acts on the
        // boxes alone (11-11: it scrolled past a playlist's end into "Recommended" and played a song from there).
        let mut reading = read_text();
        let mut window = uia::foreground(screen_size, UIA_TIMEOUT);
        // Chromium apps (browsers, Spotify, Electron) build their accessibility tree only after a client asks;
        // the first answer is nearly empty and the controls come a moment later. Spotify needs more than a second
        // (logs: 4 controls on the first look, 80 on the next), so ask again for a while.
        // A page that just opened keeps adding controls for a moment too (Spotify search, 11-06: 14 controls, then 54,
        // then 80; on the 14 the model saw no results and opened the search again 3-5 times): while a Chromium window
        // has few controls, ask until two reads agree. A window that showed the same count on the last look has
        // settled already and is not asked again.
        let asked = std::time::Instant::now();
        let mut waited = false;
        let seen_before = LAST_LOOK.lock().is_ok_and(|last| *last == (window.title.clone(), controls(&window)));
        if window.chromium && controls(&window) < CHROMIUM_FEW && seen_before {
            // The same nearly empty tree as last time: its page may still be coming (11-11: Spotify gave 4 controls
            // on eight looks in a row and 80 only on the ninth). One more short read, not the whole patience again.
            std::thread::sleep(CHROMIUM_WAKE);
            let next = uia::foreground(screen_size, UIA_TIMEOUT);
            if controls(&next) > controls(&window) {
                window = next;
                waited = true;
            }
        } else if window.chromium && controls(&window) < CHROMIUM_SETTLED && !seen_before {
            while asked.elapsed() < CHROMIUM_PATIENCE {
                std::thread::sleep(if controls(&window) < CHROMIUM_FEW { CHROMIUM_WAKE } else { CHROMIUM_POLL });
                let next = uia::foreground(screen_size, UIA_TIMEOUT);
                waited = true;
                let same = controls(&next) == controls(&window);
                window = next;
                if same && controls(&window) >= CHROMIUM_FEW {
                    break;
                }
            }
        }
        if let Ok(mut last) = LAST_LOOK.lock() {
            *last = (window.title.clone(), controls(&window));
        }
        if waited {
            // The page drew itself meanwhile: take the picture again so the boxes and the words match it.
            image = screen::capture_primary(SHOT_WIDTH)?.0;
            reading = read_text();
        }
        // The lines OCR reads join the controls as text the model can click by its words.
        if let Ok((lines, picture)) = reading.recv_timeout(OCR_TIMEOUT + Duration::from_millis(500)) {
            ocr::merge(&mut window.elements, &lines, picture);
        }
        let fingerprint = marks::fingerprint(&image);
        // No boxes or numbers over the screen any more: the model clicks controls by their names (11-12), and a clean
        // picture shows the app as it is. The rulers stay for the few things clicked by X Y.
        marks::draw_rulers(&mut image);
        let jpeg = marks::encode_jpeg(&image, JPEG_QUALITY)?;
        Ok(Look { jpeg, title: window.title, elements: window.elements, fingerprint })
    }

    fn click(&self, x: u16, y: u16, button: MouseButton, count: u8) -> Result<(), String> {
        mouse_coming();
        input::click(x, y, button, count)
    }

    fn click_exact(&self, x: f32, y: f32, button: MouseButton, count: u8) -> Result<(), String> {
        mouse_coming();
        input::click_exact(x, y, button, count)
    }

    fn scroll(&self, x: u16, y: u16, up: bool) -> Result<(), String> {
        mouse_coming();
        input::scroll(x, y, up)
    }

    fn type_text(&self, text: &str) -> Result<(), String> {
        if !input::should_paste(text, &self.active_title()) {
            return input::type_keys(text);
        }
        // Paste through the clipboard, then put the user's clipboard back.
        let mut clipboard = arboard::Clipboard::new().map_err(|e| format!("Clipboard: {e}"))?;
        let previous = clipboard.get_text().ok();
        clipboard.set_text(text).map_err(|e| format!("Clipboard: {e}"))?;
        std::thread::sleep(Duration::from_millis(60));
        input::press(&input::parse_combo("ctrl+v")?)?;
        // The app reads the clipboard right after Ctrl+V.
        std::thread::sleep(Duration::from_millis(350));
        if let Some(previous) = previous {
            let _ = clipboard.set_text(previous);
        }
        Ok(())
    }

    fn hotkey(&self, combo: &str) -> Result<(), String> {
        input::press(&input::parse_combo(combo)?)
    }

    fn launch_app(&self, name: &str) -> Result<(), String> {
        apps::launch(name)
    }

    fn fingerprint(&self) -> Result<u64, String> {
        // A small capture of the window in front is enough to see whether it still changes (a few ms); other
        // windows that animate (clocks, videos, spinners) must not keep us waiting.
        screen::capture_foreground(FINGERPRINT_WIDTH).map(|image| marks::fingerprint(&image))
    }

    fn active_title(&self) -> String {
        uia::active_title()
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        // Windows Settings pages too (ms-settings:bluetooth); other schemes can start programs and stay blocked.
        if !["http://", "https://", "spotify:", "ms-settings:"].iter().any(|scheme| url.to_ascii_lowercase().starts_with(scheme)) {
            return Err("Only web, Spotify and Windows Settings links can be opened.".into());
        }
        // Spaces would cut the address (models write them in search and mail links).
        let url = url.trim().replace(' ', "%20");
        open::that_detached(&url).map_err(|e| format!("Could not open {url}: {e}"))
    }

    fn foreground(&self) -> isize {
        apps::foreground_window()
    }

    fn app_window(&self, name: &str) -> Option<isize> {
        apps::app_window(name)
    }

    fn bring_to_front(&self, window: isize) -> Result<(), String> {
        apps::focus_window(window)
    }

    fn user_window(&self) -> Option<isize> {
        apps::user_window()
    }

    fn own_window_in_front(&self) -> bool {
        apps::own_window_in_front()
    }

    fn default_browser(&self) -> Option<String> {
        apps::default_browser()
    }

    fn close_app(&self, name: &str) -> Result<usize, String> {
        apps::close_app(name)
    }

    fn now_playing(&self) -> Option<(bool, String)> {
        media_session::now_playing().ok().map(|now| now.map(|n| (n.playing, n.label())).unwrap_or((false, String::new())))
    }

    fn screen_text(&self) -> Option<String> {
        let window = screen::capture_foreground(u32::MAX).ok()?;
        let lines = ocr::read(&window, OCR_TIMEOUT).ok()?;
        Some(lines.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n"))
    }
}

#[derive(Default)]
pub struct NativeSystem;

impl SystemOps for NativeSystem {
    fn clipboard_read(&self) -> Result<String, String> {
        arboard::Clipboard::new().and_then(|mut c| c.get_text()).map_err(|e| format!("Clipboard: {e}"))
    }

    fn clipboard_write(&self, text: &str) -> Result<(), String> {
        arboard::Clipboard::new().and_then(|mut c| c.set_text(text)).map_err(|e| format!("Clipboard: {e}"))
    }

    fn media_key(&self, key: MediaKey) -> Result<(), String> {
        media::press(key)
    }

    fn open(&self, target: &str) -> Result<(), String> {
        open::that_detached(target).map_err(|e| format!("Could not open {target}: {e}"))
    }

    fn now_playing(&self) -> Result<Option<NowPlaying>, String> {
        match media_session::now_playing() {
            // Not listed yet: a Spotify that was just opened still shows its state in its window title (test 3,
            // 05:20: "No music player is open" right after open_app put Spotify in front).
            Ok(None) => Ok(apps::spotify_from_title(&apps::top_windows())
                .map(|(playing, artist, title)| NowPlaying { app: "Spotify".into(), title, artist, playing })),
            listed => listed,
        }
    }

    fn media_play(&self, play: bool) -> Result<bool, String> {
        media_session::play(play)
    }

    fn volume(&self) -> Result<(u8, bool), String> {
        volume::get()
    }

    fn set_volume(&self, level: Option<u8>, mute: Option<bool>) -> Result<(), String> {
        volume::set(level, mute)
    }

    fn browser_tab(&self) -> Result<Option<BrowserTab>, String> {
        let windows = apps::top_windows();
        let Some((window, browser, title)) = apps::browser_window(&windows) else { return Ok(None) };
        let url = uia::browser_address(window.handle, BROWSER_PATIENCE);
        Ok(Some(BrowserTab { browser: browser.to_string(), title, url }))
    }

    fn launch_app(&self, name: &str) -> Result<(), String> {
        apps::launch(name)
    }

    fn app_buttons(&self, app: &str) -> Result<Vec<String>, String> {
        let window = apps::app_window(app).ok_or_else(|| format!("{app} has no window"))?;
        Ok(uia::buttons(window))
    }

    fn press_app_button(&self, app: &str, name: &str) -> Result<(), String> {
        let window = apps::app_window(app).ok_or_else(|| format!("{app} has no window"))?;
        uia::press(window, name)
    }
}

#[cfg(windows)]
mod media {
    use anytask_core::tools::system::MediaKey;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_MEDIA_NEXT_TRACK,
        VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
    };

    pub fn virtual_key(key: MediaKey) -> VIRTUAL_KEY {
        match key {
            MediaKey::PlayPause => VK_MEDIA_PLAY_PAUSE,
            MediaKey::Next => VK_MEDIA_NEXT_TRACK,
            MediaKey::Previous => VK_MEDIA_PREV_TRACK,
            MediaKey::Stop => VK_MEDIA_STOP,
            MediaKey::VolumeUp => VK_VOLUME_UP,
            MediaKey::VolumeDown => VK_VOLUME_DOWN,
            MediaKey::Mute => VK_VOLUME_MUTE,
        }
    }

    fn input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        let flags = if up { KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP } else { KEYEVENTF_EXTENDEDKEY };
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
    }

    pub fn press(key: MediaKey) -> Result<(), String> {
        let vk = virtual_key(key);
        let inputs = [input(vk, false), input(vk, true)];
        // SAFETY: the slice holds two fully initialised keyboard inputs and the size matches INPUT.
        let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent == inputs.len() as u32 { Ok(()) } else { Err("Windows blocked the media key (another program may hold input).".into()) }
    }
}

#[cfg(not(windows))]
mod media {
    use anytask_core::tools::system::MediaKey;

    pub fn press(_key: MediaKey) -> Result<(), String> {
        Err("Media keys are not supported on this system yet.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn media_keys_map_to_windows_keys() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_MEDIA_PLAY_PAUSE, VK_VOLUME_UP};
        assert_eq!(media::virtual_key(MediaKey::PlayPause), VK_MEDIA_PLAY_PAUSE);
        assert_eq!(media::virtual_key(MediaKey::VolumeUp), VK_VOLUME_UP);
    }

    /// Reads the real browser, so it only runs on request with a browser open:
    /// cargo test -p anytask-control -- --ignored reads_the_open_browser_tab --nocapture
    #[test]
    #[ignore]
    fn reads_the_open_browser_tab() {
        let started = std::time::Instant::now();
        let tab = NativeSystem.browser_tab().unwrap().expect("a browser window is open");
        println!("{tab:?} in {:?}", started.elapsed());
        assert!(tab.url.is_some_and(|u| u.contains("://")));
    }

    /// Reads this PC's settings, so it only runs on request: cargo test -p anytask-control -- --ignored default_browser --nocapture
    #[test]
    #[ignore]
    fn reads_the_default_browser() {
        println!("{:?}", NativeDesktop.default_browser());
        assert!(NativeDesktop.default_browser().is_some());
    }

    /// Touches the real clipboard, so it only runs on request: cargo test -p anytask-control -- --ignored
    #[test]
    #[ignore]
    fn clipboard_round_trip() {
        let system = NativeSystem;
        let before = system.clipboard_read().unwrap_or_default();
        system.clipboard_write("anytask-test سلام").unwrap();
        assert_eq!(system.clipboard_read().unwrap(), "anytask-test سلام");
        let _ = system.clipboard_write(&before);
    }
}
