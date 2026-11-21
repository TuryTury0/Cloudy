//! The controls of the foreground window, from Windows UI Automation: exact positions and names, so the model names
//! the control it means and the click lands on its exact middle instead of a guessed pixel. Optional: if UI
//! Automation is slow or fails, the agent works from the screenshot alone.

use anytask_core::tools::desktop::Element;
use std::time::Duration;

pub struct Window {
    pub title: String,
    /// A Chromium window (browser, Spotify, Electron app).
    pub chromium: bool,
    pub elements: Vec<Element>,
}

const LIMIT: usize = 80;
/// At most this many lines of plain text (headings, labels, values) join the controls. They have their own budget,
/// so a page full of words cannot push the buttons out.
pub const TEXT_LIMIT: usize = 40;
/// When there are too many text lines, the ones up to this long (headings, labels) are kept before paragraphs.
const SHORT_TEXT: usize = 40;

/// Kind names worth listing (whole-window panes are left out). Plain text is read too, as "text": headings and
/// labels tell what the controls around them belong to.
fn kind_name(id: i32) -> Option<&'static str> {
    // UIA control type ids (UIA_*ControlTypeId).
    Some(match id {
        50000 => "button",
        50002 => "checkbox",
        50003 => "combobox",
        50004 => "edit",
        50005 => "hyperlink",
        50007 => "listitem",
        50011 => "menuitem",
        50013 => "radiobutton",
        50015 => "slider",
        50019 => "tabitem",
        50020 => "text",
        50024 => "treeitem",
        50029 => "dataitem",
        50031 => "splitbutton",
        _ => return None,
    })
}

/// Turns screen rectangles into 0..1000 elements: off-screen, empty and whole-screen ones are dropped. The controls
/// come in reading order, then the text lines after them. Each keeps its exact middle from its pixel rectangle.
pub fn normalize(raw: Vec<RawControl>, screen: (i32, i32)) -> Vec<Element> {
    let (sw, sh) = (screen.0.max(1) as f32, screen.1.max(1) as f32);
    let scale = |v: i32, size: f32| (v as f32 / size * 1000.0).round().clamp(0.0, 1000.0) as u16;
    let mut out: Vec<Element> = raw
        .into_iter()
        .filter_map(|(name, kind, enabled, [l, t, r, b])| {
            let kind = kind_name(kind)?;
            let (l, t, r, b) = (l.max(0), t.max(0), r.min(screen.0), b.min(screen.1));
            if r - l < 4 || b - t < 4 {
                return None;
            }
            let (x, y) = (scale(l, sw), scale(t, sh));
            let (w, h) = (scale(r, sw).saturating_sub(x).max(1), scale(b, sh).saturating_sub(y).max(1));
            if w > 900 && h > 900 {
                return None;
            }
            let mid = ((l + r) as f32 / 2.0 / sw * 1000.0, (t + b) as f32 / 2.0 / sh * 1000.0);
            let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
            Some(Element { mid: Some(mid), ..Element::new(kind, &name, enabled, (x, y, w, h)) })
        })
        .collect();
    out.sort_by_key(|e| (e.y / 20, e.x));
    let (texts, mut controls): (Vec<Element>, Vec<Element>) = out.into_iter().partition(|e| e.kind == "text");
    // Lists report each row and every cell of it with the same rectangle; one box per spot is enough.
    let mut seen = std::collections::HashSet::new();
    controls.retain(|e| seen.insert((e.x, e.y, e.w, e.h)));
    drop_cells(&mut controls);
    cut(&mut controls, LIMIT);
    let texts = labels(texts, &controls, TEXT_LIMIT);
    controls.extend(texts);
    controls
}

