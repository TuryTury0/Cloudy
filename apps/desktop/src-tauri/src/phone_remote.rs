//! What a paired phone can do on this PC besides talking to the assistant: use it as a touchpad and keyboard, watch
//! the screen, browse and copy files both ways, and shut down, restart, sleep or lock the PC.
//!
//! Only paired phones reach this (the Noise handshake checks their key); every message comes from one of them.
//! Files move in pieces the receiver asks for one at a time, so a big file never sits in memory whole and a lost
//! connection costs one piece.

use anytask_control::input;
use anytask_core::tools::desktop::MouseButton;
use anytask_core::paths::UserFolders;
use base64::Engine;
use serde_json::{Value, json};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// One piece of a file on the wire (before base64).
pub const PIECE: usize = 384 * 1024;
/// The phone's screen view: picture width and JPEG quality (small enough for a slow link, readable on a phone).
const SCREEN_WIDTH: u32 = 960;
const SCREEN_QUALITY: u8 = 55;
/// Seconds the app waits before asking Windows to shut down or restart, so a mistaken press can still be cancelled.
pub const POWER_DELAY: u64 = 20;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn button(value: &Value) -> MouseButton {
    if value.as_str() == Some("right") { MouseButton::Right } else { MouseButton::Left }
}

/// Touchpad and keyboard: {"op": "move", "dx", "dy"} | {"op": "button", "button", "down"} | {"op": "click", "button",
/// "count"} | {"op": "tap", "x", "y" (0..1000 of the screen), "button", "count"} | {"op": "wheel", "dy", "dx"} |
/// {"op": "text", "text"} | {"op": "key", "combo"}.
pub fn input(message: &Value) -> Result<(), String> {
    let num = |key: &str| message[key].as_f64().unwrap_or(0.0);
    match message["op"].as_str().unwrap_or_default() {
        "move" => input::move_by(num("dx").round().clamp(-4000.0, 4000.0) as i32, num("dy").round().clamp(-4000.0, 4000.0) as i32),
        "button" => input::button(button(&message["button"]), message["down"].as_bool().unwrap_or(false)),
        "click" => {
            let b = button(&message["button"]);
            for i in 0..message["count"].as_u64().unwrap_or(1).clamp(1, 3) {
                if i > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(60));
                }
                input::button(b, true)?;
                input::button(b, false)?;
            }
            Ok(())
        }
        "tap" => {
            let at = |key: &str| num(key).round().clamp(0.0, 1000.0) as u16;
            input::click(at("x"), at("y"), button(&message["button"]), message["count"].as_u64().unwrap_or(1).clamp(1, 3) as u8)
        }
        "wheel" => {
            let (dx, dy) = (num("dx").round() as i32, num("dy").round() as i32);
            if dy != 0 {
                input::wheel(dy, false)?;
            }
            if dx != 0 {
                input::wheel(dx, true)?;
            }
            Ok(())
        }
        "text" => {
            let text: String = message["text"].as_str().unwrap_or_default().chars().take(4000).collect();
            if text.is_empty() {
                return Ok(());
            }
            // What the phone's keyboard just typed: a few letters go in as key presses (any language); longer text
            // is pasted like the assistant does, which is exact in every app.
            if text.chars().count() <= 8 && !text.contains('\n') {
                input::type_keys(&text)
            } else {
                use anytask_core::tools::desktop::DesktopOps;
                anytask_control::NativeDesktop.type_text(&text)
            }
        }
        "key" => input::press(&input::parse_combo(message["combo"].as_str().unwrap_or_default())?),
        other => Err(format!("Unknown input \"{other}\".")),
    }
}

/// The screen as a JPEG for the phone's screen view, with where the pointer is (0..1000).
pub fn screen(width: Option<u64>) -> Result<Value, String> {
    let width = width.map_or(SCREEN_WIDTH, |w| (w as u32).clamp(320, 1600));
    let (image, _) = anytask_control::screen::capture_primary(width)?;
    let jpeg = anytask_control::marks::encode_jpeg(&image, SCREEN_QUALITY)?;
    let cursor = input::cursor();
    Ok(json!({ "type": "screen", "jpeg": b64(&jpeg), "w": image.width, "h": image.height, "cursor": cursor.map(|(x, y)| json!([x, y])) }))
}

/// Where browsing starts: the user's folders and the PC's drives (listed without touching them: an empty card
/// reader or a lost network drive would hang a check).
fn roots() -> Vec<(String, PathBuf)> {
    let f = UserFolders::detect();
    let mut out = vec![
        ("Desktop".to_string(), f.desktop),
        ("Downloads".to_string(), f.downloads),
        ("Documents".to_string(), f.documents),
        ("Pictures".to_string(), f.pictures),
        ("Videos".to_string(), f.videos),
        ("Music".to_string(), f.music),
    ];
    for letter in anytask_control::machine::drives() {
        out.push((format!("{letter}:"), PathBuf::from(format!("{letter}:\\"))));
    }
    out
}

fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A folder's content for the phone, folders first; no path: the starting places.
pub fn list(path: Option<&str>) -> Result<Value, String> {
    let Some(path) = path.map(str::trim).filter(|p| !p.is_empty()) else {
        let entries: Vec<Value> = roots().into_iter().map(|(name, p)| json!({ "name": name, "path": p.to_string_lossy(), "dir": true })).collect();
        return Ok(json!({ "type": "files", "path": "", "parent": Value::Null, "entries": entries }));
    };
    let dir = PathBuf::from(path);
    let read = std::fs::read_dir(&dir).map_err(|e| format!("Could not open {path}: {e}"))?;
    let mut entries: Vec<(bool, String, Value)> = Vec::new();
    let mut truncated = false;
    for (i, entry) in read.flatten().enumerate() {
        if i >= 2000 {
            truncated = true;
            break;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Hidden system entries are noise on a phone.
        if name.starts_with('.') || name.eq_ignore_ascii_case("desktop.ini") || name.starts_with("~$") || name.starts_with('$') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let is_dir = meta.is_dir();
        entries.push((
            !is_dir,
            name.to_lowercase(),
            json!({ "name": name, "path": entry.path().to_string_lossy(), "dir": is_dir, "size": if is_dir { 0 } else { meta.len() }, "modified": modified_ms(&meta) }),
        ));
    }
    entries.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let parent = dir.parent().map(|p| p.to_string_lossy().to_string());
    Ok(json!({ "type": "files", "path": dir.to_string_lossy(), "parent": parent, "truncated": truncated, "entries": entries.into_iter().map(|e| e.2).collect::<Vec<_>>() }))
}

/// One piece of a file, from `offset`.
pub fn piece(path: &str, offset: u64) -> Result<Value, String> {
    let file_path = Path::new(path);
    let mut file = std::fs::File::open(file_path).map_err(|e| format!("Could not open {path}: {e}"))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if file_path.is_dir() {
        return Err("That is a folder.".into());
    }
    file.seek(SeekFrom::Start(offset.min(size))).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; PIECE.min((size.saturating_sub(offset)) as usize)];
    file.read_exact(&mut buf).map_err(|e| format!("Could not read {path}: {e}"))?;
    let name = file_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
    let end = offset + buf.len() as u64;
    Ok(json!({ "type": "chunk", "path": path, "name": name, "size": size, "offset": offset, "data": b64(&buf), "last": end >= size }))
}

/// A file name the phone sent, made safe: no folders, no characters Windows refuses, never empty.
fn safe_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let cleaned: String = cleaned.trim().trim_matches('.').chars().take(150).collect();
    if cleaned.is_empty() {
        return "file from phone".into();
    }
    // Windows' device names (CON, NUL, COM1...) swallow what is written to them, with or without an extension.
    let stem = cleaned.split('.').next().unwrap_or_default().trim().to_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0');
    if device { format!("_{cleaned}") } else { cleaned }
}

