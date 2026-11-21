//! System tools: clipboard, media keys, opening files and links. The OS work is behind `SystemOps` so the core
//! stays testable; the desktop app provides the real implementation (crates/control).

use super::{Tool, ToolCtx, arg_str, short};
use crate::lang::{digits, tr};
use crate::paths::UserFolders;
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// Said when a music request finds no player: a media key would reach nothing, so the model must open the app
/// instead of telling the user that something plays.
const NO_PLAYER: &str = "No music player is open. Use play_music: it opens Spotify and starts the last song (or a song you name).";
/// Said when Windows lists no player but a key was pressed anyway: players Windows does not list (VLC 3, PotPlayer,
/// KMPlayer, MPC-HC) still get media keys, so the key is not held back, but nothing is claimed either.
const UNLISTED: &str = "Windows lists no player; one it does not list (VLC, PotPlayer, ...) may have got the key. Do not say \
                        that something plays, paused or changed; say the key was pressed.";
/// A music app that was just opened (Spotify) registers with Windows only once it plays: after the play key it gets
/// this long to show up.
const APPEAR_WAIT: Duration = Duration::from_secs(4);
/// How long a player gets to start or pause after being asked (Spotify answers in a few hundred ms).
const START_WAIT: Duration = Duration::from_secs(2);
/// How long a player gets to show the next song (or its new state) after a media key. Spotify takes more than a
/// second to show the next song to Windows (test 3, 05:22: the answer to "ahang badi" could not name it).
const KEY_WAIT: Duration = Duration::from_millis(2500);
const STATE_POLL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    Previous,
    Stop,
    VolumeUp,
    VolumeDown,
    Mute,
}

/// The page open in the user's browser: the front-most browser window, as the user last saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserTab {
    /// "Vivaldi", "Google Chrome", ...
    pub browser: String,
    /// The page's title, from the window title without the browser's name.
    pub title: String,
    /// What the address bar shows, with "https://" added where the browser hides it. None when it could not be read.
    pub url: Option<String>,
}

/// What the player Windows knows about (the one in its media flyout) is doing. Media keys cannot tell whether any
/// player is open, so this is what the assistant checks before it says that something plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NowPlaying {
    /// The player's app ("Spotify").
    pub app: String,
    pub title: String,
    pub artist: String,
    /// Playing right now (false: paused, stopped or between songs).
    pub playing: bool,
}

impl NowPlaying {
    /// "Artist - Title", or the part of it that is known.
    pub fn label(&self) -> String {
        match (self.artist.trim(), self.title.trim()) {
            ("", title) => title.to_string(),
            (artist, "") => artist.to_string(),
            (artist, title) => format!("{artist} - {title}"),
        }
    }
}

pub trait SystemOps: Send + Sync {
    fn clipboard_read(&self) -> Result<String, String>;
    fn clipboard_write(&self, text: &str) -> Result<(), String>;
    fn media_key(&self, key: MediaKey) -> Result<(), String>;
    /// Opens a file, folder or http(s) link with the default program.
    fn open(&self, target: &str) -> Result<(), String>;
    /// What the current media session plays. Ok(None): there is no session, so no player is open (or none that
    /// Windows knows of) and a media key would reach nothing. Err: this system cannot tell. May wait for the system
    /// to answer, so call it off the async runtime.
    fn now_playing(&self) -> Result<Option<NowPlaying>, String> {
        Err("unknown".into())
    }
    /// Starts (true) or pauses (false) the current media session itself; the play/pause key only toggles, so it
    /// pauses music that already plays. Ok(false): the player refused. Err: this system cannot do it.
    fn media_play(&self, _play: bool) -> Result<bool, String> {
        Err("unknown".into())
    }
    /// The speakers' volume as Windows shows it in its volume flyout: level 0-100 and whether they are muted. The
    /// volume keys only step it and cannot tell where it is, so "set it to 30" needs this. Err: this system cannot
    /// read it. May wait for the system, so call it off the async runtime.
    fn volume(&self) -> Result<(u8, bool), String> {
        Err("unknown".into())
    }
    /// Sets the speakers' level (0-100) and/or mute; None leaves that part as it is. Err: this system cannot.
    fn set_volume(&self, _level: Option<u8>, _mute: Option<bool>) -> Result<(), String> {
        Err("unknown".into())
    }
    /// The tab the user has open in their front-most browser window, also while AnyTask's own window is in front.
    /// Ok(None): no browser window is open. May wait for the browser to answer, so call it off the async runtime.
    fn browser_tab(&self) -> Result<Option<BrowserTab>, String> {
        Err("Reading the browser is not supported on this system.".into())
    }
    /// Starts an installed app, or brings it to the front when it is open. Returns once it was started; its window
    /// may come a moment later.
    fn launch_app(&self, _name: &str) -> Result<(), String> {
        Err("Starting apps is not supported on this system.".into())
    }
    /// The names of the buttons in an app's main window, in the order of its page (UI Automation), whether it is in
    /// front or not. Err: the app has no window, or it cannot be read. May wait for the app, so call it off the async
    /// runtime.
    fn app_buttons(&self, _app: &str) -> Result<Vec<String>, String> {
        Err("Reading app buttons is not supported on this system.".into())
    }
    /// Presses the first button with exactly this name in an app's main window: through UI Automation when the app
    /// allows it (no mouse, the window may stay where it is), else a click on its middle.
    fn press_app_button(&self, _app: &str, _name: &str) -> Result<(), String> {
        Err("Pressing app buttons is not supported on this system.".into())
    }
}