/// A table row's cells: items of the row's own kind inside a bigger named row ("#", "Title", "36,652" inside
/// Spotify's song row, whose name already holds all of them). They only repeat the row in pieces and push useful
/// controls past the limit; the row is what is clicked (a double click plays or opens it). Buttons and links
/// inside a row are other kinds and stay.
fn drop_cells(items: &mut Vec<Element>) {
    const ROWS: &[&str] = &["listitem", "dataitem", "treeitem"];
    let area = |e: &Element| u32::from(e.w) * u32::from(e.h);
    let cells: std::collections::HashSet<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, e)| ROWS.contains(&e.kind.as_str()))
        .filter(|(_, e)| {
            items.iter().any(|r| {
                r.kind == e.kind && !r.name.is_empty() && area(r) > area(e) && r.x <= e.x && r.y <= e.y && e.x + e.w <= r.x + r.w && e.y + e.h <= r.y + r.h
            })
        })
        .map(|(i, _)| i)
        .collect();
    let keep = (0..items.len()).filter(|i| !cells.contains(i)).collect();
    keep_only(items, &keep);
}

/// Keeps the items whose index `keep` lets through, in their order.
fn keep_only(items: &mut Vec<Element>, keep: &std::collections::HashSet<usize>) {
    let mut i = 0;
    items.retain(|_| {
        i += 1;
        keep.contains(&(i - 1))
    });
}

/// Cuts the controls to `limit`, still in reading order. Kept first: the items of an open menu, which are what the next
/// click is for (11-11: File Explorer's Sort menu lost "Descending" past the 80th control, so the model clicked
/// "Ascending"); then named controls; then the buttons and links inside a named list row (the row itself is kept and
/// opens or plays with a double click); unnamed ones last. A cut by position alone dropped the bottom of the screen
/// (11-11: Spotify's open playlist filled all 80 places with the top of the page, so the last rows and the
/// "Recommended" heading under them never reached the model).
fn cut(out: &mut Vec<Element>, limit: usize) {
    if out.len() <= limit {
        return;
    }
    let rows: Vec<&Element> = out.iter().filter(|e| matches!(e.kind.as_str(), "listitem" | "dataitem" | "treeitem") && !e.name.is_empty()).collect();
    let in_row = |e: &Element| {
        let (x, y) = e.center();
        let area = u32::from(e.w) * u32::from(e.h);
        rows.iter().any(|r| r.x <= x && x <= r.x + r.w && r.y <= y && y <= r.y + r.h && u32::from(r.w) * u32::from(r.h) > area)
    };
    let rank = |e: &Element| {
        if e.kind == "menuitem" {
            0
        } else if e.name.is_empty() {
            3
        } else if in_row(e) {
            2
        } else {
            1
        }
    };
    let mut order: Vec<usize> = (0..out.len()).collect();
    order.sort_by_key(|&i| (rank(&out[i]), i));
    let keep = order.into_iter().take(limit).collect();
    keep_only(out, &keep);
}

/// The words on the screen that no control says already: headings ("Recommended", "Made for you"), labels and
/// values. They tell what a control belongs to (11-11: below a playlist's last song Spotify shows "Recommended" and
/// more songs; without the heading the model played one of those as the playlist's last). Left out as noise: a text
/// inside a named control (its own label), words some control is already named (Spotify's "Now playing: ..." twice),
/// the same words on the same line again, copyright lines and marks without a letter or digit. When there are too
/// many, short ones (headings, labels) are kept before long ones (paragraphs), still in reading order.
pub fn labels(texts: Vec<Element>, controls: &[Element], limit: usize) -> Vec<Element> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Element> = texts
        .into_iter()
        .filter(|t| !noise(&t.name, controls) && !inside_named(controls, t.center()) && seen.insert((t.name.to_lowercase(), t.y / 20)))
        .collect();
    if out.len() > limit {
        let mut order: Vec<usize> = (0..out.len()).collect();
        order.sort_by_key(|&i| (out[i].name.chars().count() > SHORT_TEXT, i));
        let keep = order.into_iter().take(limit).collect();
        keep_only(&mut out, &keep);
    }
    out
}

/// Text that tells the model nothing: shorter than two characters, no letter or digit, a copyright line, or words a
/// control is already named.
pub fn noise(text: &str, elements: &[Element]) -> bool {
    let text = text.trim();
    text.chars().count() < 2
        || !text.chars().any(char::is_alphanumeric)
        || text.starts_with(['\u{a9}', '\u{2117}'])
        || elements.iter().any(|e| e.kind != "text" && e.name.trim().eq_ignore_ascii_case(text))
}

