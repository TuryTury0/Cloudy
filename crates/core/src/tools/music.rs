//! Music without a screen job: "play something", "play X in Spotify". Spotify's own search (a spotify:search: link)
//! and its "Play …" buttons, pressed through UI Automation, instead of a screen job that looked, thought and clicked:
//! a song took 30-70 s that way, with up to 31 s for one step and Spotify launched three times (logs 11-06 .. 11-12).
//! The button is pressed by its name, so nothing depends on where the window is or what the mouse does.

use super::system::{MediaControl, NowPlaying, SystemOps, off_runtime};
use super::{Tool, ToolCtx, arg_str, short};
use crate::lang::tr;
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

const SPOTIFY: &str = "Spotify";
/// How long a Spotify that is starting gets to show its window with its buttons.
const OPEN_WAIT: Duration = Duration::from_secs(15);
/// How long Spotify's search page gets to show its results.
const RESULTS_WAIT: Duration = Duration::from_secs(8);
/// How long the pressed song gets to start.
const PLAY_WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(300);
/// Said when the song could not be started this way: the model can still try the screen.
const NOT_FOUND: &str = "Spotify showed no matching song to play. Try a shorter query (song and artist as written on Spotify, Latin letters \
                         for Persian artists), or desktop_task in Spotify.";

pub struct PlayMusic {
    pub ops: Arc<dyn SystemOps>,
}

/// Words of a song name or query, for matching ("Imagine Dragons - Believer" -> imagine, dragons, believer).
fn words(text: &str) -> Vec<String> {
    const SKIP: &[&str] = &["by", "the", "a", "song", "music", "play", "ahang", "ahange", "az", "spotify", "feat", "ft", "آهنگ", "از"];
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2 && !SKIP.contains(w))
        .map(str::to_string)
        .collect()
}

/// How many of the query's words a name has.
fn score(name: &str, query: &[String]) -> usize {
    let have = words(name);
    query.iter().filter(|w| have.contains(w)).count()
}

/// The "Play …" button of Spotify's search page to press for `query`: the one with the most of its words, then one
/// the page did not show before the search (the library's playlists on the left have play buttons too), then the
/// first (the top result comes first). None while the results are not there.
pub fn pick_play_button(names: &[String], before: &HashSet<String>, query: &str) -> Option<String> {
    let wanted = words(query);
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| n.len() > 5 && n.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("play ")))
        .map(|(i, n)| (score(&n[5..], &wanted), !before.contains(n), i, n))
        .filter(|(score, new, _, _)| *score > 0 || *new)
        .max_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(b.2.cmp(&a.2)))
        .map(|(_, _, _, n)| n.clone())
}

/// Every word of the query is in the name: no need to wait for the page to settle.
fn full_match(name: &str, query: &str) -> bool {
    let wanted = words(query);
    !wanted.is_empty() && score(name.get(5..).unwrap_or(name), &wanted) == wanted.len()
}

