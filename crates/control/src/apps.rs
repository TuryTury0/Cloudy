//! Starting installed apps by name (ported from v1 platform.js): well-known programs directly, apps that answer a
//! link protocol through it, anything else through the Start menu list (Get-StartApps).

/// Program for a well-known app name.
fn known(name: &str) -> Option<&'static str> {
    Some(match name {
        "chrome" | "google chrome" => "chrome.exe",
        "edge" | "microsoft edge" => "msedge.exe",
        "firefox" => "firefox.exe",
        "notepad" | "نوت پد" | "notepad.exe" => "notepad.exe",
        "calculator" | "calc" | "ماشین حساب" => "calc.exe",
        "explorer" | "files" | "file explorer" => "explorer.exe",
        "terminal" | "windows terminal" => "wt.exe",
        "paint" => "mspaint.exe",
        "word" => "winword.exe",
        "excel" => "excel.exe",
        "powerpoint" => "powerpnt.exe",
        "vscode" | "vs code" | "visual studio code" => "code",
        _ => return None,
    })
}

/// Apps that open through their own link: launching only focuses an already open window.
fn protocol(name: &str) -> Option<&'static str> {
    Some(match name {
        "spotify" => "spotify:",
        "discord" => "discord:",
        "steam" => "steam:",
        "settings" | "windows settings" => "ms-settings:",
        _ => return None,
    })
}

/// An installed app name, not a command or a file (v1 rules).
pub fn valid_name(value: &str) -> Option<String> {
    let name = value.trim();
    let lower = name.to_lowercase();
    let ok = (1..=120).contains(&name.chars().count())
        && name.chars().next().is_some_and(char::is_alphanumeric)
        && name.chars().all(|c| c.is_alphanumeric() || " _.()+-".contains(c))
        && !name.contains("..")
        && !name.ends_with('.')
        && !["setup", "installer"].iter().any(|w| lower.split([' ', '.', '_', '-']).any(|p| p == *w))
        && ![".bat", ".cmd", ".ps1", ".vbs", ".js", ".msi", ".scr", ".com", ".sh"].iter().any(|ext| lower.ends_with(ext))
        && !(lower.ends_with(".exe") && known(&lower).is_none());
    ok.then(|| name.to_string())
}

pub fn launch(name: &str) -> Result<(), String> {
    let name = valid_name(name).ok_or("Give an installed application name, not a command or a file path.")?;
    // An app that is already open comes to the front instead of starting again (a second Spotify, another
    // browser window).
    if let Some(window) = find_window(&name, &running::windows()) {
        return focus_window(window);
    }
    let lower = name.to_lowercase();
    if let Some(uri) = protocol(&lower) {
        return open::that_detached(uri).map_err(|e| format!("Could not open {name}: {e}"));
    }
    if let Some(program) = known(&lower) {
        return open::that_detached(program).map_err(|e| format!("Could not start {name}: {e}"));
    }
    start_menu(&name)
}

/// A visible top-level window of another program.
#[derive(Debug, Clone)]
pub struct TopWindow {
    pub handle: isize,
    pub title: String,
    /// Program file name without ".exe", lowercase ("spotify", "msedge").
    pub exe: String,
}

/// The program names (lowercase, without .exe) behind a name the user says.
fn exe_names(name: &str) -> Vec<String> {
    let name = name.trim().to_lowercase().replace(['-', '_'], " ");
    let alias: &[&str] = match name.as_str() {
        "word" | "microsoft word" => &["winword"],
        "excel" | "microsoft excel" => &["excel"],
        "powerpoint" | "microsoft powerpoint" => &["powerpnt"],
        "edge" | "microsoft edge" => &["msedge"],
        "chrome" | "google chrome" => &["chrome"],
        "vs code" | "vscode" | "visual studio code" | "code" => &["code"],
        "whatsapp" => &["whatsapp", "whatsapp.root"],
        _ => &[],
    };
    alias.iter().map(|s| s.to_string()).chain([name.replace(' ', "")]).collect()
}

/// The open window of the app with this name: by its program, else by its title ("Calculator", "… - Notepad").
pub fn find_window(name: &str, windows: &[TopWindow]) -> Option<isize> {
    let lower = name.trim().to_lowercase();
    // The Windows shell itself always runs; opening it means a new folder window.
    if lower.is_empty() || matches!(lower.as_str(), "explorer" | "file explorer" | "this pc") {
        return None;
    }
    let exes = exe_names(&lower);
    windows
        .iter()
        .find(|w| exes.contains(&w.exe))
        .or_else(|| {
            windows.iter().find(|w| {
                let title = w.title.to_lowercase();
                title == lower || title.ends_with(&format!(" - {lower}"))
            })
        })
        .map(|w| w.handle)
}