/// Whether a point lies inside a named element that is not a whole page: words there are its own label.
pub fn inside_named(elements: &[Element], (x, y): (u16, u16)) -> bool {
    elements.iter().any(|e| !e.name.is_empty() && e.w < 900 && e.x <= x && x <= e.x + e.w && e.y <= y && y <= e.y + e.h)
}

/// Reads the foreground window; gives up after `timeout` (a hung app must not stall the agent).
pub fn foreground(screen: (i32, i32), timeout: Duration) -> Window {
    window(foreground_handle(), screen, timeout)
}

/// Reads one window, in front or not (its controls are read where they are; nothing is focused or moved).
pub fn window(hwnd: isize, screen: (i32, i32), timeout: Duration) -> Window {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read_window(hwnd, false));
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok((title, chromium, raw))) => Window { title, chromium, elements: normalize(raw, screen) },
        Ok(Err((title, chromium))) => Window { title, chromium, elements: Vec::new() },
        Err(_) => Window { title: String::new(), chromium: false, elements: Vec::new() },
    }
}

/// A control as UI Automation reports it: name, control type id, enabled, screen rectangle.
type RawControl = (String, i32, bool, [i32; 4]);
/// Title, whether it is a Chromium window, and its controls.
type Raw = (String, bool, Vec<RawControl>);

/// Title of the window in front.
#[cfg(windows)]
pub fn active_title() -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    // SAFETY: reads the text of the foreground window into a local buffer.
    unsafe {
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(GetForegroundWindow(), &mut buf);
        String::from_utf16_lossy(&buf[..len.max(0) as usize])
    }
}

#[cfg(not(windows))]
pub fn active_title() -> String {
    String::new()
}

/// Every on-screen control of a window with its UI Automation type id, also the kinds that are left out: for checking
/// what an app exposes (examples/uia_dump.rs).
pub fn raw_controls(hwnd: isize) -> Vec<(String, i32, bool, [i32; 4])> {
    read_window(hwnd, true).map(|(_, _, raw)| raw).unwrap_or_default()
}

/// The names of a window's on-screen buttons, in the order of its page (a Chromium app lists its page top to bottom).
pub fn buttons(hwnd: isize) -> Vec<String> {
    read_window(hwnd, true)
        .map(|(_, _, raw)| raw.into_iter().filter(|(name, kind, _, _)| *kind == 50000 && !name.trim().is_empty()).map(|(name, ..)| name).collect())
        .unwrap_or_default()
}

/// Presses the first on-screen button named exactly `name` in a window. UI Automation's Invoke presses it without the
/// mouse and with the window wherever it is (Chromium apps such as Spotify allow it); a button that cannot be invoked
/// is clicked in its middle after its window comes to the front.
#[cfg(windows)]
pub fn press(hwnd: isize, name: &str) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Foundation::VARIANT_FALSE;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::Variant::{VARIANT, VT_BOOL};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationInvokePattern, TreeScope_Descendants, UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId,
        UIA_InvokePatternId, UIA_IsOffscreenPropertyId, UIA_NamePropertyId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

    let wanted = name.trim().to_string();
    // COM for UI Automation runs on its own thread, as for reading.
    let found = std::thread::spawn(move || -> Result<Option<[i32; 4]>, String> {
        // SAFETY: COM is initialised for this thread; every interface is used on it and released by drop.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).map_err(|e| e.to_string())?;
            let root = uia.ElementFromHandle(HWND(hwnd as *mut std::ffi::c_void)).map_err(|e| e.to_string())?;
            let cache = uia.CreateCacheRequest().map_err(|e| e.to_string())?;
            for property in [UIA_NamePropertyId, UIA_ControlTypePropertyId, UIA_BoundingRectanglePropertyId] {
                cache.AddProperty(property).map_err(|e| e.to_string())?;
            }
            let mut not_offscreen = VARIANT::default();
            (*not_offscreen.Anonymous.Anonymous).vt = VT_BOOL;
            (*not_offscreen.Anonymous.Anonymous).Anonymous.boolVal = VARIANT_FALSE;
            let on_screen = uia.CreatePropertyCondition(UIA_IsOffscreenPropertyId, &not_offscreen).map_err(|e| e.to_string())?;
            let all = root.FindAllBuildCache(TreeScope_Descendants, &on_screen, &cache).map_err(|e| e.to_string())?;
            for i in 0..all.Length().map_err(|e| e.to_string())? {
                let Ok(el) = all.GetElement(i) else { continue };
                if el.CachedControlType().map(|t| t.0) != Ok(50000) || el.CachedName().map(|n| n.to_string().trim().to_string()).ok().as_deref() != Some(wanted.as_str()) {
                    continue;
                }
                if let Ok(invoke) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    && invoke.Invoke().is_ok()
                {
                    return Ok(None);
                }
                let r = el.CachedBoundingRectangle().map_err(|e| e.to_string())?;
                return Ok(Some([r.left, r.top, r.right, r.bottom]));
            }
            Err(format!("no button named \"{wanted}\""))
        }
    })
    .join()
    .map_err(|_| "UI Automation stopped".to_string())??;
    let Some([l, t, r, b]) = found else { return Ok(()) };
    crate::apps::focus_window(hwnd)?;
    std::thread::sleep(Duration::from_millis(150));
    // SAFETY: reads system metrics only.
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN).max(1), GetSystemMetrics(SM_CYSCREEN).max(1)) };
    // The button's exact middle, to the pixel.
    let x = (l + r) as f32 / 2.0 * 1000.0 / sw as f32;
    let y = (t + b) as f32 / 2.0 * 1000.0 / sh as f32;
    crate::input::click_exact(x, y, anytask_core::tools::desktop::MouseButton::Left, 1)
}

