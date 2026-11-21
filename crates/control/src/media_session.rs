//! The player Windows knows about (the one in its media flyout): what it plays, and starting or pausing it
//! directly. Media keys only toggle and cannot tell whether any player is open; the media session can.

use anytask_core::tools::system::NowPlaying;

/// The app's name from its media-session id: "Spotify.exe" -> Spotify,
/// "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic" -> Microsoft.ZuneMusic.
pub fn app_name(id: &str) -> String {
    let name = id.rsplit('!').next().unwrap_or(id).trim();
    match name.len().checked_sub(4) {
        Some(cut) if name.is_char_boundary(cut) && name[cut..].eq_ignore_ascii_case(".exe") => name[..cut].to_string(),
        _ => name.to_string(),
    }
}

#[cfg(windows)]
mod imp {
    use super::{NowPlaying, app_name};
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };

    /// Windows answers in tens of milliseconds; a session that hangs must not hold the tool.
    const ANSWER: Duration = Duration::from_secs(2);

    /// Called with the result of a Windows async call.
    type Done<T> = Box<dyn FnOnce(windows::core::Result<T>) + Send>;

    /// Waits for a Windows async call, at most `ANSWER`. `start` starts the call and hands `done` to its `when`.
    fn wait<T: Send + 'static>(start: impl FnOnce(Done<T>) -> windows::core::Result<()>) -> Result<T, String> {
        let (sent, got) = mpsc::channel();
        start(Box::new(move |result| {
            let _ = sent.send(result.map_err(|e| e.to_string()));
        }))
        .map_err(|e| e.to_string())?;
        got.recv_timeout(ANSWER).map_err(|_| "Windows did not answer about the media session in time.".to_string())?
    }

    /// The current session; None when no player has one.
    fn current() -> Result<Option<Session>, String> {
        let manager = wait(|done| Manager::RequestAsync()?.when(done))?;
        match manager.GetCurrentSession() {
            Ok(session) => Ok(Some(session)),
            // A null session (no error code) means there is none.
            Err(e) if e.code().is_ok() => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn now_playing() -> Result<Option<NowPlaying>, String> {
        let Some(session) = current()? else { return Ok(None) };
        let status = session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()).map_err(|e| e.to_string())?;
        let app = app_name(&session.SourceAppUserModelId().map(|id| id.to_string()).unwrap_or_default());
        // Some players give no song details (or are slow to); the state alone is still worth having.
        let (title, artist) = match wait(|done| session.TryGetMediaPropertiesAsync()?.when(done)) {
            Ok(song) => (song.Title().map(|t| t.to_string()).unwrap_or_default(), song.Artist().map(|a| a.to_string()).unwrap_or_default()),
            Err(_) => (String::new(), String::new()),
        };
        Ok(Some(NowPlaying { app, title, artist, playing: status == Status::Playing }))
    }

    pub fn play(start: bool) -> Result<bool, String> {
        let session = current()?.ok_or("No media session.")?;
        if start { wait(|done| session.TryPlayAsync()?.when(done)) } else { wait(|done| session.TryPauseAsync()?.when(done)) }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::NowPlaying;

    pub fn now_playing() -> Result<Option<NowPlaying>, String> {
        Err("Media sessions are not supported on this system yet.".into())
    }

    pub fn play(_start: bool) -> Result<bool, String> {
        Err("Media sessions are not supported on this system yet.".into())
    }
}

pub use imp::{now_playing, play};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names_come_from_session_ids() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("SPOTIFY.EXE"), "SPOTIFY");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Microsoft.ZuneMusic");
        assert_eq!(app_name("chrome"), "chrome");
        assert_eq!(app_name(""), "");
    }

    /// Asks Windows for the real media session (read-only: nothing is pressed or started). Runs only on request:
    /// cargo test -p anytask-control -- --ignored now_playing_reads_the_real_session
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn now_playing_reads_the_real_session() {
        let now = now_playing();
        println!("now playing: {now:?}");
        assert!(now.is_ok(), "{now:?}");
    }
}