/// The visible top-level windows of other programs.
pub fn top_windows() -> Vec<TopWindow> {
    running::windows()
}

/// The folder open in the front-most File Explorer window ("save it here" means there), or None when no folder
/// window is open. Explorer tells its folders only through the Shell's COM objects, read here by a hidden
/// PowerShell (about half a second; a hung Shell is given up after 6 s).
#[cfg(windows)]
pub fn explorer_folder() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let front = top_windows().into_iter().find(|w| w.exe == "explorer" && w.title != "Program Manager")?;
    let script = "$ErrorActionPreference='SilentlyContinue'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; \
                  foreach($w in (New-Object -ComObject Shell.Application).Windows()){ $p=$w.Document.Folder.Self.Path; \
                  if($p){ [Console]::Out.WriteLine(\"$($w.HWND)`t$p\") } }";
    let child = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = rx.recv_timeout(std::time::Duration::from_secs(6)).ok()?.ok()?;
    folder_of(&String::from_utf8_lossy(&output.stdout), front.handle)
}

#[cfg(not(windows))]
pub fn explorer_folder() -> Option<String> {
    None
}

/// The folder of window `handle` in lines of "HWND<tab>path"; only real folders (not "This PC" and other Shell views).
pub fn folder_of(listing: &str, handle: isize) -> Option<String> {
    listing.lines().find_map(|line| {
        let (hwnd, path) = line.trim().split_once('\t')?;
        let mut chars = path.chars();
        let drive = chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':');
        let real = drive || path.starts_with("\\\\");
        (hwnd.trim().parse::<i64>().ok()? == handle as i64 && real).then(|| path.trim().to_string())
    })
}

/// Spotify's state from its window title, for when Windows does not list it as a player yet (a Spotify that was
/// just opened registers only once it plays): "Spotify", "Spotify Free" or "Spotify Premium" is open and paused;
/// "Artist - Song" plays. None when Spotify has no window.
pub fn spotify_from_title(windows: &[TopWindow]) -> Option<(bool, String, String)> {
    let window = windows.iter().find(|w| w.exe == "spotify" && !w.title.trim().is_empty())?;
    let title = window.title.trim();
    if matches!(title, "Spotify" | "Spotify Free" | "Spotify Premium") {
        return Some((false, String::new(), String::new()));
    }
    let (artist, song) = title.split_once(" - ").unwrap_or(("", title));
    Some((true, artist.trim().to_string(), song.trim().to_string()))
}

/// Browser programs and their names.
const BROWSERS: &[(&str, &str)] = &[
    ("vivaldi", "Vivaldi"),
    ("chrome", "Google Chrome"),
    ("msedge", "Microsoft Edge"),
    ("brave", "Brave"),
    ("firefox", "Firefox"),
    ("opera", "Opera"),
    ("arc", "Arc"),
];

/// The front-most browser window (the list is front first), its browser's name and the page title without the
/// browser's name ("Example Domain - Vivaldi" -> "Example Domain").
pub fn browser_window(windows: &[TopWindow]) -> Option<(&TopWindow, &'static str, String)> {
    windows.iter().find_map(|w| {
        let (_, name) = BROWSERS.iter().find(|(exe, _)| w.exe == *exe)?;
        let brand = name.split(' ').next_back().unwrap_or(name).to_lowercase();
        // Firefox ends its titles with " — Mozilla Firefox" (an em dash), the others with " - ".
        let title = match w.title.rsplit_once(" - ").or_else(|| w.title.rsplit_once(" — ")) {
            Some((page, tail)) if tail.to_lowercase().contains(&brand) => page.trim().to_string(),
            _ => w.title.trim().to_string(),
        };
        Some((w, *name, title))
    })
}

/// The front-most window of another program that the user works in, as Alt+Tab would pick it: not the desktop itself
/// ("Program Manager"), not a minimized window, not an overlay or tool window (`skip`).
pub fn first_user_window(windows: &[TopWindow], skip: impl Fn(&TopWindow) -> bool) -> Option<isize> {
    windows.iter().find(|w| !(w.exe == "explorer" && w.title == "Program Manager") && !skip(w)).map(|w| w.handle)
}

