// The phone app also runs on the desktop for development.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    anytask_mobile_lib::run()
}
