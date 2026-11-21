//! The PC itself: its drives (for browsing files from the phone) and sleep.

/// Drive letters of the PC's fixed, removable and network drives, read without touching them (a check such as
/// `exists()` on an empty card reader or a lost network drive can hang for seconds).
#[cfg(windows)]
pub fn drives() -> Vec<char> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    use windows::core::PCWSTR;
    // DRIVE_REMOVABLE 2, DRIVE_FIXED 3, DRIVE_REMOTE 4.
    // SAFETY: reads a bit mask and asks the type of each drive root by a NUL-terminated wide string.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .filter_map(|i| {
            let letter = char::from(b'A' + i as u8);
            let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(std::iter::once(0)).collect();
            let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
            matches!(kind, 2..=4).then_some(letter)
        })
        .collect()
}

#[cfg(not(windows))]
pub fn drives() -> Vec<char> {
    Vec::new()
}

/// Puts the PC to sleep (not hibernation: waking it from the phone works from sleep far more often). Returns after
/// the PC wakes again.
#[cfg(windows)]
pub fn sleep() -> Result<(), String> {
    use windows::Win32::System::Power::SetSuspendState;
    // SAFETY: a plain system call with three flags: no hibernation, no force, wake events allowed.
    let slept = unsafe { SetSuspendState(false, false, false) };
    if slept { Ok(()) } else { Err("Windows did not go to sleep.".into()) }
}

#[cfg(not(windows))]
pub fn sleep() -> Result<(), String> {
    Err("Sleep works on Windows only.".into())
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    #[test]
    fn the_system_drive_is_listed() {
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let letter = system.chars().next().unwrap().to_ascii_uppercase();
        assert!(super::drives().contains(&letter), "{:?}", super::drives());
    }
}
