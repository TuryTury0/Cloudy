//! Opens the microphone for two seconds and reports chunks and loudness (nothing is recorded or sent).
fn main() {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = count.clone();
    let mic = anytask_audio::Mic::start(move |chunk| {
        assert_eq!(chunk.len(), anytask_audio::MIC_CHUNK);
        c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    })
    .expect("microphone");
    let mut loudest = 0.0f32;
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        loudest = loudest.max(mic.level());
    }
    println!("{} chunks of 40 ms in 2 s (expected ~50), loudest level {loudest:.2}", count.load(std::sync::atomic::Ordering::Relaxed));
}