#[cfg(not(windows))]
pub fn press(_hwnd: isize, _name: &str) -> Result<(), String> {
    Err("Pressing buttons is not supported on this system yet.".into())
}

#[cfg(windows)]
fn foreground_handle() -> isize {
    // SAFETY: returns a handle value only.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize }
}

#[cfg(not(windows))]
fn foreground_handle() -> isize {
    0
}

#[cfg(windows)]
fn read_window(hwnd: isize, every_kind: bool) -> Result<Raw, (String, bool)> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_IsEnabledPropertyId,
        UIA_IsOffscreenPropertyId, UIA_NamePropertyId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowTextW};
    use windows::Win32::Foundation::VARIANT_FALSE;
    use windows::Win32::System::Variant::{VARIANT, VT_BOOL};

    // SAFETY: COM is initialised for this (dedicated) thread; all interfaces are used on it and released by drop.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let hwnd = HWND(hwnd as *mut std::ffi::c_void);
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        let title = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
        let mut class = [0u16; 128];
        let class_len = GetClassNameW(hwnd, &mut class);
        let chromium = String::from_utf16_lossy(&class[..class_len.max(0) as usize]).starts_with("Chrome_WidgetWin");
        let run = || -> windows::core::Result<Vec<RawControl>> {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = uia.ElementFromHandle(hwnd)?;
            let cache = uia.CreateCacheRequest()?;
            for property in [UIA_NamePropertyId, UIA_ControlTypePropertyId, UIA_BoundingRectanglePropertyId, UIA_IsEnabledPropertyId] {
                cache.AddProperty(property)?;
            }
            let mut not_offscreen = VARIANT::default();
            (*not_offscreen.Anonymous.Anonymous).vt = VT_BOOL;
            (*not_offscreen.Anonymous.Anonymous).Anonymous.boolVal = VARIANT_FALSE;
            let on_screen = uia.CreatePropertyCondition(UIA_IsOffscreenPropertyId, &not_offscreen)?;
            let found = root.FindAllBuildCache(TreeScope_Descendants, &on_screen, &cache)?;
            let mut out = Vec::new();
            for i in 0..found.Length()? {
                let el = found.GetElement(i)?;
                let kind = el.CachedControlType()?.0;
                if kind_name(kind).is_none() && !every_kind {
                    continue;
                }
                let r = el.CachedBoundingRectangle()?;
                let name = el.CachedName().map(|b| b.to_string()).unwrap_or_default();
                let enabled = el.CachedIsEnabled().map(|b| b.as_bool()).unwrap_or(true);
                out.push((name, kind, enabled, [r.left, r.top, r.right, r.bottom]));
            }
            Ok(out)
        };
        run().map(|raw| (title.clone(), chromium, raw)).map_err(|_| (title, chromium))
    }
}