/// Runs a system call off the async runtime: Windows may take a moment to answer (media session, audio devices).
pub(crate) async fn off_runtime<T: Send + 'static>(ops: Arc<dyn SystemOps>, f: impl FnOnce(&dyn SystemOps) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(move || f(ops.as_ref())).await.map_err(|e| format!("system worker stopped: {e}"))?
}

const BLOCKED_OPEN: &[&str] = &[
    "exe", "msi", "py", "pyw", "bat", "cmd", "com", "scr", "pif", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "hta", "cpl", "msc", "reg", "lnk", "url",
    "jar", "dll", "sh", "app", "appimage", "desktop", "apk", "dmg", "pkg", "deb", "rpm",
];

pub struct ClipboardRead {
    pub ops: Arc<dyn SystemOps>,
}

#[async_trait]
impl Tool for ClipboardRead {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "clipboard_read".into(),
            description: "Reads the text currently on the clipboard.".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }
    fn label(&self, _: &Value) -> String {
        tr("Reading the clipboard", "خواندن کلیپ‌بورد").into()
    }
    async fn run(&self, _args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        let text = self.ops.clipboard_read()?;
        Ok(json!({ "text": text.chars().take(20_000).collect::<String>() }))
    }
}

pub struct ClipboardWrite {
    pub ops: Arc<dyn SystemOps>,
}

#[async_trait]
impl Tool for ClipboardWrite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "clipboard_write".into(),
            description: "Puts text on the clipboard.".into(),
            parameters: json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] }),
        }
    }
    fn label(&self, _: &Value) -> String {
        tr("Copying to the clipboard", "کپی در کلیپ‌بورد").into()
    }
    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        let text = args.get("text").and_then(Value::as_str).ok_or("text is required.")?;
        self.ops.clipboard_write(text)?;
        Ok(json!({ "copied": text.chars().count() }))
    }
}

pub struct MediaControl {
    pub ops: Arc<dyn SystemOps>,
}

/// What media_control was asked to do.
enum MediaAction {
    /// Start (true) or pause (false), never toggle.
    Set(bool),
    Key(MediaKey),
    /// Only say what plays; nothing is pressed.
    Status,
}

impl MediaAction {
    fn parse(args: &Value) -> Result<Self, String> {
        match arg_str(args, "action").unwrap_or("play_pause") {
            "play" => Ok(Self::Set(true)),
            "pause" => Ok(Self::Set(false)),
            "status" => Ok(Self::Status),
            other => serde_json::from_value(json!(other)).map(Self::Key).map_err(|_| "Unknown media action.".to_string()),
        }
    }
}