/// A spotify:search: link for the query.
pub fn search_link(query: &str) -> String {
    let mut out = String::from("spotify:search:");
    for byte in query.trim().bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

impl PlayMusic {
    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&dyn SystemOps) -> Result<T, String> + Send + 'static) -> Result<T, String> {
        off_runtime(self.ops.clone(), f).await
    }

    async fn pause(ctx: &ToolCtx, wait: Duration) -> Result<(), String> {
        tokio::select! {
            _ = ctx.cancel.cancelled() => Err("Stopped".into()),
            _ = tokio::time::sleep(wait) => Ok(()),
        }
    }

    async fn buttons(&self) -> Vec<String> {
        self.blocking(|o| o.app_buttons(SPOTIFY)).await.unwrap_or_default()
    }

    async fn now(&self) -> Option<NowPlaying> {
        self.blocking(|o| o.now_playing()).await.ok().flatten()
    }

    /// Starts Spotify or brings it to the front, and waits until its window shows its buttons.
    async fn open_spotify(&self, ctx: &ToolCtx) -> Result<(), String> {
        (ctx.progress)(tr("Opening Spotify", "باز کردن اسپاتیفای").into());
        self.blocking(|o| o.launch_app(SPOTIFY)).await.map_err(|why| format!("Could not open Spotify ({why}). Is it installed?"))?;
        let started = Instant::now();
        loop {
            // A window that is still starting has a handful of buttons; its page has dozens.
            if self.buttons().await.len() >= 5 {
                (ctx.log)(format!("play_music: Spotify ready after {:.1} s", started.elapsed().as_secs_f32()));
                return Ok(());
            }
            if started.elapsed() > OPEN_WAIT {
                return Err("Spotify did not open in time.".into());
            }
            Self::pause(ctx, POLL).await?;
        }
    }

    /// Waits until something plays that `fits`; the last answer either way.
    async fn wait_playing(&self, ctx: &ToolCtx, fits: impl Fn(&NowPlaying) -> bool) -> Result<Option<NowPlaying>, String> {
        let started = Instant::now();
        loop {
            Self::pause(ctx, POLL).await?;
            let now = self.now().await;
            if now.as_ref().is_some_and(|p| p.playing && fits(p)) || started.elapsed() > PLAY_WAIT {
                return Ok(now);
            }
        }
    }

    fn report(player: Option<NowPlaying>, pressed: &str) -> Value {
        match player {
            Some(p) => {
                let mut out = json!({ "playing": p.playing, "now": p.label(), "app": p.app });
                if !pressed.is_empty() {
                    out["pressed"] = json!(pressed);
                }
                if !p.playing {
                    out["note"] = json!("It did not start playing.");
                }
                out
            }
            None => json!({ "playing": false, "note": "Spotify does not report a song playing." }),
        }
    }

    /// No song named: resume what plays last, opening Spotify when no player is open.
    async fn resume(&self, ctx: &ToolCtx) -> Result<Value, String> {
        let media = MediaControl { ops: self.ops.clone() };
        match self.now().await {
            Some(player) if player.playing => return Ok(json!({ "playing": true, "already": true, "now": player.label(), "app": player.app })),
            // A paused player is started where it is; with none open, a play key would reach nothing.
            Some(_) => match media.set_playing(true, ctx).await {
                Ok(result) if result["playing"] == json!(true) => return Ok(result),
                Ok(result) => (ctx.log)(format!("play_music: play gave {result}, opening Spotify")),
                Err(why) => (ctx.log)(format!("play_music: {why}")),
            },
            None => {}
        }
        self.open_spotify(ctx).await?;
        // Spotify opens paused on the last song: its own Play button starts it, wherever the keyboard focus is.
        let pressed = match self.blocking(|o| o.press_app_button(SPOTIFY, "Play")).await {
            Ok(()) => "Play",
            Err(why) => {
                (ctx.log)(format!("play_music: no Play button ({why}), using the play key"));
                media.set_playing(true, ctx).await.ok();
                ""
            }
        };
        Ok(Self::report(self.wait_playing(ctx, |_| true).await?, pressed))
    }

    /// A song, artist or playlist: Spotify's search, then the best "Play …" button of the results.
    async fn play(&self, query: &str, ctx: &ToolCtx) -> Result<Value, String> {
        let started = Instant::now();
        let wanted = words(query);
        if let Some(now) = self.now().await.filter(|p| p.playing && !wanted.is_empty() && score(&p.label(), &wanted) == wanted.len()) {
            return Ok(json!({ "playing": true, "already": true, "now": now.label(), "app": now.app }));
        }
        self.open_spotify(ctx).await?;
        let before: HashSet<String> = self.buttons().await.into_iter().collect();
        let playing_before = self.now().await.map(|p| p.label());
        (ctx.progress)(format!("{} {}", tr("Searching Spotify for", "جستجو در اسپاتیفای:"), short(query, 40)));
        let link = search_link(query);
        self.blocking(move |o| o.open(&link)).await?;
        let mut pick: Option<String> = None;
        let waited = Instant::now();
        let name = loop {
            Self::pause(ctx, POLL).await?;
            let found = pick_play_button(&self.buttons().await, &before, query);
            // Results come in pieces: take a button once two reads agree, or at once when it has every word.
            match found {
                Some(name) if full_match(&name, query) || pick.as_deref() == Some(name.as_str()) => break name,
                Some(name) => pick = Some(name),
                None => {}
            }
            if waited.elapsed() > RESULTS_WAIT {
                match pick {
                    Some(name) => break name,
                    None => return Err(NOT_FOUND.into()),
                }
            }
        };
        (ctx.progress)(format!("{} {}", tr("Playing", "پخش"), short(name.get(5..).unwrap_or(&name), 50)));
        let press = name.clone();
        self.blocking(move |o| o.press_app_button(SPOTIFY, &press)).await?;
        let title = name[5..].to_string();
        let after = self.wait_playing(ctx, |p| Some(p.label()) != playing_before || score(&p.label(), &words(&title)) > 0).await?;
        (ctx.log)(format!("play_music: \"{query}\" -> pressed \"{name}\", {:?} after {:.1} s", after.as_ref().map(|p| (p.playing, p.label())), started.elapsed().as_secs_f32()));
        Ok(Self::report(after, &name))
    }
}

