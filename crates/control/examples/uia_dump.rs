//! Prints what UI Automation reports for an app's window, without bringing it to the front or touching it:
//! the count of each control type, and the controls the model would get.
//!   cargo run -p anytask-control --example uia_dump -- Spotify
//! ALL=1 also prints every raw control (type id, name, rectangle). LOOK=1 instead times three real looks at the
//! window in front (screenshot, controls and the words read from the screen; nothing is clicked or typed).

use anytask_control::{apps, uia};
use std::collections::BTreeMap;
use std::time::Duration;

fn main() {
    if std::env::var("LOOK").is_ok() {
        use anytask_core::tools::desktop::DesktopOps;
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let look = anytask_control::NativeDesktop.look().expect("look");
            let texts: Vec<&str> = look.elements.iter().filter(|e| e.kind == "text").map(|e| e.name.as_str()).collect();
            println!("look {} ms: {} controls, {} texts, \"{}\"", started.elapsed().as_millis(), look.elements.len() - texts.len(), texts.len(), look.title);
            println!("  texts: {}", texts.iter().take(12).copied().collect::<Vec<_>>().join(" | "));
        }
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let (full, _) = anytask_control::screen::capture_primary(u32::MAX).expect("capture");
            let captured = started.elapsed().as_millis();
            let lines = anytask_control::ocr::read(&full, Duration::from_secs(5)).map(|l| l.len()).unwrap_or(0);
            let screen = anytask_control::screen::capture_primary(64).map(|(_, size)| size).unwrap_or_default();
            let uia_started = std::time::Instant::now();
            let window = uia::foreground(screen, Duration::from_secs(4));
            println!(
                "{}x{}: capture {captured} ms, OCR {} ms ({lines} lines); UI Automation alone {} ms ({} elements)",
                full.width,
                full.height,
                uia_started.duration_since(started).as_millis() - captured,
                uia_started.elapsed().as_millis(),
                window.elements.len()
            );
        }
        return;
    }
    let app = std::env::args().nth(1).unwrap_or_else(|| "Spotify".into());
    let Some(hwnd) = apps::app_window(&app) else {
        eprintln!("no window of {app}");
        return;
    };
    let raw = uia::raw_controls(hwnd);
    let mut kinds: BTreeMap<i32, (usize, usize)> = BTreeMap::new();
    for (name, kind, _, _) in &raw {
        let entry = kinds.entry(*kind).or_default();
        entry.0 += 1;
        if !name.trim().is_empty() {
            entry.1 += 1;
        }
    }
    println!("{} raw controls; type id: count (named)", raw.len());
    for (kind, (count, named)) in &kinds {
        println!("  {kind}: {count} ({named})");
    }
    if std::env::var("ALL").is_ok() {
        for (name, kind, enabled, rect) in &raw {
            println!("  {kind} {rect:?} {}{}", name.chars().take(70).collect::<String>(), if *enabled { "" } else { " (disabled)" });
        }
    }
    let screen = anytask_control::screen::capture_primary(64).map(|(_, size)| size).unwrap_or((1920, 1080));
    let window = uia::window(hwnd, screen, Duration::from_secs(5));
    println!("\nwhat the model gets from \"{}\": {} elements", window.title, window.elements.len());
    for e in &window.elements {
        println!("  {} \"{}\" at {} {} {}x{}", e.kind, e.name.chars().take(60).collect::<String>(), e.x, e.y, e.w, e.h);
    }
}
