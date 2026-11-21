//! Opening an installed app without the screen agent: start it (or bring its open window to the front) and wait
//! until its window is in front. A few seconds, instead of two rounds of the screen model (often 20 s each).
//!
//! Success is judged by windows, not by titles alone: the app's own window (found by its program) must be in front,
//! or, for an app whose program cannot be matched, a NEW window must have come to the front. A window that was in
//! front before ("AnyTask Settings" when opening Settings) never counts.

use super::desktop::DesktopOps;
use super::{Tool, ToolCtx, arg_str, short};
use crate::lang::tr;
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// How long to wait for the app's window to come to the front (a cold start of a big app takes a few seconds).
const WAIT: Duration = Duration::from_secs(12);
const POLL: Duration = Duration::from_millis(250);
/// How long a closed app gets to go away before it is taken as asking the user something.
const CLOSE_WAIT: Duration = Duration::from_secs(3);
/// The app's window exists but is not in front after this: bring it forward (Windows keeps windows of programs
/// started from the background behind).
const PUSH_AFTER: Duration = Duration::from_millis(1500);
/// A new window whose title does not name the app is accepted after it stayed in front this long.
const NEW_WINDOW_CALM: Duration = Duration::from_millis(1500);

/// Words of an app name that tell it apart: "Microsoft Teams" -> teams, "Google Chrome" -> chrome.
fn telling_words(app: &str) -> Vec<String> {
    const VENDOR: &[&str] = &["microsoft", "google", "windows", "app", "desktop", "the", "for"];
    let words: Vec<String> = app.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(str::to_string).collect();
    let telling: Vec<String> = words.iter().filter(|w| !VENDOR.contains(&w.as_str())).cloned().collect();
    if telling.is_empty() { words } else { telling }
}

/// The window title names the app as a whole word ("New Tab - Google Chrome" for Google Chrome, "Spotify Free" for
/// Spotify; not "Inbox - Microsoft Edge" for Microsoft Teams, not "knowledge" for Edge).
fn shows(title: &str, app: &str) -> bool {
    let title: Vec<String> = title.to_lowercase().split(|c: char| !c.is_alphanumeric()).map(str::to_string).collect();
    telling_words(app).iter().any(|w| title.contains(w))
}

pub struct OpenApp {
    pub ops: Arc<dyn DesktopOps>,
}

impl OpenApp {
    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&dyn DesktopOps) -> Result<T, String> + Send + 'static) -> Result<T, String> {
        let ops = self.ops.clone();
        tokio::task::spawn_blocking(move || f(ops.as_ref())).await.map_err(|e| format!("desktop worker stopped: {e}"))?
    }

    /// The window in front, its title, and the app's own window if it has one.
    async fn screen(&self, app: &str) -> (isize, String, Option<isize>) {
        let app = app.to_string();
        self.blocking(move |ops| Ok((ops.foreground(), ops.active_title(), ops.app_window(&app)))).await.unwrap_or((0, String::new(), None))
    }
}

#[async_trait]
impl Tool for OpenApp {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "open_app".into(),
            description: "Opens an installed app by its Start-menu name (Spotify, Telegram, Google Chrome, Notepad, Calculator, Settings, \
                          ...), or brings it to the front when it is already open. Takes a few seconds and does not need the screen. Use it \
                          when the user only wants an app opened; for work inside the app use desktop_task with the whole goal."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": { "app": { "type": "string", "description": "The app's name as in the Start menu, in English (e.g. Telegram, not تلگرام)." } },
                "required": ["app"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Opening", "باز کردن"), short(arg_str(args, "app").unwrap_or(""), 30))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let app = arg_str(&args, "app").ok_or("app is required.")?.to_string();
        let (before, _, _) = self.screen(&app).await;
        let name = app.clone();
        self.blocking(move |ops| ops.launch_app(&name)).await?;
        let started = Instant::now();
        let (mut pushed, mut new_since): (bool, Option<Instant>) = (false, None);
        let mut title = String::new();
        while started.elapsed() < WAIT {
            let (front, window);
            (front, title, window) = self.screen(&app).await;
            let new_front = front != 0 && front != before;
            let found = match window {
                Some(own) if own == front => true,
                Some(own) => {
                    if !pushed && started.elapsed() >= PUSH_AFTER {
                        pushed = true;
                        let _ = self.blocking(move |ops| ops.bring_to_front(own)).await;
                    }
                    false
                }
                // The app's program is not known by name: a new window that names it, or one that stays in front.
                None if new_front => shows(&title, &app) || new_since.get_or_insert_with(Instant::now).elapsed() >= NEW_WINDOW_CALM,
                None => {
                    new_since = None;
                    false
                }
            };
            if found {
                (ctx.log)(format!("open_app: \"{title}\" in front after {:.1} s", started.elapsed().as_secs_f32()));
                return Ok(json!({ "opened": true, "app": app, "window": title }));
            }
            tokio::select! {
                _ = ctx.cancel.cancelled() => return Err("Stopped".into()),
                _ = tokio::time::sleep(POLL) => {}
            }
        }
        (ctx.log)(format!("open_app: no window of {app} came to the front within {} s (in front: \"{title}\")", WAIT.as_secs()));
        Ok(json!({
            "opened": false,
            "app": app,
            "window": title,
            "note": format!("{app} was started, but no window of it came to the front within {} s. It may still be starting, run only in the tray, or be installed under another name; check with desktop_task before telling the user it is open.", WAIT.as_secs())
        }))
    }
}


