// Only the app's Rust side calls this plugin (no JavaScript commands); the build packs the Android library.
fn main() {
    tauri_plugin::Builder::new(&[]).android_path("android").build();
}