pub fn user_window() -> Option<isize> {
    first_user_window(&running::windows(), |w| running::minimized_or_overlay(w.handle))
}

/// The default web browser ("Vivaldi"), from the https handler Windows opens links with. Read once (a hidden
/// reg.exe call, a few ms) and kept.
pub fn default_browser() -> Option<String> {
    static BROWSER: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BROWSER.get_or_init(read_default_browser).clone()
}

#[cfg(windows)]
fn read_default_browser() -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg.exe")
        .args(["query", r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice", "/v", "ProgId"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let prog_id = text.lines().find(|l| l.contains("ProgId"))?.split_whitespace().last()?.to_string();
    crate::inventory::known_browser(&prog_id)
}

#[cfg(not(windows))]
fn read_default_browser() -> Option<String> {
    None
}

/// The window in front belongs to this program (the assistant's chat, settings or island).
#[cfg(windows)]
pub fn own_window_in_front() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0u32;
    // SAFETY: reads the process id of the foreground window into a local.
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid as *mut u32));
        pid != 0 && pid == GetCurrentProcessId()
    }
}

#[cfg(not(windows))]
pub fn own_window_in_front() -> bool {
    false
}

/// Every open window of the app with this name (by its program, else by its title, as `find_window`).
pub fn app_windows(name: &str, windows: &[TopWindow]) -> Vec<isize> {
    let lower = name.trim().to_lowercase();
    if lower.is_empty() || matches!(lower.as_str(), "explorer" | "file explorer" | "this pc") {
        return Vec::new();
    }
    let exes = exe_names(&lower);
    let by_program: Vec<isize> = windows.iter().filter(|w| exes.contains(&w.exe)).map(|w| w.handle).collect();
    if !by_program.is_empty() {
        return by_program;
    }
    windows
        .iter()
        // The shell's own windows never: WM_CLOSE to "Program Manager" opens the shut-down dialog.
        .filter(|w| w.exe != "explorer")
        .filter(|w| {
            let title = w.title.to_lowercase();
            title == lower || title.ends_with(&format!(" - {lower}"))
        })
        .map(|w| w.handle)
        .collect()
}

/// Asks every window of the app to close (WM_CLOSE, what its X button sends). The app decides: it may ask the user
/// to save first.
pub fn close_app(name: &str) -> Result<usize, String> {
    let windows = app_windows(name, &running::windows());
    for &window in &windows {
        running::ask_to_close(window)?;
    }
    Ok(windows.len())
}

/// The open window of the app with this name, if it has one (see `find_window`).
pub fn app_window(name: &str) -> Option<isize> {
    find_window(name, &running::windows())
}

#[cfg(windows)]
pub fn foreground_window() -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize }
}

#[cfg(not(windows))]
pub fn foreground_window() -> isize {
    0
}

/// Brings a window to the front (restored first when minimized).
#[cfg(windows)]
pub fn focus_window(handle: isize) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow, SwitchToThisWindow};
    let hwnd = HWND(handle as *mut core::ffi::c_void);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        // Windows may refuse a background program the foreground; switching still works then.
        if !SetForegroundWindow(hwnd).as_bool() || GetForegroundWindow() != hwnd {
            SwitchToThisWindow(hwnd, true);
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn focus_window(_handle: isize) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
mod running {
    use super::TopWindow;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GW_OWNER, GetWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible};
    use windows::core::{BOOL, PWSTR};

    /// Visible top-level windows of other programs, front first.
    pub fn windows() -> Vec<TopWindow> {
        unsafe extern "system" fn each(hwnd: HWND, list: LPARAM) -> BOOL {
            // SAFETY: list points at the Vec below for the whole EnumWindows call.
            unsafe { (*(list.0 as *mut Vec<HWND>)).push(hwnd) };
            BOOL(1)
        }
        let mut handles: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut handles as *mut Vec<HWND> as isize));
        }
        let me = unsafe { GetCurrentProcessId() };
        handles
            .into_iter()
            .filter_map(|hwnd| unsafe {
                if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) || cloaked(hwnd) {
                    return None;
                }
                let len = GetWindowTextLengthW(hwnd);
                if len <= 0 {
                    return None;
                }
                let mut buf = vec![0u16; len as usize + 1];
                let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
                if pid == me {
                    return None;
                }
                Some(TopWindow { handle: hwnd.0 as isize, title: String::from_utf16_lossy(&buf[..n]), exe: exe_name(pid) })
            })
            .collect()
    }

    /// Windows that report themselves visible but are not shown: suspended store apps ("Windows Input Experience",
    /// a closed Calculator's frame), windows on another virtual desktop.
    fn cloaked(hwnd: HWND) -> bool {
        use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
        let mut value = 0u32;
        // SAFETY: writes at most 4 bytes into the local u32.
        let read = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut value as *mut u32 as *mut core::ffi::c_void, 4) };
        read.is_ok() && value != 0
    }

    pub fn ask_to_close(handle: isize) -> Result<(), String> {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        // SAFETY: posts a message to a window handle; a stale handle only makes the call fail.
        unsafe { PostMessageW(Some(HWND(handle as *mut core::ffi::c_void)), WM_CLOSE, WPARAM(0), LPARAM(0)) }.map_err(|e| format!("Could not ask the window to close: {e}"))
    }

    /// Minimized, or a window Alt+Tab leaves out (tool windows and overlays that never take the focus).
    pub fn minimized_or_overlay(handle: isize) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, IsIconic, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW};
        let hwnd = HWND(handle as *mut core::ffi::c_void);
        // SAFETY: reads the state and style of a window handle; a stale handle only gives 0.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            IsIconic(hwnd).as_bool() || style & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0
        }
    }

    fn exe_name(pid: u32) -> String {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
            let mut buf = [0u16; 512];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
            let _ = CloseHandle(process);
            if !ok {
                return String::new();
            }
            let path = String::from_utf16_lossy(&buf[..size as usize]).to_lowercase();
            path.rsplit(['\\', '/']).next().unwrap_or_default().trim_end_matches(".exe").to_string()
        }
    }
}