#[cfg(not(windows))]
fn read_window(_hwnd: isize, _every_kind: bool) -> Result<Raw, (String, bool)> {
    Err((String::new(), false))
}

/// An edit control as UI Automation reports it: name, automation id, value.
type RawEdit = (String, String, String);

/// What a browser window's address bar shows. Chromium builds its accessibility tree only after a client asks, so
/// the first reads may find nothing (Vivaldi: no edit on the first try, the address bar on the next): ask again for
/// up to `patience`.
pub fn browser_address(window: isize, patience: Duration) -> Option<String> {
    let started = std::time::Instant::now();
    loop {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read_edits(window));
        });
        if let Ok(Ok(edits)) = rx.recv_timeout(Duration::from_secs(3))
            && let Some(address) = pick_address(&edits)
        {
            return Some(address);
        }
        if started.elapsed() >= patience {
            return None;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

/// The address among a window's edit fields: the one named like an address bar first (Chrome/Edge "Address and
/// search bar", Vivaldi "urlFieldInput", Firefox "urlbar-input"), else any field that holds an address.
pub fn pick_address(edits: &[RawEdit]) -> Option<String> {
    let named = |edit: &&RawEdit| {
        let label = format!("{} {}", edit.0, edit.1).to_lowercase();
        ["address", "url", "location", "آدرس", "نشانی"].iter().any(|k| label.contains(k))
    };
    edits.iter().filter(named).chain(edits.iter()).find_map(|edit| as_address(&edit.2))
}

/// A value as a full address, or None when it is not one (empty, or words being typed as a search). Chrome and Edge
/// hide "https://" in the bar ("example.com/page"), so it is put back.
pub fn as_address(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return None;
    }
    if value.contains("://") || value.starts_with("about:") {
        return Some(value.to_string());
    }
    let host = value.split(['/', '?', '#']).next().unwrap_or_default();
    (host.contains('.') && !host.starts_with('.') && !host.ends_with('.')).then(|| format!("https://{value}"))
}

#[cfg(windows)]
fn read_edits(window: isize) -> Result<Vec<RawEdit>, ()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::Variant::{VARIANT, VT_I4};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ControlTypePropertyId,
        UIA_NamePropertyId, UIA_ValuePatternId,
    };

    // SAFETY: COM is initialised for this (dedicated) thread; all interfaces are used on it and released by drop.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let run = || -> windows::core::Result<Vec<RawEdit>> {
            let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
            let root = uia.ElementFromHandle(HWND(window as *mut std::ffi::c_void))?;
            let cache = uia.CreateCacheRequest()?;
            cache.AddProperty(UIA_NamePropertyId)?;
            cache.AddProperty(UIA_AutomationIdPropertyId)?;
            let mut edit = VARIANT::default();
            (*edit.Anonymous.Anonymous).vt = VT_I4;
            (*edit.Anonymous.Anonymous).Anonymous.lVal = 50004;
            let is_edit = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &edit)?;
            let read = |el: &windows::Win32::UI::Accessibility::IUIAutomationElement| -> RawEdit {
                let name = el.CachedName().map(|b| b.to_string()).unwrap_or_default();
                let id = el.CachedAutomationId().map(|b| b.to_string()).unwrap_or_default();
                let value = el
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .and_then(|p| p.CurrentValue())
                    .map(|b| b.to_string())
                    .unwrap_or_default();
                (name, id, value)
            };
            // The address bar is the first edit field in the tree (browser frame before the page): one quick search.
            // Walking every field of a heavy page (Gmail, Docs) could take seconds, so that is only the fallback.
            if let Ok(first) = root.FindFirstBuildCache(TreeScope_Descendants, &is_edit, &cache) {
                let first = read(&first);
                if pick_address(std::slice::from_ref(&first)).is_some() {
                    return Ok(vec![first]);
                }
            }
            let found = root.FindAllBuildCache(TreeScope_Descendants, &is_edit, &cache)?;
            let mut out = Vec::new();
            for i in 0..found.Length()?.min(12) {
                out.push(read(&found.GetElement(i)?));
            }
            Ok(out)
        };
        run().map_err(|_| ())
    }
}

