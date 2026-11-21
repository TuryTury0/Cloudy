//! Plays a soft 440 Hz tone for one second and reports the speaker level while it plays.
fn main() {
    let speaker = anytask_audio::Speaker::start().expect("speaker");
    let tone: Vec<i16> = (0..16_000).map(|i| (6000.0 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin()) as i16).collect();
    speaker.play(&tone, 16_000);
    let mut levels = Vec::new();
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        levels.push(format!("{:.2}{}", speaker.level(), if speaker.is_playing() { "" } else { "." }));
    }
    println!("levels: {}", levels.join(" "));
}