/// "Spotify ro bebad": the app's windows are asked to close like their X button, so the app itself asks the user
/// about unsaved work. A screen job may never close windows (it once answered "Don't save" for the user), and on
/// 11-10 it worked around that through the window menu and Task Manager.
pub struct CloseApp {
    pub ops: Arc<dyn DesktopOps>,
}

/// Programs that are never closed this way: the assistant itself, the Windows shell and system tools.
fn never_close(app: &str) -> bool {
    let app = app.trim().to_lowercase();
    [
        "anytask", "explorer", "file explorer", "windows explorer", "task manager", "taskmgr", "windows", "system", "settings", "start", "taskbar",
        // Closing a console ends what runs in it.
        "cmd", "command prompt", "powershell", "windows powershell", "terminal", "windows terminal",
    ]
    .contains(&app.as_str())
}

#[async_trait]
impl Tool for CloseApp {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "close_app".into(),
            description: "Closes an app the user asked to close or quit (\"Spotify ro bebad\"), like pressing its X: the app itself asks \
                          the user about unsaved work. Only when the user asks to close an app; never to tidy up. Never use desktop_task \
                          to close windows."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": { "app": { "type": "string", "description": "The app's name as in the Start menu, in English (e.g. Spotify, Notepad)." } },
                "required": ["app"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Closing", "بستن"), short(arg_str(args, "app").unwrap_or(""), 30))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let app = arg_str(&args, "app").map(str::trim).filter(|a| !a.is_empty()).ok_or("app is required: the name of the app to close.")?.to_string();
        if never_close(&app) {
            return Err(format!("{app} is not closed this way (it is part of Windows or the assistant itself)."));
        }
        let ops = self.ops.clone();
        let name = app.clone();
        let asked = tokio::task::spawn_blocking(move || ops.close_app(&name)).await.map_err(|e| format!("desktop worker stopped: {e}"))??;
        if asked == 0 {
            return Ok(json!({ "app": app, "closed": false, "note": format!("{app} has no open window, so there was nothing to close.") }));
        }
        (ctx.log)(format!("asked {asked} window(s) of {app} to close"));
        // Give it a moment to go; a window that stays is usually its own "save changes?" question to the user.
        let started = Instant::now();
        loop {
            let ops = self.ops.clone();
            let name = app.clone();
            let open = tokio::task::spawn_blocking(move || ops.app_window(&name).is_some()).await.unwrap_or(false);
            if !open {
                return Ok(json!({ "app": app, "closed": true }));
            }
            if started.elapsed() > CLOSE_WAIT {
                return Ok(json!({
                    "app": app, "closed": false,
                    "note": format!("{app} is still open: it may be asking whether to save. That question is the user's to answer; do not press anything for them.")
                }));
            }
            tokio::select! {
                _ = ctx.cancel.cancelled() => return Err("cancelled".into()),
                _ = tokio::time::sleep(POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::DenyAll;
    use crate::tools::desktop::{Look, MouseButton};
    use crate::tools::test_support::ctx;
    use std::sync::Mutex;

    /// What launching does on the fake desktop.
    #[derive(Clone, Copy)]
    enum Launch {
        /// Nothing comes up.
        Nothing,
        /// The app's own window (handle 7) opens in front.
        OwnWindow,
        /// The app's window (handle 7) opens behind; only bringing it forward puts it in front.
        Behind,
        /// A new window (handle 9) comes up whose program is not known by name.
        Unknown,
    }

    struct Desk {
        front: Mutex<(isize, String)>,
        own: Mutex<Option<isize>>,
        launch: Launch,
        title: &'static str,
        launched: Mutex<Vec<String>>,
    }

    fn desk(front: (isize, &str), own: Option<isize>, launch: Launch, title: &'static str) -> Arc<Desk> {
        Arc::new(Desk { front: Mutex::new((front.0, front.1.into())), own: Mutex::new(own), launch, title, launched: Mutex::new(Vec::new()) })
    }

    impl DesktopOps for Desk {
        fn look(&self) -> Result<Look, String> {
            Err("not used".into())
        }
        fn click(&self, _: u16, _: u16, _: MouseButton, _: u8) -> Result<(), String> {
            Ok(())
        }
        fn scroll(&self, _: u16, _: u16, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn type_text(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn hotkey(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn launch_app(&self, name: &str) -> Result<(), String> {
            if name == "Nope" {
                return Err("Nope is not installed.".into());
            }
            self.launched.lock().unwrap().push(name.into());
            match self.launch {
                Launch::Nothing => {}
                Launch::OwnWindow => {
                    *self.own.lock().unwrap() = Some(7);
                    *self.front.lock().unwrap() = (7, self.title.into());
                }
                Launch::Behind => *self.own.lock().unwrap() = Some(7),
                Launch::Unknown => *self.front.lock().unwrap() = (9, self.title.into()),
            }
            Ok(())
        }
        fn open_url(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn close_app(&self, _: &str) -> Result<usize, String> {
            Ok(self.own.lock().unwrap().take().map_or(0, |_| 1))
        }
        fn active_title(&self) -> String {
            self.front.lock().unwrap().1.clone()
        }
        fn foreground(&self) -> isize {
            self.front.lock().unwrap().0
        }
        fn bring_to_front(&self, window: isize) -> Result<(), String> {
            *self.front.lock().unwrap() = (window, self.title.into());
            Ok(())
        }
        fn app_window(&self, _: &str) -> Option<isize> {
            *self.own.lock().unwrap()
        }
    }

    async fn open(desk: Arc<Desk>, app: &str) -> Value {
        OpenApp { ops: desk }.run(json!({ "app": app }), &ctx(Arc::new(DenyAll))).await.unwrap()
    }

    #[test]
    fn titles_name_the_app_with_a_telling_whole_word() {
        assert!(shows("New Tab - Google Chrome", "Google Chrome"));
        assert!(shows("Spotify Free", "spotify"));
        assert!(shows("Untitled - Notepad", "Notepad"));
        assert!(shows("Inbox - Microsoft\u{200b} Edge", "Microsoft Edge"));
        assert!(!shows("Inbox - Microsoft Edge", "Microsoft Teams"));
        assert!(!shows("Knowledge base - Notepad", "Edge"));
        assert!(!shows("Home", "File Explorer"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_app_opens_without_the_screen_model() {
        let desk = desk((1, "AnyTask Settings"), None, Launch::OwnWindow, "Spotify Free");
        assert_eq!(open(desk.clone(), "Spotify").await, json!({ "opened": true, "app": "Spotify", "window": "Spotify Free" }));
        assert_eq!(*desk.launched.lock().unwrap(), vec!["Spotify".to_string()]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_window_that_was_in_front_does_not_count() {
        // "Settings" is a word of "AnyTask Settings", but nothing new came up.
        let out = open(desk((1, "AnyTask Settings"), None, Launch::Nothing, ""), "Settings").await;
        assert_eq!(out["opened"], false, "{out}");
        assert!(out["note"].as_str().unwrap().contains("desktop_task"), "{out}");
        let out = open(desk((1, "Inbox - Microsoft Edge"), None, Launch::Nothing, ""), "Microsoft Teams").await;
        assert_eq!(out["opened"], false, "{out}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_app_already_in_front_is_open() {
        let out = open(desk((7, "Spotify Free"), Some(7), Launch::Nothing, "Spotify Free"), "Spotify").await;
        assert_eq!(out["opened"], true, "{out}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_window_kept_behind_is_brought_forward() {
        let out = open(desk((1, "AnyTask"), None, Launch::Behind, "Telegram"), "Telegram").await;
        assert_eq!(out, json!({ "opened": true, "app": "Telegram", "window": "Telegram" }));
    }

    #[tokio::test(start_paused = true)]
    async fn a_new_window_of_an_unknown_program_counts_once_it_stays() {
        let out = open(desk((1, "AnyTask"), None, Launch::Unknown, "Home"), "File Explorer").await;
        assert_eq!(out, json!({ "opened": true, "app": "File Explorer", "window": "Home" }));
    }

    #[tokio::test]
    async fn a_launch_error_is_reported() {
        let err = OpenApp { ops: desk((1, "Desktop"), None, Launch::Nothing, "") }.run(json!({ "app": "Nope" }), &ctx(Arc::new(DenyAll))).await.unwrap_err();
        assert!(err.contains("not installed"), "{err}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_app_the_user_asked_to_close_is_closed() {
        let d = desk((7, "Spotify Free"), Some(7), Launch::Nothing, "Spotify Free");
        let tool = CloseApp { ops: d.clone() };
        let closed = tool.run(json!({ "app": "Spotify" }), &ctx(Arc::new(DenyAll))).await.unwrap();
        assert_eq!(closed["closed"], true, "{closed}");
        let again = tool.run(json!({ "app": "Spotify" }), &ctx(Arc::new(DenyAll))).await.unwrap();
        assert_eq!(again["closed"], false);
        assert!(again["note"].as_str().unwrap().contains("no open window"));
        assert!(tool.run(json!({ "app": "AnyTask" }), &ctx(Arc::new(DenyAll))).await.is_err(), "never itself");
        assert!(tool.run(json!({ "app": "File Explorer" }), &ctx(Arc::new(DenyAll))).await.is_err(), "never the shell");
    }
}
