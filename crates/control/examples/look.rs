//! Takes one look at the screen the way the desktop agent does (read-only: no clicks, no typing), prints how long
//! each part took and saves the screenshot with its rulers.   cargo run -p anytask-control --example look -- out.jpg
use anytask_control::{marks, screen, uia};
use std::time::{Duration, Instant};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "look.jpg".into());
    let t = Instant::now();
    let (mut image, size) = screen::capture_primary(1280).expect("capture");
    let capture = t.elapsed();
    let t = Instant::now();
    let mut window = uia::foreground(size, Duration::from_secs(10));
    if window.chromium && window.elements.len() < 8 {
        std::thread::sleep(Duration::from_millis(700));
        window = uia::foreground(size, Duration::from_secs(10));
    }
    let controls = t.elapsed();
    let t = Instant::now();
    marks::draw_rulers(&mut image);
    let jpeg = marks::encode_jpeg(&image, 70).expect("jpeg");
    let encode = t.elapsed();
    println!("capture {capture:?}, controls {controls:?}, rulers+jpeg {encode:?}; {} KB, {} controls in \"{}\" (chromium {})", jpeg.len() / 1024, window.elements.len(), window.title, window.chromium);
    for e in window.elements.iter().take(8) {
        println!("  {} \"{}\" at {},{} {}x{}", e.kind, e.name, e.x, e.y, e.w, e.h);
    }
    std::fs::write(&out, &jpeg).expect("write");
}