#[cfg(not(windows))]
fn read_edits(_window: isize) -> Result<Vec<RawEdit>, ()> {
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_menu_survives_the_cut() {
        // 85 controls: a file list first, then the Sort menu's items lower down the order.
        let raw: Vec<RawControl> = (0..80)
            .map(|i| (format!("file {i}"), 50007, true, [10, 100 + i * 10, 400, 108 + i * 10]))
            .chain(["Name", "Date modified", "Type", "Ascending", "Descending"].iter().enumerate().map(|(i, n)| (n.to_string(), 50011, true, [500, 950 + i as i32 * 20, 700, 965 + i as i32 * 20])))
            .collect();
        let elements = normalize(raw, (1000, 2000));
        assert_eq!(elements.len(), LIMIT);
        assert!(elements.iter().any(|e| e.name == "Descending"), "every menu item is kept");
    }

    #[test]
    fn finds_the_address_among_edit_fields() {
        let edit = |name: &str, id: &str, value: &str| (name.to_string(), id.to_string(), value.to_string());
        // Vivaldi as read on this PC.
        let vivaldi = [edit("Search Google or enter an address", "urlFieldInput", "https://example.com")];
        assert_eq!(pick_address(&vivaldi).as_deref(), Some("https://example.com"));
        // Chrome hides the scheme; a search box on the page comes first but holds words.
        let chrome = [edit("Search", "q", "how to cook"), edit("Address and search bar", "", "news.ycombinator.com/item?id=1")];
        assert_eq!(pick_address(&chrome).as_deref(), Some("https://news.ycombinator.com/item?id=1"));
        // Words typed into the bar are not an address; nothing else holds one.
        assert_eq!(pick_address(&[edit("Address and search bar", "", "ghormeh sabzi")]), None);
        assert_eq!(as_address("localhost"), None);
        assert_eq!(as_address("vivaldi://settings").as_deref(), Some("vivaldi://settings"));
        assert_eq!(as_address(" divar.ir "), Some("https://divar.ir".to_string()));
    }

    #[test]
    fn keeps_useful_controls_in_reading_order() {
        let raw = vec![
            ("Play".to_string(), 50000, true, [960, 540, 1000, 580]),
            ("  Search \n box ".to_string(), 50004, true, [100, 100, 500, 140]),
            ("Static text".to_string(), 50020, true, [0, 0, 300, 20]),
            ("Whole window".to_string(), 50000, true, [0, 0, 1920, 1080]),
            ("Tiny".to_string(), 50000, true, [10, 10, 12, 12]),
            ("Off screen".to_string(), 50000, false, [-500, -500, -400, -450]),
            ("Row cell".to_string(), 50029, true, [100, 100, 500, 140]),
        ];
        let els = normalize(raw, (1920, 1080));
        assert_eq!(els.len(), 3);
        assert_eq!((els[0].kind.as_str(), els[0].name.as_str()), ("edit", "Search box"));
        assert_eq!((els[1].x, els[1].y), (500, 500));
        assert_eq!(els[1].center(), (510, 518));
        assert_eq!(els[1].middle(), (980.0 / 1920.0 * 1000.0, 560.0 / 1080.0 * 1000.0), "the exact middle of its pixels");
        assert_eq!((els[2].kind.as_str(), els[2].name.as_str()), ("text", "Static text"), "text comes after the controls");
    }

    #[test]
    fn headings_are_read_and_a_controls_own_label_is_not() {
        let raw = vec![
            ("Play Believer by Imagine Dragons".to_string(), 50000, true, [100, 100, 900, 140]),
            ("Believer".to_string(), 50020, true, [120, 110, 300, 130]),
            ("Recommended".to_string(), 50020, true, [100, 200, 400, 230]),
            ("Recommended".to_string(), 50020, true, [500, 200, 800, 230]),
            ("x".to_string(), 50020, true, [100, 300, 120, 320]),
            // Noise: a control's name again elsewhere, a copyright line, marks only.
            ("Play Believer by Imagine Dragons".to_string(), 50020, true, [100, 400, 500, 420]),
            ("\u{a9} 2017 KIDinaKORNER".to_string(), 50020, true, [100, 450, 500, 470]),
            ("--".to_string(), 50020, true, [100, 500, 500, 520]),
            ("4,140".to_string(), 50020, true, [100, 550, 300, 570]),
        ];
        let els = normalize(raw, (1000, 1000));
        let names: Vec<(&str, &str)> = els.iter().map(|e| (e.kind.as_str(), e.name.as_str())).collect();
        assert_eq!(names, [("button", "Play Believer by Imagine Dragons"), ("text", "Recommended"), ("text", "4,140")], "a value with digits stays");
    }

    #[test]
    fn a_rows_own_cells_are_left_out_and_its_buttons_stay() {
        // Spotify's song row as read on 11-13: the row, its cells (same kind, inside it) and its buttons.
        let raw = vec![
            ("1 Play Khodaye Asemoonha by Toofan Khodaye Asemoonha Toofan 36,652 4:26".to_string(), 50029, true, [273, 348, 794, 400]),
            ("1 Play Khodaye Asemoonha by Toofan".to_string(), 50029, true, [282, 349, 291, 399]),
            ("Khodaye Asemoonha Toofan".to_string(), 50029, true, [299, 349, 568, 399]),
            ("36,652".to_string(), 50029, true, [576, 349, 710, 399]),
            ("Add to Liked Songs".to_string(), 50000, true, [732, 359, 741, 389]),
            ("Liked Songs".to_string(), 50029, true, [99, 278, 252, 337]),
        ];
        let els = normalize(raw, (1000, 1000));
        let names: Vec<&str> = els.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Liked Songs", "1 Play Khodaye Asemoonha by Toofan Khodaye Asemoonha Toofan 36,652 4:26", "Add to Liked Songs"]);
    }

    #[test]
    fn the_cut_keeps_named_controls_before_unnamed_ones_and_short_text_before_long() {
        // 60 unnamed icons at the top, then 40 named rows down to the bottom of the screen.
        let raw: Vec<RawControl> = (0..60)
            .map(|i| (String::new(), 50000, true, [10 + i * 10, 10, 18 + i * 10, 18]))
            .chain((0..40).map(|i| (format!("song {i}"), 50007, true, [100, 100 + i * 20, 900, 118 + i * 20])))
            .collect();
        let els = normalize(raw, (1000, 1000));
        assert_eq!(els.len(), LIMIT);
        assert!(els.iter().any(|e| e.name == "song 39"), "the bottom row is kept");
        assert_eq!(els.iter().filter(|e| e.name.is_empty()).count(), LIMIT - 40);

        // A playlist: 30 rows down the screen, each with a play button, a like button and two links inside.
        let mut raw: Vec<RawControl> = Vec::new();
        for i in 0..30 {
            let top = 100 + i * 30;
            raw.push((format!("{} song {i}", i + 1), 50029, true, [100, top, 900, top + 28]));
            raw.push((format!("Play song {i}"), 50000, true, [105, top + 4, 125, top + 24]));
            raw.push(("Add to Liked Songs".into(), 50000, true, [700, top + 4, 720, top + 24]));
            raw.push((format!("Artist {i}"), 50005, true, [300, top + 4, 400, top + 24]));
            raw.push((format!("Album {i}"), 50005, true, [500, top + 4, 600, top + 24]));
        }
        let els = normalize(raw, (1000, 1000));
        assert!((0..30).all(|i| els.iter().any(|e| e.name == format!("{} song {i}", i + 1))), "every row is kept, down to the last");

        let long = "a long paragraph of text that goes on and on about many things at once";
        let texts: Vec<Element> = (0..TEXT_LIMIT as u16 + 5)
            .map(|i| Element::new("text", &if i < 10 { format!("{long} {i}") } else { format!("Heading {i}") }, true, (10, i * 20, 50, 10)))
            .collect();
        let kept = labels(texts, &[], TEXT_LIMIT);
        assert_eq!(kept.len(), TEXT_LIMIT);
        assert!(kept.iter().all(|t| t.name.starts_with("Heading") || t.name.ends_with(" 0") || t.name.ends_with(" 1") || t.name.ends_with(" 2") || t.name.ends_with(" 3") || t.name.ends_with(" 4")), "long lines go first");
        assert!(kept.windows(2).all(|w| w[0].y < w[1].y), "still in reading order");
    }
}