impl MediaControl {
    /// Runs a system call off the async runtime: asking Windows about the media session waits for its answer.
    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&dyn SystemOps) -> Result<T, String> + Send + 'static) -> Result<T, String> {
        off_runtime(self.ops.clone(), f).await
    }

    async fn now(&self) -> Result<Option<NowPlaying>, String> {
        self.blocking(|ops| ops.now_playing()).await
    }

    async fn press(&self, key: MediaKey, times: u64) -> Result<(), String> {
        for i in 0..times {
            self.blocking(move |ops| ops.media_key(key)).await?;
            if i + 1 < times {
                tokio::time::sleep(Duration::from_millis(80)).await;
            }
        }
        Ok(())
    }

    /// Asks the player again until `done` holds, it is gone, or `wait` passed. Returns the last answer: players
    /// take a moment to start or show the next song.
    async fn settle(&self, wait: Duration, done: impl Fn(&NowPlaying) -> bool, ctx: &ToolCtx) -> Result<Option<NowPlaying>, String> {
        let started = Instant::now();
        loop {
            tokio::select! {
                _ = ctx.cancel.cancelled() => return Err("Stopped".into()),
                _ = tokio::time::sleep(STATE_POLL) => {}
            }
            let now = self.now().await;
            match &now {
                Ok(Some(player)) if !done(player) && started.elapsed() < wait => continue,
                _ => return now,
            }
        }
    }

    /// Starts (play) or pauses the player and reports what it really does. Never toggles blindly: a player that
    /// already plays is left alone, and with no player open nothing is pressed.
    pub(crate) async fn set_playing(&self, play: bool, ctx: &ToolCtx) -> Result<Value, String> {
        let before = match self.now().await {
            Ok(Some(player)) => player,
            Ok(None) if play => return self.wake(ctx).await,
            Ok(None) => {
                self.press(MediaKey::PlayPause, 1).await?;
                return Ok(json!({ "pressed": 1, "note": UNLISTED }));
            }
            Err(why) => {
                // This system cannot see the player: the toggle key is all there is, as before.
                (ctx.log)(format!("media_control: cannot see the player ({why}), pressing play/pause"));
                self.press(MediaKey::PlayPause, 1).await?;
                return Ok(json!({ "pressed": 1, "note": "could not check whether something is playing now" }));
            }
        };
        if before.playing == play {
            return Ok(json!({ "playing": before.playing, "already": true, "now": before.label(), "app": before.app }));
        }
        let asked = self.blocking(move |ops| ops.media_play(play)).await;
        // Asked directly and it said no, or it cannot be asked: the player is known to be in the other state, so
        // the toggle key goes the right way. An answer that did not come in time may still arrive: the key goes
        // only if the player is still in the other state, or it would undo that answer.
        let toggle = match asked {
            Ok(true) => false,
            Ok(false) => true,
            Err(_) => matches!(self.now().await, Ok(Some(ref player)) if player.playing != play),
        };
        if toggle {
            self.press(MediaKey::PlayPause, 1).await?;
        }
        let started = Instant::now();
        let after = self.settle(START_WAIT, |player| player.playing == play, ctx).await;
        (ctx.log)(format!(
            "media_control: {} {} -> {:?} after {:.1} s",
            if play { "play" } else { "pause" },
            before.app,
            after.as_ref().map(|p| p.as_ref().map(|p| p.playing)),
            started.elapsed().as_secs_f32()
        ));
        Ok(match after {
            Ok(Some(player)) => {
                let mut out = json!({ "playing": player.playing, "now": player.label(), "app": player.app });
                if player.playing != play {
                    out["note"] = json!(if play { "The player did not start playing." } else { "The player did not pause." });
                }
                out
            }
            Ok(None) => json!({ "playing": false, "note": "The player closed." }),
            Err(_) => json!({ "note": "could not check whether something is playing now" }),
        })
    }

    /// "play" with no player Windows knows: Spotify just opened by open_app registers only once it plays, and the
    /// app in front gets the play key. So the key is pressed once and the player gets a moment to show up.
    async fn wake(&self, ctx: &ToolCtx) -> Result<Value, String> {
        self.press(MediaKey::PlayPause, 1).await?;
        let started = Instant::now();
        while started.elapsed() < APPEAR_WAIT {
            tokio::select! {
                _ = ctx.cancel.cancelled() => return Err("Stopped".into()),
                _ = tokio::time::sleep(STATE_POLL) => {}
            }
            if let Ok(Some(player)) = self.now().await
                && player.playing
            {
                (ctx.log)(format!("media_control: {} started after the play key in {:.1} s", player.app, started.elapsed().as_secs_f32()));
                return Ok(json!({ "playing": true, "now": player.label(), "app": player.app }));
            }
        }
        Err(NO_PLAYER.into())
    }

    /// Presses a media key. With no player Windows lists, the key still goes (players it does not list get it too)
    /// but nothing is claimed. When the player is known, the result says what it plays afterwards.
    async fn key(&self, key: MediaKey, times: u64, ctx: &ToolCtx) -> Result<Value, String> {
        // Volume and mute work without a player: no need to ask.
        let before = match key {
            MediaKey::VolumeUp | MediaKey::VolumeDown | MediaKey::Mute => None,
            MediaKey::PlayPause | MediaKey::Next | MediaKey::Previous => match self.now().await {
                Ok(None) => {
                    self.press(key, times).await?;
                    return Ok(json!({ "pressed": times, "note": UNLISTED }));
                }
                now => now.ok().flatten(),
            },
            MediaKey::Stop => self.now().await.ok().flatten(),
        };
        self.press(key, times).await?;
        let mut out = json!({ "pressed": times });
        if let Some(before) = before {
            let done = |player: &NowPlaying| match key {
                // While a song plays, the next one is there once it plays too (in between the player is "changing").
                MediaKey::Next | MediaKey::Previous => player.label() != before.label() && (player.playing || !before.playing),
                MediaKey::PlayPause => player.playing != before.playing,
                _ => !player.playing,
            };
            let mut after = self.settle(KEY_WAIT, &done, ctx).await;
            // Players restart the song on the first "previous" (training round 2: "ahang ghabli" said it went back
            // while the same song played on): when the song did not change, press it once more.
            if key == MediaKey::Previous && matches!(&after, Ok(Some(player)) if player.label() == before.label()) {
                self.press(MediaKey::Previous, 1).await?;
                out["pressed"] = json!(times + 1);
                after = self.settle(KEY_WAIT, &done, ctx).await;
            }
            if let Ok(Some(player)) = after {
                out["playing"] = json!(player.playing);
                out["app"] = json!(player.app);
                // A song the player still shows from before is not the next one.
                if done(&player) || !matches!(key, MediaKey::Next | MediaKey::Previous) {
                    out["now"] = json!(player.label());
                }
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl Tool for MediaControl {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "media_control".into(),
            description: "Instantly controls the music/video player that Windows knows (Spotify, a browser tab, ...): play (starts or resumes \
                          the last song; never pauses), pause (never starts), next, previous, stop, volume_up, volume_down, mute. Use play \
                          and pause, not play_pause, to start or stop. The result says what really plays (\"playing\", \"now\"); an error \
                          means no player is open. status only tells what plays (\"what song is this\", \"chi dare play mishe\") and                           presses nothing. Use it for resume/pause/next/volume instead of clicking."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "play", "pause", "play_pause", "next", "previous", "stop", "volume_up", "volume_down", "mute"]
                    },
                    "times": { "type": "integer", "description": "How many presses (e.g. 5 for volume). Default 1." }
                },
                "required": ["action"]
            }),
        }
    }
    fn label(&self, args: &Value) -> String {
        let action = match arg_str(args, "action").unwrap_or("play_pause") {
            "play" => tr("Play", "پخش"),
            "pause" => tr("Pause", "مکث"),
            "next" => tr("Next song", "آهنگ بعدی"),
            "previous" => tr("Previous song", "آهنگ قبلی"),
            "stop" => tr("Stop playing", "توقف پخش"),
            "volume_up" => tr("Volume up", "صدا بلندتر"),
            "volume_down" => tr("Volume down", "صدا آرام‌تر"),
            "mute" => tr("Mute", "بی‌صدا"),
            "status" => tr("What is playing", "چه چیزی پخش می‌شود"),
            _ => tr("Play / pause", "پخش / مکث"),
        };
        action.to_string()
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let times = args.get("times").and_then(Value::as_u64).unwrap_or(1).clamp(1, 10);
        match MediaAction::parse(&args)? {
            MediaAction::Set(play) => self.set_playing(play, ctx).await,
            MediaAction::Key(key) => self.key(key, times, ctx).await,
            MediaAction::Status => Ok(match self.now().await {
                Ok(Some(player)) => json!({ "playing": player.playing, "now": player.label(), "app": player.app }),
                Ok(None) => json!({ "playing": false, "note": "No player is open: nothing plays." }),
                Err(why) => json!({ "note": format!("Cannot tell what plays on this system ({why}).") }),
            }),
        }
    }
}

