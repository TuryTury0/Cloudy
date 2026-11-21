//! Mouse and keyboard through Windows SendInput. Positions are 0..1000 of the primary screen.

use anytask_core::tools::desktop::MouseButton;

/// A parsed key combination: modifiers plus one key, as virtual-key codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub modifiers: Vec<u16>,
    pub key: u16,
}

const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_LWIN: u16 = 0x5B;
const VK_RETURN: u16 = 0x0D;

fn named_key(name: &str) -> Option<u16> {
    Some(match name {
        "enter" | "return" => VK_RETURN,
        "tab" => 0x09,
        "escape" | "esc" => 0x1B,
        "space" => 0x20,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "page_up" | "pgup" => 0x21,
        "pagedown" | "page_down" | "pgdn" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "media_play_pause" | "play_pause" => 0xB3,
        "media_next" => 0xB0,
        "media_prev" | "media_previous" => 0xB1,
        "media_stop" => 0xB2,
        "volume_up" => 0xAF,
        "volume_down" => 0xAE,
        "volume_mute" | "mute" => 0xAD,
        "win" | "windows" | "meta" | "super" => VK_LWIN,
        "plus" => 0xBB,
        "minus" => 0xBD,
        "comma" => 0xBC,
        "period" | "dot" => 0xBE,
        "slash" => 0xBF,
        "backslash" => 0xDC,
        "semicolon" => 0xBA,
        "quote" => 0xDE,
        "backquote" | "grave" => 0xC0,
        "bracketleft" => 0xDB,
        "bracketright" => 0xDD,
        _ => {
            if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
                && (1..=24).contains(&n)
            {
                return Some(0x70 + n - 1);
            }
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u16,
                (Some(c), None) => punctuation(c)?,
                _ => return None,
            }
        }
    })
}

fn punctuation(c: char) -> Option<u16> {
    Some(match c {
        '=' | '+' => 0xBB,
        '-' => 0xBD,
        ',' => 0xBC,
        '.' => 0xBE,
        '/' => 0xBF,
        '\\' => 0xDC,
        ';' => 0xBA,
        '\'' => 0xDE,
        '`' => 0xC0,
        '[' => 0xDB,
        ']' => 0xDD,
        _ => return None,
    })
}

/// Parses "ctrl+shift+t", "alt+left", "enter", "media_play_pause", "win+d".
pub fn parse_combo(text: &str) -> Result<Combo, String> {
    let text = text.trim().to_lowercase().replace(' ', "");
    let parts: Vec<&str> = text.split('+').filter(|p| !p.is_empty()).collect();
    let (last, mods) = parts.split_last().ok_or("Empty key.")?;
    let mut modifiers = Vec::new();
    for m in mods {
        modifiers.push(match *m {
            "ctrl" | "control" => VK_CONTROL,
            "shift" => VK_SHIFT,
            "alt" | "option" => VK_MENU,
            "win" | "windows" | "meta" | "super" | "cmd" => VK_LWIN,
            other => return Err(format!("Unknown modifier \"{other}\".")),
        });
    }
    let key = named_key(last).ok_or(format!("Unknown key \"{last}\"."))?;
    Ok(Combo { modifiers, key })
}

/// Text is pasted, which is fast and exact in every app: key-by-key typing comes out garbled in some (Windows 11
/// Notepad repeats and drops letters of fast key events: "emrooz kar mikone" became "oooooz iiiiiikone").
/// Only a key or two, or a short plain search in the Start menu (which reacts to each key), is typed.
pub fn should_paste(text: &str, window: &str) -> bool {
    let plain = text.chars().count() <= 40 && !text.contains('\n') && text.chars().all(|c| (' '..='~').contains(&c));
    let search = matches!(window.trim(), "Search" | "Start");
    !(plain && (text.chars().count() <= 2 || search))
}