/// Where a file from the phone goes: Downloads\AnyTask, never over an existing file.
fn target(folder: &Path, name: &str) -> PathBuf {
    let name = safe_name(name);
    let first = folder.join(&name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    (2..10_000).map(|n| folder.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// A file arriving from a phone: written to a ".part" file, named for real only when the last piece is in.
struct Arriving {
    part: PathBuf,
    name: String,
    written: u64,
}

/// Files arriving from phones, by the phone's key and the id it gave the file (two phones never share a file).
#[derive(Default)]
pub struct Uploads {
    open: std::sync::Mutex<std::collections::HashMap<(String, String), Arriving>>,
}

impl Uploads {
    /// Writes one piece: {"id", "name", "offset", "data", "last"}. The answer tells the phone the next offset, and
    /// where the file is once the last piece is in.
    pub fn piece(&self, device: &str, message: &Value, folder: &Path) -> Value {
        let id = message["id"].as_str().unwrap_or_default().to_string();
        let result = (|| -> Result<Value, String> {
            if id.is_empty() {
                return Err("A file without an id.".into());
            }
            let key = (device.to_string(), id.clone());
            let offset = message["offset"].as_u64().unwrap_or(0);
            let data = base64::engine::general_purpose::STANDARD.decode(message["data"].as_str().unwrap_or_default()).map_err(|_| "A damaged piece.")?;
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            if offset == 0 {
                std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
                if let Some(old) = open.remove(&key) {
                    let _ = std::fs::remove_file(old.part);
                }
                let name = safe_name(message["name"].as_str().unwrap_or_default());
                let tag: String = id.chars().filter(char::is_ascii_alphanumeric).take(16).collect();
                let part = folder.join(format!("{name}.{tag}.part"));
                std::fs::OpenOptions::new().write(true).create_new(true).open(&part).map_err(|e| format!("Could not save it: {e}"))?;
                open.insert(key.clone(), Arriving { part, name, written: 0 });
            }
            let arriving = open.get_mut(&key).ok_or("Send the file again from the start.")?;
            if offset != arriving.written {
                return Err(format!("Expected the piece at {}.", arriving.written));
            }
            let mut file = std::fs::OpenOptions::new().append(true).open(&arriving.part).map_err(|e| e.to_string())?;
            file.write_all(&data).map_err(|e| format!("Could not save it: {e}"))?;
            drop(file);
            arriving.written += data.len() as u64;
            let next = arriving.written;
            if !message["last"].as_bool().unwrap_or(false) {
                return Ok(json!({ "type": "uploaded", "id": id, "offset": next, "done": false, "path": Value::Null }));
            }
            let arriving = open.remove(&key).expect("present");
            let path = target(folder, &arriving.name);
            std::fs::rename(&arriving.part, &path).map_err(|e| format!("Could not save it: {e}"))?;
            Ok(json!({ "type": "uploaded", "id": id, "offset": next, "done": true, "path": path.to_string_lossy() }))
        })();
        result.unwrap_or_else(|e| json!({ "type": "uploaded", "id": id, "error": e }))
    }

    /// A phone went away: its unfinished files are removed.
    pub fn forget(&self, device: &str) {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        let gone: Vec<(String, String)> = open.keys().filter(|(d, _)| d == device).cloned().collect();
        for key in gone {
            if let Some(arriving) = open.remove(&key) {
                let _ = std::fs::remove_file(arriving.part);
            }
        }
    }
}

/// Writes one piece of a file the PC sends to the phone (chosen on the Phone page) under `folder/<id>/<name>`; the
/// whole file's path once the last piece is in. A piece must continue the file where it stands.
pub fn stage_piece(folder: &Path, id: &str, name: &str, offset: u64, data: &str, last: bool) -> Result<Option<PathBuf>, String> {
    let id: String = id.chars().filter(char::is_ascii_alphanumeric).take(32).collect();
    if id.is_empty() {
        return Err("A file without an id.".into());
    }
    let dir = folder.join(&id);
    if offset == 0 {
        // Copies of earlier sends: the phone fetched them within minutes; an hour later they only fill the disk
        // (this app runs for days without a restart).
        let hour = std::time::Duration::from_secs(3600);
        for old in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let stale = old.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > hour);
            if stale && old.path() != dir {
                let _ = std::fs::remove_dir_all(old.path());
            }
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(safe_name(name));
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|_| "A damaged piece.")?;
    if offset != 0 && std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) != offset {
        return Err("Send the file again from the start.".into());
    }
    let mut file = if offset == 0 { std::fs::File::create(&path) } else { std::fs::OpenOptions::new().append(true).open(&path) }.map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    Ok(last.then_some(path))
}

/// The program and arguments for a power action, run when it is due; None for an unknown one. Shut down and
/// restart have no timeout of their own (the app waited POWER_DELAY already): with a timeout Windows forces apps
/// closed and unsaved work is lost; without one an app can still ask to save. Sleep goes through the API (sleep()).
pub fn power_command(action: &str) -> Option<(&'static str, Vec<String>)> {
    Some(match action {
        "shutdown" => ("shutdown", vec!["/s".into(), "/t".into(), "0".into()]),
        "restart" => ("shutdown", vec!["/r".into(), "/t".into(), "0".into()]),
        "cancel" => ("shutdown", vec!["/a".into()]),
        "lock" => ("rundll32.exe", vec!["user32.dll,LockWorkStation".into()]),
        _ => return None,
    })
}

/// Runs a power action now (blocking; the caller waits POWER_DELAY first for shut down and restart).
pub fn power(action: &str) -> Result<String, String> {
    if action == "sleep" {
        return anytask_control::machine::sleep().map(|_| "Slept.".into());
    }
    let (program, args) = power_command(action).ok_or_else(|| format!("Unknown power action \"{action}\"."))?;
    let mut command = std::process::Command::new(program);
    command.args(&args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let status = command.status().map_err(|e| format!("Could not run {program}: {e}"))?;
    if !status.success() && action != "cancel" {
        return Err(format!("Windows refused ({status})."));
    }
    Ok(match action {
        "shutdown" => "Shutting down.".into(),
        "restart" => "Restarting.".into(),
        "cancel" => "Cancelled.".into(),
        _ => "Locked.".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_actions_wait_and_can_be_cancelled_and_unknown_ones_are_refused() {
        let (program, args) = power_command("shutdown").unwrap();
        assert_eq!(program, "shutdown");
        assert_eq!(args, ["/s", "/t", "0"], "no timeout: Windows would force apps closed and lose unsaved work");
        assert_eq!(power_command("restart").unwrap().1[0], "/r");
        assert_eq!(power_command("cancel").unwrap().1, ["/a"]);
        assert!(power_command("format c:").is_none());
        assert!(power("rm -rf").is_err());
    }

    #[test]
    fn names_from_the_phone_are_made_safe_and_never_overwrite() {
        assert_eq!(safe_name("../../Windows/evil.exe"), "evil.exe");
        assert_eq!(safe_name(r"C:\x\a<b>.txt"), "a_b_.txt");
        assert_eq!(safe_name(" .. "), "file from phone");
        assert_eq!(safe_name("NUL"), "_NUL");
        assert_eq!(safe_name("conout$.txt"), "_conout$.txt");
        assert_eq!(safe_name("com3.txt"), "_com3.txt");
        assert_eq!(safe_name("COM0.txt"), "COM0.txt");
        assert_eq!(safe_name("console.log"), "console.log");
        let dir = std::env::temp_dir().join(format!("anytask-upload-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let uploads = Uploads::default();
        let send = |id: &str, offset: u64, data: &[u8], last: bool| {
            uploads.piece("phone-1", &json!({ "id": id, "name": "photo.jpg", "offset": offset, "data": b64(data), "last": last }), &dir)
        };
        assert_eq!(send("a", 0, b"hello ", false)["offset"], 6);
        assert!(!dir.join("photo.jpg").exists(), "the real name appears only when the file is complete");
        assert!(send("a", 3, b"x", false)["error"].as_str().unwrap().contains("Expected"), "a piece out of order is refused");
        let done = send("a", 6, b"world", true);
        assert_eq!(done["done"], true);
        let first = PathBuf::from(done["path"].as_str().unwrap());
        assert_eq!(std::fs::read(&first).unwrap(), b"hello world");
        let second = send("b", 0, b"again", true);
        assert!(second["path"].as_str().unwrap().ends_with("photo (2).jpg"), "{second}");
        // A phone that goes away leaves no half file behind.
        assert_eq!(send("c", 0, b"half", false)["offset"], 4);
        uploads.forget("phone-1");
        let left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        assert!(left.iter().all(|n| !n.ends_with(".part")), "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_for_the_phone_is_staged_piece_by_piece() {
        let dir = std::env::temp_dir().join(format!("anytask-stage-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(stage_piece(&dir, "ab1", "../x/report.pdf", 0, &b64(b"part one "), false).unwrap(), None);
        assert!(stage_piece(&dir, "ab1", "report.pdf", 3, &b64(b"bad"), false).is_err(), "a piece that does not continue the file");
        let path = stage_piece(&dir, "ab1", "report.pdf", 9, &b64(b"part two"), true).unwrap().unwrap();
        assert_eq!(path, dir.join("ab1").join("report.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), b"part one part two");
        assert!(stage_piece(&dir, "../", "x", 0, "", true).is_err(), "the id is letters and digits only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_go_out_in_pieces_and_folders_list_folders_first() {
        let dir = std::env::temp_dir().join(format!("anytask-list-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("b folder")).unwrap();
        let data: Vec<u8> = (0..(PIECE + 10)).map(|i| (i % 251) as u8).collect();
        std::fs::write(dir.join("a.bin"), &data).unwrap();
        std::fs::write(dir.join("desktop.ini"), "x").unwrap();
        let listed = list(Some(&dir.to_string_lossy())).unwrap();
        let names: Vec<&str> = listed["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["b folder", "a.bin"]);
        let path = dir.join("a.bin").to_string_lossy().to_string();
        let first = piece(&path, 0).unwrap();
        assert_eq!((first["size"].as_u64(), first["last"].as_bool()), (Some(data.len() as u64), Some(false)));
        let second = piece(&path, PIECE as u64).unwrap();
        assert_eq!(second["last"], true);
        let back = base64::engine::general_purpose::STANDARD.decode(second["data"].as_str().unwrap()).unwrap();
        assert_eq!(back, &data[PIECE..]);
        assert!(list(None).unwrap()["entries"].as_array().unwrap().iter().any(|e| e["name"] == "Desktop"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