/// Said when the volume cannot be read or set exactly here: the volume keys still work, but blindly.
const VOLUME_KEYS: &str = "Use media_control volume_up / volume_down instead (2% per press, times up to 10) or media_control mute, \
                           and do not claim an exact level.";

/// Reads and sets the speakers' volume exactly. The volume keys (media_control) move it 2% per press and cannot tell
/// where it is, so "seda ro bezar ru 30" or "nesf kon" needs the level itself.
pub struct Volume {
    pub ops: Arc<dyn SystemOps>,
}

impl Volume {
    /// A number argument; models send 30, 30.0 or "30".
    fn number(args: &Value, name: &str) -> Result<Option<f64>, String> {
        match args.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => Ok(n.as_f64()),
            Some(Value::String(s)) => s.trim().trim_end_matches('%').trim().parse().map(Some).map_err(|_| format!("{name} must be a number.")),
            Some(_) => Err(format!("{name} must be a number.")),
        }
        .map(|n| n.filter(|n| n.is_finite()))
    }
}

fn volume_json((level, muted): (u8, bool)) -> Value {
    json!({ "level": level, "muted": muted })
}

#[async_trait]
impl Tool for Volume {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "volume".into(),
            description: "Reads or sets the speakers' volume exactly (instant, no player needed). level: set it to that percent (0-100). \
                          change: move it by that many points (-10 quieter, 20 louder). mute: true mutes, false unmutes. Any level above \
                          0 also unmutes. With no arguments it only reports. For \"half\" (nesf), set level to half of the current level \
                          from the report. The result has before and now ({level, muted}); say the level from now. Use this, not \
                          media_control volume_up/volume_down, for exact or relative volume."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "level": { "type": "integer", "description": "Exact volume in percent, 0-100." },
                    "change": { "type": "integer", "description": "Relative change in percent points, e.g. -10 or 20." },
                    "mute": { "type": "boolean", "description": "true mutes, false unmutes." }
                }
            }),
        }
    }
    fn label(&self, args: &Value) -> String {
        let level = Self::number(args, "level").ok().flatten();
        let change = Self::number(args, "change").ok().flatten();
        match (args.get("mute").and_then(Value::as_bool), level, change) {
            (Some(true), _, _) => tr("Mute", "بی‌صدا").into(),
            (_, Some(level), _) => format!("{} {}{}", tr("Volume", "صدا"), digits(level.round().clamp(0.0, 100.0) as u64), tr("%", "٪")),
            (_, None, Some(change)) if change > 0.0 => format!("{} {}{}", tr("Volume up", "صدا بلندتر"), digits(change.round().min(100.0) as u64), tr("%", "٪")),
            (_, None, Some(change)) if change < 0.0 => format!("{} {}{}", tr("Volume down", "صدا آرام‌تر"), digits((-change).round().min(100.0) as u64), tr("%", "٪")),
            (Some(false), _, _) => tr("Unmute", "برگرداندن صدا").into(),
            _ => tr("Volume", "میزان صدا").into(),
        }
    }
    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let level = Self::number(&args, "level")?;
        let change = Self::number(&args, "change")?;
        let mute = match args.get("mute") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_bool().ok_or("mute must be true or false.")?),
        };
        let before = off_runtime(self.ops.clone(), |ops| ops.volume()).await.map_err(|why| {
            (ctx.log)(format!("volume: cannot read the level ({why})"));
            format!("Cannot read the volume on this system ({why}). {VOLUME_KEYS}")
        })?;
        // An exact level wins over a change; either way it stays within 0-100.
        let target = level.or_else(|| change.map(|change| f64::from(before.0) + change)).map(|level| level.round().clamp(0.0, 100.0) as u8);
        // A level above 0 is meant to be heard: it unmutes too (as the volume keys do), unless mute says otherwise.
        let mute = mute.or((before.1 && target.is_some_and(|level| level > 0)).then_some(false));
        if target.is_none() && mute.is_none() {
            return Ok(json!({ "now": volume_json(before) }));
        }
        off_runtime(self.ops.clone(), move |ops| ops.set_volume(target, mute))
            .await
            .map_err(|why| format!("Could not set the volume ({why}). {VOLUME_KEYS}"))?;
        // Read back: what the speakers really are now, not what was asked.
        let now = off_runtime(self.ops.clone(), |ops| ops.volume()).await.map_err(|why| format!("The volume was set but could not be read back ({why})."))?;
        (ctx.log)(format!("volume: {before:?} -> {now:?} (asked level {target:?}, mute {mute:?})"));
        Ok(json!({ "before": volume_json(before), "now": volume_json(now) }))
    }
}

pub struct OpenTarget {
    pub ops: Arc<dyn SystemOps>,
    pub folders: Arc<UserFolders>,
}