#[cfg(windows)]
mod os {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
        MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY,
    };

    fn send(inputs: &[INPUT]) -> Result<(), String> {
        // SAFETY: the inputs are fully initialised and the size matches INPUT.
        let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() { Ok(()) } else { Err("Windows blocked the input (a window with higher rights may be in front).".into()) }
    }

    fn mouse(flags: MOUSE_EVENT_FLAGS, x: i32, y: i32, data: i32) -> INPUT {
        INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: x, dy: y, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
    }

    fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
    }

    fn extended(vk: u16) -> KEYBD_EVENT_FLAGS {
        // Arrows, navigation keys and media keys need the extended flag.
        if matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0xAD..=0xB3) { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) }
    }

    /// 0..1000 (with fractions) to Windows' absolute 0..65535: about 30 steps per screen pixel, so an exact point
    /// stays exact.
    fn to_absolute(v: f32) -> i32 {
        (v.clamp(0.0, 1000.0) * 65535.0 / 1000.0).round() as i32
    }

    pub fn move_to(x: u16, y: u16) -> Result<(), String> {
        move_exact(f32::from(x), f32::from(y))
    }

    fn move_exact(x: f32, y: f32) -> Result<(), String> {
        send(&[mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, to_absolute(x), to_absolute(y), 0)])
    }

    pub fn click(x: u16, y: u16, button: MouseButton, count: u8) -> Result<(), String> {
        click_exact(f32::from(x), f32::from(y), button, count)
    }

    /// A click on an exact point, 0..1000 of the primary screen with fractions (a control's own middle).
    pub fn click_exact(x: f32, y: f32, button: MouseButton, count: u8) -> Result<(), String> {
        move_exact(x, y)?;
        sleep(Duration::from_millis(40));
        let (down, up) = match button {
            MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        };
        for i in 0..count.max(1) {
            send(&[mouse(down, 0, 0, 0), mouse(up, 0, 0, 0)])?;
            if i + 1 < count {
                sleep(Duration::from_millis(60));
            }
        }
        Ok(())
    }

    /// Where the pointer is, 0..1000 of the primary screen (the phone draws it on its screen view).
    pub fn cursor() -> Option<(u16, u16)> {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let mut p = POINT::default();
        // SAFETY: writes the pointer position into a local value; reads screen metrics.
        unsafe {
            GetCursorPos(&mut p).ok()?;
            let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            if w <= 0 || h <= 0 {
                return None;
            }
            Some(((p.x.clamp(0, w) * 1000 / w) as u16, (p.y.clamp(0, h) * 1000 / h) as u16))
        }
    }

    /// Moves the pointer by (dx, dy) pixels from where it is: the phone's touchpad.
    pub fn move_by(dx: i32, dy: i32) -> Result<(), String> {
        send(&[mouse(MOUSEEVENTF_MOVE, dx, dy, 0)])
    }

    /// Presses or releases a button where the pointer is (a tap is both; holding is a drag).
    pub fn button(button: MouseButton, down: bool) -> Result<(), String> {
        let flag = match (button, down) {
            (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
            (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
            (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
            (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        };
        send(&[mouse(flag, 0, 0, 0)])
    }

    /// Turns the wheel where the pointer is: positive is up (or right), 120 is one notch.
    pub fn wheel(delta: i32, horizontal: bool) -> Result<(), String> {
        send(&[mouse(if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL }, 0, 0, delta.clamp(-2400, 2400))])
    }

    pub fn scroll(x: u16, y: u16, up: bool) -> Result<(), String> {
        move_to(x, y)?;
        sleep(Duration::from_millis(30));
        // Five notches: about a third of a page in most apps.
        send(&[mouse(MOUSEEVENTF_WHEEL, 0, 0, if up { 600 } else { -600 })])
    }

    pub fn press(combo: &Combo) -> Result<(), String> {
        let mut inputs = Vec::new();
        for m in &combo.modifiers {
            inputs.push(key(*m, 0, extended(*m)));
        }
        inputs.push(key(combo.key, 0, extended(combo.key)));
        inputs.push(key(combo.key, 0, extended(combo.key) | KEYEVENTF_KEYUP));
        for m in combo.modifiers.iter().rev() {
            inputs.push(key(*m, 0, extended(*m) | KEYEVENTF_KEYUP));
        }
        send(&inputs)
    }

    /// Types text as Unicode key events (any language); `\n` becomes Enter.
    pub fn type_keys(text: &str) -> Result<(), String> {
        for line in text.split('\n').enumerate() {
            if line.0 > 0 {
                press(&Combo { modifiers: vec![], key: VK_RETURN })?;
            }
            for unit in line.1.encode_utf16() {
                send(&[key(0, unit, KEYEVENTF_UNICODE), key(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)])?;
                sleep(Duration::from_millis(8));
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod os {
    use super::*;
    const NO: &str = "Desktop control is not supported on this system yet.";
    pub fn click(_: u16, _: u16, _: MouseButton, _: u8) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn scroll(_: u16, _: u16, _: bool) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn click_exact(_: f32, _: f32, _: MouseButton, _: u8) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn move_by(_: i32, _: i32) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn cursor() -> Option<(u16, u16)> {
        None
    }
    pub fn button(_: MouseButton, _: bool) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn wheel(_: i32, _: bool) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn press(_: &Combo) -> Result<(), String> {
        Err(NO.into())
    }
    pub fn type_keys(_: &str) -> Result<(), String> {
        Err(NO.into())
    }
}

pub use os::{button, click, click_exact, cursor, move_by, press, scroll, type_keys, wheel};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_combinations() {
        assert_eq!(parse_combo("Ctrl+L").unwrap(), Combo { modifiers: vec![VK_CONTROL], key: b'L' as u16 });
        assert_eq!(parse_combo("ctrl + shift + tab").unwrap(), Combo { modifiers: vec![VK_CONTROL, VK_SHIFT], key: 0x09 });
        assert_eq!(parse_combo("alt+left").unwrap().key, 0x25);
        assert_eq!(parse_combo("f5").unwrap().key, 0x74);
        assert_eq!(parse_combo("media_play_pause").unwrap().key, 0xB3);
        assert_eq!(parse_combo("win+d").unwrap(), Combo { modifiers: vec![VK_LWIN], key: b'D' as u16 });
        assert_eq!(parse_combo("ctrl+=").unwrap().key, 0xBB);
        assert!(parse_combo("hyper+x").is_err());
        assert!(parse_combo("ctrl+nothing").is_err());
    }

    #[test]
    fn long_or_non_english_text_is_pasted() {
        assert!(should_paste("test-e AnyTask emrooz kar mikone", "Untitled - Notepad"), "Notepad garbles typed keys");
        assert!(!should_paste("notepad", "Search"), "the Start search reacts to each key");
        assert!(!should_paste("y", "Untitled - Notepad"));
        assert!(should_paste("سلام", "Search"));
        assert!(should_paste("line1\nline2", "Search"));
        assert!(should_paste(&"a".repeat(41), "Search"));
    }
}