#[async_trait]
impl Tool for PlayMusic {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "play_music".into(),
            description: "Plays music in Spotify in a few seconds, without the screen: opens Spotify when needed. With query (a song, an artist, \
                          an album or a playlist) it searches Spotify and starts the best match; without query it resumes the last song. \
                          The result says what really plays (\"playing\", \"now\"). Use it for every 'play X' and 'play some music' request."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Song and/or artist as written on Spotify (Latin letters for Persian artists, e.g. 'Shadmehr Aghili Bi Ehsas', 'Believer Imagine Dragons'). Leave out to resume."
                    }
                }
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        match arg_str(args, "query") {
            Some(query) => format!("{} {}", tr("Playing", "پخش"), short(query, 40)),
            None => tr("Playing music", "پخش موسیقی").into(),
        }
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        match arg_str(&args, "query") {
            Some(query) => self.play(query, ctx).await,
            None => self.resume(ctx).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::DenyAll;
    use crate::tools::system::MediaKey;
    use crate::tools::test_support::ctx;
    use std::sync::Mutex;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_matching_song_is_picked_not_the_library_or_the_player() {
        let before: HashSet<String> = names(&["Play", "Play Liked Songs", "Play Daily Mix 1"]).into_iter().collect();
        let page = names(&["Play", "Play Liked Songs", "Play Daily Mix 1", "Play Imagine Dragons", "Play Believer by Imagine Dragons", "Play Thunder by Imagine Dragons"]);
        assert_eq!(pick_play_button(&page, &before, "Believer Imagine Dragons").as_deref(), Some("Play Believer by Imagine Dragons"));
        // A query in Persian letters matches no Latin name: the first new button (the top result) is taken.
        assert_eq!(pick_play_button(&page, &before, "شادمهر").as_deref(), Some("Play Imagine Dragons"));
        // Before the results come, only old buttons are there: nothing to press yet.
        assert_eq!(pick_play_button(&names(&["Play", "Play Liked Songs"]), &before, "Believer"), None);
        assert!(full_match("Play Believer by Imagine Dragons", "believer imagine dragons"));
        assert!(!full_match("Play Thunder by Imagine Dragons", "believer imagine dragons"));
    }

    #[test]
    fn search_links_are_encoded() {
        assert_eq!(search_link("Bi Ehsas Shadmehr"), "spotify:search:Bi%20Ehsas%20Shadmehr");
        assert_eq!(search_link("گل"), "spotify:search:%DA%AF%D9%84");
    }

    /// A Spotify that starts paused and plays whatever "Play …" button is pressed.
    #[derive(Default)]
    struct FakeSpotify {
        open: Mutex<bool>,
        searched: Mutex<bool>,
        playing: Mutex<Option<String>>,
        pressed: Mutex<Vec<String>>,
    }

    impl SystemOps for FakeSpotify {
        fn clipboard_read(&self) -> Result<String, String> {
            Ok(String::new())
        }
        fn clipboard_write(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn media_key(&self, _: MediaKey) -> Result<(), String> {
            Ok(())
        }
        fn open(&self, target: &str) -> Result<(), String> {
            assert!(target.starts_with("spotify:search:"));
            *self.searched.lock().unwrap() = true;
            Ok(())
        }
        fn now_playing(&self) -> Result<Option<NowPlaying>, String> {
            Ok(self.playing.lock().unwrap().clone().map(|song| NowPlaying { app: "Spotify".into(), title: song, artist: "Imagine Dragons".into(), playing: true }))
        }
        fn media_play(&self, _: bool) -> Result<bool, String> {
            Ok(false)
        }
        fn launch_app(&self, name: &str) -> Result<(), String> {
            assert_eq!(name, "Spotify");
            *self.open.lock().unwrap() = true;
            Ok(())
        }
        fn app_buttons(&self, _: &str) -> Result<Vec<String>, String> {
            if !*self.open.lock().unwrap() {
                return Err("no window".into());
            }
            let mut list = names(&["Play", "Home", "Search", "Play Liked Songs", "Next", "Previous"]);
            if *self.searched.lock().unwrap() {
                list.extend(names(&["Play Believer by Imagine Dragons", "Play Thunder by Imagine Dragons"]));
            }
            Ok(list)
        }
        fn press_app_button(&self, _: &str, name: &str) -> Result<(), String> {
            self.pressed.lock().unwrap().push(name.to_string());
            let song = name.strip_prefix("Play ").unwrap_or("Last song").split(" by ").next().unwrap_or_default();
            *self.playing.lock().unwrap() = Some(if song.is_empty() || name == "Play" { "Last song".into() } else { song.to_string() });
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_song_is_searched_pressed_and_reported() {
        let spotify = Arc::new(FakeSpotify::default());
        let tool = PlayMusic { ops: spotify.clone() };
        let out = tool.run(json!({ "query": "Believer Imagine Dragons" }), &ctx(Arc::new(DenyAll))).await.unwrap();
        assert_eq!(out["playing"], json!(true));
        assert_eq!(out["now"], json!("Imagine Dragons - Believer"));
        assert_eq!(*spotify.pressed.lock().unwrap(), ["Play Believer by Imagine Dragons"]);
    }

    #[tokio::test]
    async fn no_song_named_opens_spotify_and_presses_its_play_button() {
        let spotify = Arc::new(FakeSpotify::default());
        let tool = PlayMusic { ops: spotify.clone() };
        let out = tool.run(json!({}), &ctx(Arc::new(DenyAll))).await.unwrap();
        assert_eq!(out["playing"], json!(true), "{out}");
        assert_eq!(*spotify.pressed.lock().unwrap(), ["Play"]);
        assert!(!*spotify.searched.lock().unwrap());
    }

    #[tokio::test]
    async fn a_song_that_already_plays_is_left_alone() {
        let spotify = Arc::new(FakeSpotify::default());
        *spotify.playing.lock().unwrap() = Some("Believer".into());
        let out = PlayMusic { ops: spotify.clone() }.run(json!({ "query": "believer imagine dragons" }), &ctx(Arc::new(DenyAll))).await.unwrap();
        assert_eq!(out["already"], json!(true));
        assert!(spotify.pressed.lock().unwrap().is_empty());
    }
}