#[async_trait]
impl Tool for OpenTarget {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "open".into(),
            description: "Opens a document, media file or folder with its default program, an http(s) link in the browser, or a Windows Settings page (ms-settings:bluetooth, ms-settings:display, ms-settings:sound, ms-settings:network-wifi, ms-settings:windowsupdate ...) at once. Programs and scripts are blocked. Not for music, video or chats when an app for it is installed: use that app (desktop_task) unless the user asks for the website."
                .into(),
            parameters: json!({ "type": "object", "properties": { "target": { "type": "string", "description": "Path (e.g. Desktop/report.pdf), https:// link, or ms-settings: page." } }, "required": ["target"] }),
        }
    }
    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Opening", "باز کردن"), short(arg_str(args, "target").unwrap_or(""), 40))
    }
    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        let target = arg_str(&args, "target").ok_or("target is required.")?;
        // A spotify: link opens in the Spotify app (training round 3: it was looked for as a file on the Desktop), and an
        // ms-settings: page in Windows Settings (11-11: "ms-settings:bluetooth" was looked for on the Desktop, and the
        // screen job that followed took 93 s). Only these: other schemes can start programs (ms-msdt:, search-ms:).
        if ["http://", "https://", "spotify:", "ms-settings:"].iter().any(|scheme| target.to_ascii_lowercase().starts_with(scheme)) {
            self.ops.open(target)?;
            return Ok(json!({ "opened": target }));
        }
        let path = self.folders.resolve(target);
        if let Some(why) = self.folders.blocked(&path, true) {
            return Err(format!("Access denied: {why}."));
        }
        let ext = Path::new(&path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        if BLOCKED_OPEN.contains(&ext.as_str()) {
            return Err("Opening programs and scripts is blocked.".into());
        }
        if !path.exists() {
            return Err(format!("Not found: {}", path.display()));
        }
        self.ops.open(&path.display().to_string())?;
        Ok(json!({ "opened": path.display().to_string() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::DenyAll;
    use crate::tools::test_support::ctx;
    use std::sync::Mutex;

    /// Records what is opened.
    #[derive(Default)]
    struct Opener(Mutex<Vec<String>>);

    impl SystemOps for Opener {
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
            self.0.lock().unwrap().push(target.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn windows_settings_pages_open_but_other_schemes_do_not() {
        let opener = Arc::new(Opener::default());
        let home = std::env::temp_dir().join(format!("anytask-open-{}", std::process::id()));
        let folders = Arc::new(UserFolders {
            desktop: home.join("Desktop"),
            downloads: home.join("Downloads"),
            documents: home.join("Documents"),
            pictures: home.join("Pictures"),
            music: home.join("Music"),
            videos: home.join("Videos"),
            home: home.clone(),
        });
        let tool = OpenTarget { ops: opener.clone(), folders };
        let ctx = ctx(Arc::new(DenyAll));
        tool.run(json!({ "target": "ms-settings:bluetooth" }), &ctx).await.unwrap();
        tool.run(json!({ "target": "https://example.com" }), &ctx).await.unwrap();
        assert_eq!(*opener.0.lock().unwrap(), ["ms-settings:bluetooth", "https://example.com"]);
        // Schemes that can start programs are not handed to Windows: they are looked for as files and not found.
        assert!(tool.run(json!({ "target": "ms-msdt:/id PCWDiagnostic" }), &ctx).await.is_err());
        assert!(tool.run(json!({ "target": "search-ms:query=x" }), &ctx).await.is_err());
        assert_eq!(opener.0.lock().unwrap().len(), 2);
    }

    fn song(title: &str, playing: bool) -> NowPlaying {
        NowPlaying { app: "Spotify".into(), title: title.into(), artist: "Shadmehr".into(), playing }
    }

    /// A system with a scripted media session: the play/pause key toggles it, next moves to "Song 2".
    struct Player {
        session: Mutex<Result<Option<NowPlaying>, String>>,
        /// The session can be started or paused directly (media_play).
        direct: bool,
        /// The player ignores every request (it stays as it is).
        stuck: bool,
        /// With no session, the play key starts a player that then shows up (Spotify just opened).
        wakes: bool,
        keys: Mutex<Vec<MediaKey>>,
        asked: Mutex<Vec<bool>>,
    }

    impl Player {
        fn new(session: Result<Option<NowPlaying>, String>, direct: bool) -> Arc<Self> {
            Self::with(session, direct, false)
        }
        fn with(session: Result<Option<NowPlaying>, String>, direct: bool, stuck: bool) -> Arc<Self> {
            Arc::new(Self { session: Mutex::new(session), direct, stuck, wakes: false, keys: Mutex::default(), asked: Mutex::default() })
        }
        fn keys(&self) -> Vec<MediaKey> {
            self.keys.lock().unwrap().clone()
        }
        fn asked(&self) -> Vec<bool> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl SystemOps for Player {
        fn clipboard_read(&self) -> Result<String, String> {
            Ok(String::new())
        }
        fn clipboard_write(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn media_key(&self, key: MediaKey) -> Result<(), String> {
            self.keys.lock().unwrap().push(key);
            let mut session = self.session.lock().unwrap();
            if self.wakes && key == MediaKey::PlayPause && matches!(*session, Ok(None)) {
                *session = Ok(Some(song("Song 1", true)));
                return Ok(());
            }
            if let (false, Ok(Some(player))) = (self.stuck, &mut *session) {
                match key {
                    MediaKey::PlayPause => player.playing = !player.playing,
                    MediaKey::Next => *player = song("Song 2", true),
                    MediaKey::Stop => player.playing = false,
                    _ => {}
                }
            }
            Ok(())
        }
        fn open(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn now_playing(&self) -> Result<Option<NowPlaying>, String> {
            self.session.lock().unwrap().clone()
        }
        fn media_play(&self, play: bool) -> Result<bool, String> {
            if !self.direct {
                return Err("unknown".into());
            }
            self.asked.lock().unwrap().push(play);
            if let (false, Ok(Some(player))) = (self.stuck, &mut *self.session.lock().unwrap()) {
                player.playing = play;
            }
            Ok(true)
        }
    }

    /// A system that only has media keys: it keeps the trait's defaults for the media session.
    #[derive(Default)]
    struct KeysOnly {
        keys: Mutex<Vec<MediaKey>>,
    }

    impl SystemOps for KeysOnly {
        fn clipboard_read(&self) -> Result<String, String> {
            Ok(String::new())
        }
        fn clipboard_write(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn media_key(&self, key: MediaKey) -> Result<(), String> {
            self.keys.lock().unwrap().push(key);
            Ok(())
        }
        fn open(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    async fn media(ops: Arc<dyn SystemOps>, args: Value) -> Result<Value, String> {
        MediaControl { ops }.run(args, &ctx(Arc::new(DenyAll))).await
    }

    #[test]
    fn now_playing_names_artist_and_title() {
        assert_eq!(song("Song 1", true).label(), "Shadmehr - Song 1");
        assert_eq!(NowPlaying { artist: String::new(), ..song("Song 1", true) }.label(), "Song 1");
        assert_eq!(NowPlaying { title: " ".into(), ..song("", true) }.label(), "Shadmehr");
    }

    #[tokio::test(start_paused = true)]
    async fn status_says_what_plays_and_presses_nothing() {
        let player = Player::new(Ok(Some(song("Song 1", true))), true);
        let out = media(player.clone(), json!({ "action": "status" })).await.unwrap();
        assert_eq!((out["playing"].clone(), out["now"].clone()), (json!(true), json!("Shadmehr - Song 1")));
        assert!(player.keys().is_empty() && player.asked().is_empty());
        let none = media(Player::new(Ok(None), true), json!({ "action": "status" })).await.unwrap();
        assert_eq!(none["playing"], json!(false));
    }

    #[tokio::test(start_paused = true)]
    async fn play_with_no_player_open_points_to_play_music() {
        let player = Player::new(Ok(None), true);
        let err = media(player.clone(), json!({ "action": "play" })).await.unwrap_err();
        assert!(err.contains("No music player is open") && err.contains("play_music"), "{err}");
        assert_eq!(player.keys(), vec![MediaKey::PlayPause], "one try with the key first");
        assert!(player.asked().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_just_opened_player_that_starts_on_the_key_is_reported_playing() {
        let player = Arc::new(Player { wakes: true, ..Arc::try_unwrap(Player::new(Ok(None), true)).ok().unwrap() });
        let out = media(player.clone(), json!({ "action": "play" })).await.unwrap();
        assert_eq!(out, json!({ "playing": true, "now": "Shadmehr - Song 1", "app": "Spotify" }));
        assert_eq!(player.keys(), vec![MediaKey::PlayPause]);
    }

    #[tokio::test(start_paused = true)]
    async fn play_while_playing_leaves_it_playing() {
        let player = Player::new(Ok(Some(song("Song 1", true))), true);
        let out = media(player.clone(), json!({ "action": "play" })).await.unwrap();
        assert_eq!(out, json!({ "playing": true, "already": true, "now": "Shadmehr - Song 1", "app": "Spotify" }));
        assert!(player.keys().is_empty(), "the toggle key would pause it");
        assert!(player.asked().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn play_while_paused_starts_it_and_says_so() {
        let player = Player::new(Ok(Some(song("Song 1", false))), true);
        let out = media(player.clone(), json!({ "action": "play" })).await.unwrap();
        assert_eq!(out, json!({ "playing": true, "now": "Shadmehr - Song 1", "app": "Spotify" }));
        assert_eq!(player.asked(), vec![true]);
        assert!(player.keys().is_empty(), "started directly, no toggle");
    }

    #[tokio::test(start_paused = true)]
    async fn play_while_paused_uses_the_key_when_it_cannot_ask_directly() {
        let player = Player::new(Ok(Some(song("Song 1", false))), false);
        let out = media(player.clone(), json!({ "action": "play" })).await.unwrap();
        assert_eq!(out["playing"], true);
        assert_eq!(player.keys(), vec![MediaKey::PlayPause]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_player_that_does_not_start_is_reported_as_not_playing() {
        let player = Player::with(Ok(Some(song("Song 1", false))), true, true);
        let started = Instant::now();
        let out = media(player.clone(), json!({ "action": "play" })).await.unwrap();
        assert_eq!(out["playing"], false);
        assert!(out["note"].as_str().unwrap().contains("did not start"), "{out}");
        assert!(started.elapsed() >= START_WAIT, "it gave the player time first");
    }

    #[tokio::test(start_paused = true)]
    async fn pause_pauses_only_what_plays() {
        let player = Player::new(Ok(Some(song("Song 1", true))), true);
        let out = media(player.clone(), json!({ "action": "pause" })).await.unwrap();
        assert_eq!(out["playing"], false);
        assert_eq!(player.asked(), vec![false]);

        let out = media(player.clone(), json!({ "action": "pause" })).await.unwrap();
        assert_eq!(out["already"], true, "already paused: the toggle key would start it");
        assert_eq!(player.asked(), vec![false]);
        assert!(player.keys().is_empty());

        // A player Windows does not list (VLC) still gets the key; nothing is claimed.
        let none = Player::new(Ok(None), true);
        let out = media(none.clone(), json!({ "action": "pause" })).await.unwrap();
        assert!(out.get("playing").is_none() && out["note"].as_str().unwrap().contains("does not list"), "{out}");
        assert_eq!(none.keys(), vec![MediaKey::PlayPause]);
    }

    #[tokio::test(start_paused = true)]
    async fn track_keys_with_no_listed_player_still_go_but_claim_nothing() {
        for (action, key) in [("play_pause", MediaKey::PlayPause), ("next", MediaKey::Next), ("previous", MediaKey::Previous)] {
            let player = Player::new(Ok(None), true);
            let out = media(player.clone(), json!({ "action": action })).await.unwrap();
            assert!(out.get("playing").is_none() && out.get("now").is_none(), "{action}: {out}");
            assert!(out["note"].as_str().unwrap().contains("Do not say that something plays"), "{action}: {out}");
            assert_eq!(player.keys(), vec![key], "{action}: VLC and other unlisted players still get the key");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn previous_presses_again_when_the_song_only_restarted() {
        // The fake player ignores "previous", like Spotify on the first press: the song stays the same.
        let player = Player::new(Ok(Some(song("Song 1", true))), true);
        let out = media(player.clone(), json!({ "action": "previous" })).await.unwrap();
        assert_eq!(player.keys(), vec![MediaKey::Previous, MediaKey::Previous]);
        assert_eq!(out["pressed"], 2, "{out}");
    }

    #[tokio::test(start_paused = true)]
    async fn next_reports_the_new_song() {
        let player = Player::new(Ok(Some(song("Song 1", true))), true);
        let out = media(player.clone(), json!({ "action": "next" })).await.unwrap();
        assert_eq!(out, json!({ "pressed": 1, "playing": true, "now": "Shadmehr - Song 2", "app": "Spotify" }));
        assert_eq!(player.keys(), vec![MediaKey::Next]);

        let slow = Player::with(Ok(Some(song("Song 1", true))), true, true);
        let out = media(slow.clone(), json!({ "action": "next" })).await.unwrap();
        assert_eq!(out, json!({ "pressed": 1, "playing": true, "app": "Spotify" }), "the old song is not named as the next one");
    }

    #[tokio::test(start_paused = true)]
    async fn play_pause_reports_the_new_state() {
        let player = Player::new(Ok(Some(song("Song 1", true))), true);
        let out = media(player.clone(), json!({ "action": "play_pause" })).await.unwrap();
        assert_eq!(out["playing"], false);
        assert_eq!(player.keys(), vec![MediaKey::PlayPause]);
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_view_of_the_player_play_falls_back_to_the_key() {
        let keys = Arc::new(KeysOnly::default());
        let out = media(keys.clone(), json!({ "action": "play" })).await.unwrap();
        assert_eq!(out, json!({ "pressed": 1, "note": "could not check whether something is playing now" }));
        assert_eq!(*keys.keys.lock().unwrap(), vec![MediaKey::PlayPause]);

        let out = media(keys.clone(), json!({ "action": "next" })).await.unwrap();
        assert_eq!(out, json!({ "pressed": 1 }), "pressed as before, nothing claimed");
    }

    #[tokio::test(start_paused = true)]
    async fn volume_works_with_no_player() {
        let player = Player::new(Ok(None), true);
        let out = media(player.clone(), json!({ "action": "volume_up", "times": 3 })).await.unwrap();
        assert_eq!(out, json!({ "pressed": 3 }));
        assert_eq!(player.keys(), vec![MediaKey::VolumeUp; 3]);
    }

    #[tokio::test]
    async fn unknown_action_is_refused() {
        let err = media(Arc::new(KeysOnly::default()), json!({ "action": "dance" })).await.unwrap_err();
        assert_eq!(err, "Unknown media action.");
    }

    /// Scripted speakers: set_volume changes them, volume reads them back.
    struct Speakers {
        state: Mutex<(u8, bool)>,
        sets: Mutex<Vec<(Option<u8>, Option<bool>)>>,
    }

    impl Speakers {
        fn new(level: u8, muted: bool) -> Arc<Self> {
            Arc::new(Self { state: Mutex::new((level, muted)), sets: Mutex::default() })
        }
        fn sets(&self) -> Vec<(Option<u8>, Option<bool>)> {
            self.sets.lock().unwrap().clone()
        }
    }

    impl SystemOps for Speakers {
        fn clipboard_read(&self) -> Result<String, String> {
            Ok(String::new())
        }
        fn clipboard_write(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn media_key(&self, _: MediaKey) -> Result<(), String> {
            Err("no keys here: the volume is set directly".into())
        }
        fn open(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn volume(&self) -> Result<(u8, bool), String> {
            Ok(*self.state.lock().unwrap())
        }
        fn set_volume(&self, level: Option<u8>, mute: Option<bool>) -> Result<(), String> {
            self.sets.lock().unwrap().push((level, mute));
            let mut state = self.state.lock().unwrap();
            if let Some(level) = level {
                state.0 = level;
            }
            if let Some(mute) = mute {
                state.1 = mute;
            }
            Ok(())
        }
    }

    async fn volume(ops: Arc<dyn SystemOps>, args: Value) -> Result<Value, String> {
        Volume { ops }.run(args, &ctx(Arc::new(DenyAll))).await
    }

    #[tokio::test]
    async fn volume_is_set_to_the_exact_level() {
        let speakers = Speakers::new(50, false);
        let out = volume(speakers.clone(), json!({ "level": 30 })).await.unwrap();
        assert_eq!(out, json!({ "before": { "level": 50, "muted": false }, "now": { "level": 30, "muted": false } }));
        assert_eq!(speakers.sets(), vec![(Some(30), None)], "already unmuted: mute is left alone");

        // Models also send 30.0 or "30%"; out of range is held at 0-100.
        for (asked, set) in [(json!(62.6), 63), (json!("45%"), 45), (json!(150), 100), (json!(-5), 0)] {
            let out = volume(speakers.clone(), json!({ "level": asked })).await.unwrap();
            assert_eq!(out["now"]["level"], set, "{asked}");
        }
        let err = volume(speakers.clone(), json!({ "level": "loud" })).await.unwrap_err();
        assert_eq!(err, "level must be a number.");
    }

    #[tokio::test]
    async fn relative_change_stays_within_0_and_100() {
        let speakers = Speakers::new(50, false);
        let out = volume(speakers.clone(), json!({ "change": -10 })).await.unwrap();
        assert_eq!(out["now"], json!({ "level": 40, "muted": false }));

        let loud = Speakers::new(95, false);
        let out = volume(loud.clone(), json!({ "change": 20 })).await.unwrap();
        assert_eq!(out, json!({ "before": { "level": 95, "muted": false }, "now": { "level": 100, "muted": false } }));

        let quiet = Speakers::new(5, false);
        let out = volume(quiet.clone(), json!({ "change": "-10" })).await.unwrap();
        assert_eq!(out["now"], json!({ "level": 0, "muted": false }));
        assert_eq!(quiet.sets(), vec![(Some(0), None)]);

        // An exact level wins over a change sent with it.
        let out = volume(speakers.clone(), json!({ "level": 70, "change": -10 })).await.unwrap();
        assert_eq!(out["now"]["level"], 70);
    }

    #[tokio::test]
    async fn mute_and_unmute_keep_the_level() {
        let speakers = Speakers::new(40, false);
        let out = volume(speakers.clone(), json!({ "mute": true })).await.unwrap();
        assert_eq!(out, json!({ "before": { "level": 40, "muted": false }, "now": { "level": 40, "muted": true } }));
        let out = volume(speakers.clone(), json!({ "mute": false })).await.unwrap();
        assert_eq!(out["now"], json!({ "level": 40, "muted": false }));
        assert_eq!(speakers.sets(), vec![(None, Some(true)), (None, Some(false))]);
        let err = volume(speakers.clone(), json!({ "mute": "yes" })).await.unwrap_err();
        assert_eq!(err, "mute must be true or false.");
    }

    #[tokio::test]
    async fn setting_a_level_above_0_unmutes() {
        let speakers = Speakers::new(40, true);
        let out = volume(speakers.clone(), json!({ "level": 30 })).await.unwrap();
        assert_eq!(out, json!({ "before": { "level": 40, "muted": true }, "now": { "level": 30, "muted": false } }));
        assert_eq!(speakers.sets(), vec![(Some(30), Some(false))]);

        // So does a change; level 0 and an explicit mute stay muted.
        let changed = Speakers::new(40, true);
        assert_eq!(volume(changed.clone(), json!({ "change": 10 })).await.unwrap()["now"], json!({ "level": 50, "muted": false }));
        let zero = Speakers::new(40, true);
        assert_eq!(volume(zero.clone(), json!({ "level": 0 })).await.unwrap()["now"], json!({ "level": 0, "muted": true }));
        let kept = Speakers::new(40, true);
        assert_eq!(volume(kept.clone(), json!({ "level": 20, "mute": true })).await.unwrap()["now"], json!({ "level": 20, "muted": true }));
    }

    #[tokio::test]
    async fn with_no_arguments_volume_only_reports() {
        let speakers = Speakers::new(37, true);
        let out = volume(speakers.clone(), json!({})).await.unwrap();
        assert_eq!(out, json!({ "now": { "level": 37, "muted": true } }));
        assert!(speakers.sets().is_empty(), "nothing was changed");
    }

    #[tokio::test]
    async fn a_system_without_volume_points_to_the_volume_keys() {
        // KeysOnly keeps the trait's defaults: it cannot read or set the level.
        let keys = Arc::new(KeysOnly::default());
        let err = volume(keys.clone(), json!({ "level": 30 })).await.unwrap_err();
        assert!(err.contains("media_control volume_up / volume_down") && err.contains("do not claim an exact level"), "{err}");
        assert!(keys.keys.lock().unwrap().is_empty(), "the model decides about the keys");
    }

    #[test]
    fn volume_labels_are_persian() {
        let tool = Volume { ops: Speakers::new(50, false) };
        assert_eq!(tool.label(&json!({ "level": 30 })), "Volume 30%");
        assert_eq!(tool.label(&json!({ "mute": true })), "Mute");
        assert_eq!(tool.label(&json!({ "mute": false })), "Unmute");
        assert_eq!(tool.label(&json!({ "change": -10 })), "Volume down 10%");
        assert_eq!(tool.label(&json!({ "change": 20 })), "Volume up 20%");
        assert_eq!(tool.label(&json!({})), "Volume");
        // The model works out "half" from the report; the description has to say so.
        let description = tool.spec().description;
        assert!(description.contains("For \"half\" (nesf), set level to half of the current level"), "{description}");
        assert!(description.contains("not media_control volume_up/volume_down"), "{description}");
    }
}