#[cfg(not(windows))]
mod running {
    pub fn windows() -> Vec<super::TopWindow> {
        Vec::new()
    }

    pub fn minimized_or_overlay(_handle: isize) -> bool {
        false
    }

    pub fn ask_to_close(_handle: isize) -> Result<(), String> {
        Err("Closing apps is not supported on this system.".into())
    }
}

#[cfg(windows)]
fn start_menu(name: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // The name travels base64-encoded so it can never become part of the command.
    let encoded = base64_utf8(name);
    let script = format!(
        "$requested=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}')); \
         $app=@(Get-StartApps | Where-Object {{ $_.Name -like \"*$requested*\" }} | Select-Object -First 1); \
         if ($app.Count -lt 1) {{ exit 3 }}; Start-Process explorer.exe ('shell:AppsFolder\\' + $app[0].AppID)"
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| format!("Could not search the Start menu: {e}"))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(3) => Err(format!("No installed app called \"{name}\" was found.")),
        _ => Err(format!("Starting \"{name}\" failed.")),
    }
}

#[cfg(not(windows))]
fn start_menu(name: &str) -> Result<(), String> {
    Err(format!("Starting \"{name}\" is not supported on this system yet."))
}

#[cfg(windows)]
fn base64_utf8(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_explorer_windows_folder_is_picked() {
        let listing = "657210\t::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\r\n132322\tC:\\Users\\minea\\Downloads\r\n99\t\\\\nas\\share\r\n";
        assert_eq!(folder_of(listing, 132322).as_deref(), Some("C:\\Users\\minea\\Downloads"));
        assert_eq!(folder_of(listing, 99).as_deref(), Some("\\\\nas\\share"));
        assert_eq!(folder_of(listing, 657210), None, "This PC is not a folder");
        assert_eq!(folder_of(listing, 5), None);
    }

    #[test]
    fn an_open_app_is_found_by_its_program_or_its_title() {
        let w = |handle, title: &str, exe: &str| TopWindow { handle, title: title.into(), exe: exe.into() };
        let open = [w(1, "Qixeveria - Cheerful", "spotify"), w(2, "Gmail - Vivaldi", "vivaldi"), w(3, "Calculator", "applicationframehost"), w(4, "notes.txt - Notepad", "notepad"), w(5, "Book1 - Excel", "excel")];
        assert_eq!(find_window("Spotify", &open), Some(1));
        assert_eq!(find_window("vivaldi", &open), Some(2));
        assert_eq!(find_window("Calculator", &open), Some(3));
        assert_eq!(find_window("notepad", &open), Some(4));
        assert_eq!(find_window("Microsoft Excel", &open), Some(5));
        assert_eq!(find_window("Telegram", &open), None);
        assert_eq!(find_window("File Explorer", &open), None, "the shell always runs: open a new window");
    }

    #[test]
    fn the_front_most_browser_window_is_found() {
        let w = |handle, title: &str, exe: &str| TopWindow { handle, title: title.into(), exe: exe.into() };
        let open = [w(1, "main.ts - Visual Studio Code", "code"), w(2, "Example Domain - Vivaldi", "vivaldi"), w(3, "Gmail - Google Chrome", "chrome")];
        let (window, browser, title) = browser_window(&open).unwrap();
        assert_eq!((window.handle, browser, title.as_str()), (2, "Vivaldi", "Example Domain"));
        let edge = [w(4, "A - B - Microsoft\u{200b} Edge", "msedge")];
        assert_eq!(browser_window(&edge).map(|(_, b, t)| (b, t)), Some(("Microsoft Edge", "A - B".to_string())));
        assert!(browser_window(&open[..1]).is_none());
        let firefox = [w(5, "Example Domain — Mozilla Firefox", "firefox")];
        assert_eq!(browser_window(&firefox).map(|(_, _, t)| t), Some("Example Domain".to_string()));
    }

    #[test]
    fn every_window_of_an_app_is_found_to_close() {
        let w = |handle, title: &str, exe: &str| TopWindow { handle, title: title.into(), exe: exe.into() };
        let open = [w(1, "a.txt - Notepad", "notepad"), w(2, "Spotify Free", "spotify"), w(3, "b.txt - Notepad", "notepad")];
        assert_eq!(app_windows("Notepad", &open), vec![1, 3]);
        assert_eq!(app_windows("spotify", &open), vec![2]);
        assert!(app_windows("Telegram", &open).is_empty());
        assert!(app_windows("File Explorer", &open).is_empty());
        assert!(app_windows("Program Manager", &[w(9, "Program Manager", "explorer")]).is_empty(), "never the desktop");
    }

    #[test]
    fn the_users_window_is_picked_like_alt_tab() {
        let w = |handle, title: &str, exe: &str| TopWindow { handle, title: title.into(), exe: exe.into() };
        let open = [w(1, "NVIDIA GeForce Overlay", "nvcontainer"), w(2, "Notes - Notepad", "notepad"), w(3, "Example - Vivaldi", "vivaldi"), w(9, "Program Manager", "explorer")];
        assert_eq!(first_user_window(&open, |w| w.handle == 1), Some(2), "the overlay is skipped");
        assert_eq!(first_user_window(&open, |w| w.handle <= 2), Some(3), "a minimized window is skipped");
        assert_eq!(first_user_window(&open, |w| w.handle != 9), None, "the desktop is not a window to work in");
    }

    /// Lists the real windows, so it only runs on request: cargo test -p anytask-control -- --ignored real_windows --nocapture
    #[test]
    #[ignore]
    fn real_windows_leave_out_hidden_store_apps() {
        let windows = top_windows();
        for w in &windows {
            println!("{} | {} | {}", w.handle, w.exe, w.title);
        }
        assert!(!windows.iter().any(|w| w.title == "Windows Input Experience"), "cloaked windows are left out");
        println!("user window: {:?}", user_window());
    }

    #[test]
    fn only_app_names_are_accepted() {
        assert_eq!(valid_name(" Spotify ").as_deref(), Some("Spotify"));
        assert!(valid_name("Microsoft Edge").is_some());
        assert!(valid_name("تلگرام").is_some());
        assert!(valid_name("notepad.exe").is_some(), "known programs");
        for bad in ["evil.exe", "run.bat", "C:\\Windows\\cmd", "../x", "Setup Wizard", "a; calc", "", "x."] {
            assert!(valid_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn spotify_tells_its_state_by_its_title() {
        let window = |exe: &str, title: &str| TopWindow { handle: 1, title: title.into(), exe: exe.into() };
        assert_eq!(spotify_from_title(&[window("spotify", "Spotify Free")]), Some((false, String::new(), String::new())));
        assert_eq!(spotify_from_title(&[window("spotify", "Kourosh Yaghmaei - Gole Yakh")]), Some((true, "Kourosh Yaghmaei".into(), "Gole Yakh".into())));
        assert_eq!(spotify_from_title(&[window("msedge", "Spotify - Web Player")]), None, "only the Spotify app");
        assert_eq!(spotify_from_title(&[]), None);
    }

    #[test]
    #[cfg(windows)]
    fn names_are_encoded_safely() {
        assert_eq!(base64_utf8("Spotify"), "U3BvdGlmeQ==");
        assert_eq!(base64_utf8("ab"), "YWI=");
        assert_eq!(base64_utf8("abc"), "YWJj");
    }
}
