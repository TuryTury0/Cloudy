//! Presses a key combination, e.g. `cargo run -p anytask-control --example hotkey -- ctrl+alt+space`.
use anytask_core::tools::desktop::DesktopOps;

fn main() {
    let combo = std::env::args().nth(1).expect("a combination such as ctrl+alt+space");
    anytask_control::NativeDesktop.hotkey(&combo).expect("press");
    println!("pressed {combo}");
}
