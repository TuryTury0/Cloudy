//! The speakers' volume through Core Audio: the same level and mute the Windows volume flyout shows. The volume keys
//! only step it 2% at a time and cannot tell where it is, so "set it to 30" needs the level itself.

/// A Core Audio level (0.0-1.0) as the percent Windows shows.
pub fn percent(scalar: f32) -> u8 {
    if scalar.is_finite() { (scalar.clamp(0.0, 1.0) * 100.0).round() as u8 } else { 0 }
}

/// A percent as a Core Audio level.
pub fn scalar(level: u8) -> f32 {
    f32::from(level.min(100)) / 100.0
}

#[cfg(windows)]
mod imp {
    use super::{percent, scalar};
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};

    /// COM on the calling thread while this lives. The tools call from tokio's blocking pool, whose threads may have
    /// COM already (S_FALSE, still to be balanced) or as a single-threaded apartment (RPC_E_CHANGED_MODE: usable as
    /// it is, and not ours to undo).
    struct Com {
        balance: bool,
    }

    impl Com {
        fn init() -> Result<Self, String> {
            // SAFETY: plain COM initialisation of this thread; Drop balances it only when it succeeded.
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if hr.is_ok() {
                Ok(Self { balance: true })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self { balance: false })
            } else {
                Err(format!("COM did not start: {}", hr.message()))
            }
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            if self.balance {
                // SAFETY: balances the successful CoInitializeEx above; every interface was released before.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// Runs `f` on the default output device's volume control (the speakers or headphones Windows plays to).
    fn with_speakers<T>(f: impl FnOnce(&IAudioEndpointVolume) -> windows::core::Result<T>) -> Result<T, String> {
        let _com = Com::init()?;
        // The interfaces live only inside this closure, so they are released before COM is.
        let run = || -> windows::core::Result<T> {
            // SAFETY: COM is initialised on this thread and the interfaces are used only here.
            let speakers = unsafe {
                let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
                device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?
            };
            f(&speakers)
        };
        run().map_err(|e| format!("Windows volume: {}", e.message()))
    }

    pub fn get() -> Result<(u8, bool), String> {
        with_speakers(|speakers| {
            // SAFETY: plain reads on a live volume control.
            unsafe { Ok((percent(speakers.GetMasterVolumeLevelScalar()?), speakers.GetMute()?.as_bool())) }
        })
    }

    pub fn set(level: Option<u8>, mute: Option<bool>) -> Result<(), String> {
        with_speakers(|speakers| {
            // SAFETY: plain writes on a live volume control; a null event context is allowed.
            unsafe {
                // The level first, then mute: unmuting first would play a moment at the old level.
                if let Some(level) = level {
                    speakers.SetMasterVolumeLevelScalar(scalar(level), std::ptr::null())?;
                }
                if let Some(mute) = mute {
                    speakers.SetMute(mute, std::ptr::null())?;
                }
            }
            Ok(())
        })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn get() -> Result<(u8, bool), String> {
        Err("Reading the volume is not supported on this system yet.".into())
    }

    pub fn set(_level: Option<u8>, _mute: Option<bool>) -> Result<(), String> {
        Err("Setting the volume is not supported on this system yet.".into())
    }
}

pub use imp::{get, set};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_convert_to_percent_and_back() {
        assert_eq!(percent(0.3), 30);
        assert_eq!(percent(0.306), 31, "rounded as the flyout shows it");
        assert_eq!(percent(1.2), 100);
        assert_eq!(percent(-0.1), 0);
        assert_eq!(percent(f32::NAN), 0);
        for level in [0, 1, 30, 50, 99, 100] {
            assert_eq!(percent(scalar(level)), level);
        }
        assert_eq!(scalar(150), 1.0);
    }

    /// Reads the real speakers (read-only: the level and mute are never changed). Runs only on request:
    /// cargo test -p anytask-control -- --ignored volume_reads_the_real_speakers
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn volume_reads_the_real_speakers() {
        let now = get();
        println!("volume: {now:?}");
        let (level, _muted) = now.unwrap();
        assert!(level <= 100);
        // A second read on the same thread: COM was balanced, so it starts again cleanly.
        assert_eq!(get().map(|(level, _)| level), Ok(level));
    }
}
