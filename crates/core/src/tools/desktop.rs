//! Desktop control: the model looks at the screen, picks the next action (or a few that need no new look), the
//! actions run, the screen settles, and it looks again.
//! Ported from v1 gui-agent.js with its lessons: controls from UI Automation instead of guessed pixels (clicked by
//! their names since 11-12: a name is copied right where a box number was misread, and lands on the control's middle),
//! notes for facts found on the way, spotting actions that changed nothing and loops, typing long or
//! non-English text by pasting. New: an `ask` action before anything that cannot be undone; waiting until the
//! screen has settled (each kind of action has its own limit) instead of fixed pauses, so a screenshot is never
//! taken before an app has opened; short batches such as launch + new document + type in one decision.
//!
//! Since 11-13 this is the executor of a two-agent job: the planner (planner.rs) writes the steps first from the
//! goal alone, with the long rules and its own memory, and this loop follows them with only the rules every step
//! needs. The screen work itself is behind `DesktopOps` (crates/control on Windows), so the loop is testable.

use super::{Judge, Tool, ToolCtx, arg_str, short};
use crate::lang::tr;
use anytask_llm::{GenerateRequest, Message, Part, Role, Thinking, ToolSpec};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_STEPS: usize = 60;
const MAX_RUNTIME: Duration = Duration::from_secs(15 * 60);
const HISTORY: usize = 12;
const MAX_NOTES: usize = 20;
const NOTE_CHARS: usize = 300;
const MAX_TYPED: usize = 4000;
const MAX_BATCH: usize = 3;
/// At most this many scroll steps in one action.
const MAX_SCROLL: u8 = 10;
const POLL_MS: u64 = 200;
/// How long a web page may take to load after open_url (its tab shows the bare address until then).
const PAGE_LOAD: Duration = Duration::from_secs(12);
/// A screen step without an answer after this is asked a second time alongside the first.
const HEDGE_AFTER: Duration = Duration::from_secs(8);
/// How long both asks together may take before the step fails.
const DECISION_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_VISION_FAILURES: usize = 3;
const MAX_ACTION_FAILURES: usize = 5;
/// Looks in a row that found the agent going in circles before the task is stopped.
const MAX_LOOP_WARNINGS: usize = 3;
/// How often a "fail" answer is sent back for another try when the task is not stuck.
const MAX_FAIL_PUSHBACKS: usize = 2;

/// A control found on screen. Positions are 0..1000 of the screen width/height, like the model's coordinates.
/// Controls have no numbers: the model names the one it means, and the click lands on its exact middle.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Element {
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// The exact middle on the screen, 0..1000 with fractions, from the control's own pixel rectangle (x, y, w and h
    /// are rounded, which can put the rounded middle a few pixels off on a small icon). None: the rounded middle.
    pub mid: Option<(f32, f32)>,
}

impl Element {
    pub fn new(kind: &str, name: &str, enabled: bool, (x, y, w, h): (u16, u16, u16, u16)) -> Self {
        Self { kind: kind.into(), name: name.into(), enabled, x, y, w, h, mid: None }
    }

    /// The middle as shown to the model ("@ X Y").
    pub fn center(&self) -> (u16, u16) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    /// The point a click on this control goes to: its exact middle when known.
    pub fn middle(&self) -> (f32, f32) {
        self.mid.unwrap_or_else(|| {
            let (x, y) = self.center();
            (f32::from(x), f32::from(y))
        })
    }
}

/// One look at the screen: a JPEG (with rulers for X Y), its controls and the active window.
#[derive(Debug, Clone, Default)]
pub struct Look {
    pub jpeg: Vec<u8>,
    pub title: String,
    pub elements: Vec<Element>,
    /// Changes when the picture changes; used to tell whether an action did anything.
    pub fingerprint: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}

/// The operating-system side. Coordinates are 0..1000 of the primary screen.
pub trait DesktopOps: Send + Sync {
    fn look(&self) -> Result<Look, String>;
    fn click(&self, x: u16, y: u16, button: MouseButton, count: u8) -> Result<(), String>;
    /// A click on an exact point (0..1000 with fractions): a control's own middle, to the pixel.
    fn click_exact(&self, x: f32, y: f32, button: MouseButton, count: u8) -> Result<(), String> {
        self.click(x.round().clamp(0.0, 1000.0) as u16, y.round().clamp(0.0, 1000.0) as u16, button, count)
    }
    fn scroll(&self, x: u16, y: u16, up: bool) -> Result<(), String>;
    /// Writes text where the keyboard focus is (`\n` = Enter).
    fn type_text(&self, text: &str) -> Result<(), String>;
    /// A key or combination such as `ctrl+l`, `enter`, `alt+left`, `media_play_pause`.
    fn hotkey(&self, combo: &str) -> Result<(), String>;
    fn launch_app(&self, name: &str) -> Result<(), String>;
    fn open_url(&self, url: &str) -> Result<(), String>;
    /// A cheap fingerprint of the screen (no controls, no JPEG) for waiting until it settles.
    fn fingerprint(&self) -> Result<u64, String> {
        self.look().map(|l| l.fingerprint)
    }
    /// Title of the active window.
    fn active_title(&self) -> String {
        String::new()
    }
    /// The window in front (an opaque handle; 0 = unknown), to bring it back after the assistant's own question card
    /// took the focus.
    fn foreground(&self) -> isize {
        0
    }
    fn bring_to_front(&self, _window: isize) -> Result<(), String> {
        Ok(())
    }
    /// The open window of the app with this name, found by its program (None when it has none or it is unknown).
    fn app_window(&self, _name: &str) -> Option<isize> {
        None
    }
    /// The front-most window of another program: where the user was before the assistant's own window came up.
    fn user_window(&self) -> Option<isize> {
        None
    }
    /// The window in front belongs to the assistant itself (its chat, settings or island). Told by the program, not
    /// the title: the user's own windows can have "AnyTask" in their title too ("AnyTask - Copy - Visual Studio Code").
    fn own_window_in_front(&self) -> bool {
        false
    }
    /// The default web browser's name ("Vivaldi"), if known.
    fn default_browser(&self) -> Option<String> {
        None
    }
    /// Asks every window of the app with this name to close, as its X button does (the app asks the user itself
    /// about unsaved work). Returns how many windows were asked; 0 when the app has none open.
    fn close_app(&self, _name: &str) -> Result<usize, String> {
        Err("Closing apps is not supported on this system.".into())
    }
    /// The player Windows knows about: whether it plays, and "Artist - Song" (None: this system cannot tell).
    fn now_playing(&self) -> Option<(bool, String)> {
        None
    }
    /// All text visible in the window in front, read from its pixels (OCR); None: this system cannot read it.
    /// Used to check what a "done" claims to see.
    fn screen_text(&self) -> Option<String> {
        None
    }
}

/// A number the "done" says is on screen that is nowhere in the visible text, the window title or the controls,
/// leaving out the numbers of the goal itself (they are what was typed). 11-11: Calculator lost the typed keys and
/// showed 0, yet the model answered "shows 4140", computing it itself.
fn unseen_number(seen: &str, goal: &str, visible: &str) -> Option<String> {
    let digits_of = |text: &str| -> Vec<String> {
        let ascii: String = text
            .chars()
            .map(|c| match c {
                '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
                '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
                _ => c,
            })
            .collect();
        // "4,140", "4 140" and "4٬140" are one number; a lone dot or comma between digits groups them too.
        let mut numbers = Vec::new();
        let mut current = String::new();
        let chars: Vec<char> = ascii.chars().collect();
        for (i, c) in chars.iter().enumerate() {
            let joins = matches!(c, ',' | '٬' | '.' | '\'' | '\u{a0}') && i > 0 && chars[i - 1].is_ascii_digit() && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
            if c.is_ascii_digit() {
                current.push(*c);
            } else if !joins && !current.is_empty() {
                numbers.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            numbers.push(current);
        }
        numbers
    };
    let flat = |text: &str| digits_of(text).concat();
    let from_goal = digits_of(goal);
    let on_screen = flat(visible);
    digits_of(seen).into_iter().filter(|n| n.len() >= 3 && !from_goal.contains(n)).find(|n| !on_screen.contains(n.as_str()))
}

// ---------------------------------------------------------------- decisions

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Click { target: Target, button: MouseButton, count: u8 },
    /// `times` steps of five wheel notches (about a third of a page each), 1 to MAX_SCROLL.
    Scroll { target: Target, up: bool, times: u8 },
    TypeInto { target: Target, text: String, enter: bool },
    Type { text: String, enter: bool },
    Hotkey(String),
    LaunchApp(String),
    OpenUrl(String),
    Wait,
    Note(String),
    Ask(String),
    Done(String),
    Fail(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// A point the model read off the rulers (only for what has no name).
    Point(u16, u16),
    /// A control or text line by the name the model read in the list, optionally the one nearest a point (when names
    /// repeat). Found on the look it was chosen from and clicked on its exact middle.
    Name(String, Option<(u16, u16)>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub action: Action,
    /// Extra time the model asked for with "(wait N)" or "wait : N"; 0 = only wait until the screen settles.
    pub wait_ms: u64,
    /// The decision as one normalised line, kept in the history.
    pub raw: String,
}

/// How long to wait after an action: at least `min`, then until the screen has settled, at most `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settle {
    pub min_ms: u64,
    pub max_ms: u64,
    /// The action should visibly change something (wait for that change before calling the screen settled).
    pub expect_change: bool,
    /// A new window should come to the front (launching an app, opening a page).
    pub new_window: bool,
}

impl Decision {
    pub fn settle(&self) -> Settle {
        let s = |min_ms, max_ms, expect_change, new_window| Settle { min_ms, max_ms, expect_change, new_window };
        let base = match &self.action {
            Action::Click { .. } => s(250, 2500, true, false),
            Action::Scroll { .. } => s(200, 1500, true, false),
            Action::TypeInto { enter: true, .. } | Action::Type { enter: true, .. } => s(400, 4000, true, false),
            Action::TypeInto { .. } | Action::Type { .. } => s(150, 1200, true, false),
            // A player shows play/pause in its window title a moment later ("Spotify Free" <-> "Artist - Song"):
            // looking before that made the agent press play/pause again and again (test 3: 7 presses in 53 s).
            Action::Hotkey(k) if k.starts_with("media_") => s(300, 2500, false, true),
            Action::Hotkey(k) if k.starts_with("volume_") => s(150, 600, false, false),
            Action::Hotkey(_) => s(250, 2500, true, false),
            Action::LaunchApp(_) => s(600, 6000, true, true),
            Action::OpenUrl(_) => s(1500, 8000, true, true),
            Action::Wait => s(self.wait_ms, self.wait_ms, false, false),
            _ => s(0, 0, false, false),
        };
        Settle { min_ms: base.min_ms.max(self.wait_ms), max_ms: base.max_ms.max(self.wait_ms), ..base }
    }
}

/// Reads the model's answer: the first valid action line, plus up to two more when they need no new look at the
/// screen (typing, keys, waiting, launching). Bullets, numbering and code marks are ignored.
pub fn parse_decisions(text: &str) -> Vec<Decision> {
    let mut out: Vec<Decision> = Vec::new();
    let lines = text
        .lines()
        .map(|l| l.trim().trim_matches(|c| c == '`' || c == '*').trim_start_matches(|c: char| c == '-' || c == '•' || c.is_whitespace()).trim())
        .filter(|l| !l.is_empty());
    for line in lines {
        let Some(decision) = parse_line(line) else {
            if out.is_empty() {
                continue;
            }
            break;
        };
        let ends_batch = matches!(decision.action, Action::Note(_) | Action::Ask(_) | Action::Done(_) | Action::Fail(_));
        if out.is_empty() {
            out.push(decision);
            if ends_batch {
                break;
            }
            continue;
        }
        // Only keyboard steps follow; a second launch or page would act on something nobody has seen yet.
        let follow_up = matches!(decision.action, Action::Type { .. } | Action::Hotkey(_) | Action::Wait);
        if !follow_up || out.len() >= MAX_BATCH {
            break;
        }
        out.push(decision);
    }
    out
}

/// A window title that shows a new, empty document (Notepad "Untitled", Word "Document1", a browser "New Tab").
/// A browser tab that is still loading shows the bare address as its title ("mail.google.com - Vivaldi").
pub fn loading_title(title: &str) -> bool {
    let page = title.rsplit_once(" - ").map(|(page, _)| page).unwrap_or(title).trim();
    page.is_empty() || page.eq_ignore_ascii_case("new tab") || page.starts_with("http") || (page.contains('.') && !page.contains(' ') && !page.contains('/'))
}

pub fn fresh_document(title: &str) -> bool {
    let t = title.trim_start_matches('*').to_lowercase();
    ["untitled", "new tab", "new document", "document1", "بدون عنوان"].iter().any(|w| t.contains(w))
}

/// A launched single-window app is in front by its own name ("Calculator" after launch_app Calculator): keys typed
/// next go to it. A window titled "file - App" is a document that may hold the user's work, so it still needs a look
/// first (11-10: "launch_app : Calculator | type_enter : 123 * 45" skipped the typing and cost a model round).
fn launched_app_ready(action: &Action, title: &str) -> bool {
    // Only apps where typed keys can do no harm: in a chat app (Telegram, WhatsApp) typing and Enter would send a
    // message to whatever chat is open, and in Spotify a space toggles play (review of f91fa7e).
    const SAFE: &[&str] = &["calculator"];
    let Action::LaunchApp(app) = action else { return false };
    let (app, title) = (app.trim().to_lowercase(), title.trim().to_lowercase());
    SAFE.contains(&app.as_str()) && title == app
}

/// Text to type as the model wrote it: "\n" is a new line, except in a Windows path, where it is a folder that
/// starts with n (11-10: "C:\Users\...\Desktop\night-shift-test\kharid2.txt" was typed as "Desktop", Enter,
/// "ight-shift-test\kharid2.txt" nine times into a Save as box).
fn typed(text: &str) -> String {
    if windows_path(text) { text.to_string() } else { text.replace("\\n", "\n") }
}

/// Holds a Windows path: a drive letter ("C:\", at the start or after a space or quote), "\\server", ".\", or
/// "%NAME%\". A label before a new line ("Items:\nmilk") is not one.
fn windows_path(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let drive = chars.windows(3).enumerate().any(|(i, w)| {
        w[0].is_ascii_alphabetic() && w[1] == ':' && w[2] == '\\' && (i == 0 || !chars[i - 1].is_alphanumeric())
    });
    let env = text.starts_with('%') && text[1..].find('%').is_some_and(|end| text[1..].get(end + 1..).is_some_and(|rest| rest.starts_with('\\')));
    drive || text.starts_with("\\\\") || text.starts_with(".\\") || env
}

/// The text a type step wrote, if it is one ("type_into : 53 : C:\x.txt" -> "C:\x.txt").
fn typed_text(raw: &str) -> Option<&str> {
    if raw.starts_with("type_into") {
        raw.splitn(3, " : ").nth(2)
    } else if raw.starts_with("type") {
        raw.split_once(" : ").map(|(_, t)| t)
    } else {
        None
    }
}

/// The first decision only (single-step callers and tests).
pub fn parse_decision(text: &str) -> Option<Decision> {
    parse_decisions(text).into_iter().next()
}

fn clamp(v: f64) -> u16 {
    v.round().clamp(0.0, 1000.0) as u16
}

fn point(params: &str) -> Option<(u16, u16)> {
    let nums: Vec<f64> = params.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).map(str::parse).collect::<Result<_, _>>().ok()?;
    (nums.len() == 2).then(|| (clamp(nums[0]), clamp(nums[1])))
}

/// A bare number ("12", "#12"): never a control's name (controls have no numbers).
fn number(text: &str) -> bool {
    text.trim().trim_start_matches('#').parse::<u32>().is_ok()
}

/// A name in quotes at the start of `text` ("\"Play\" @ 500 300 ..."): the name and what follows it.
fn quoted(text: &str) -> Option<(String, &str)> {
    let text = text.trim_start();
    let open = text.chars().next().filter(|c| matches!(c, '"' | '\u{201c}' | '\u{201d}' | '\''))?;
    let rest = &text[open.len_utf8()..];
    let close = rest.find(|c: char| matches!(c, '"' | '\u{201c}' | '\u{201d}') || (open == '\'' && c == '\''))?;
    let name = rest[..close].trim().to_string();
    let after = &rest[close..];
    let after = &after[after.chars().next().map_or(0, char::len_utf8)..];
    (!name.is_empty()).then_some((name, after))
}

/// "@ 500 300" or "near 500 300" after a name: the point that tells same-named controls apart.
fn near(text: &str) -> Option<(u16, u16)> {
    let text = text.trim();
    let text = text.strip_prefix('@').or_else(|| text.strip_prefix("near")).unwrap_or(text);
    point(text.trim())
}

/// A click target written as a name: `"Play Believer"`, `"More options" @ 512 330` or `Play Believer` without quotes.
fn name_target(params: &str) -> Option<Target> {
    if let Some((name, rest)) = quoted(params) {
        return Some(Target::Name(name, near(rest)));
    }
    if point(params).is_some() || params.trim().is_empty() {
        return None;
    }
    let (name, at) = match params.rsplit_once(" @ ").or_else(|| params.rsplit_once(" near ")) {
        Some((name, rest)) if point(rest).is_some() => (name, point(rest)),
        _ => (params, None),
    };
    let name = name.trim().trim_matches(|c| c == '"' || c == '\'');
    (!name.is_empty() && !number(name)).then(|| Target::Name(name.to_string(), at))
}

/// A name as compared: lower case, one space, no quotes or "…" (the list shortens long names).
fn fold(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase().trim_matches(|c: char| c == '"' || c == '\'' || c == '\u{2026}' || c == '.' || c.is_whitespace()).to_string()
}

/// The control or text line a name means: the same name first, then one that starts with it, contains it, or is
/// contained in it; among equals the one nearest `near`, then enabled controls before text, in reading order.
pub fn find_named<'a>(elements: &'a [Element], name: &str, near: Option<(u16, u16)>) -> Option<&'a Element> {
    let want = fold(name);
    if want.is_empty() {
        return None;
    }
    let rank = |e: &Element| -> Option<u8> {
        let have = fold(&e.name);
        if have.is_empty() {
            None
        } else if have == want {
            Some(0)
        } else if have.starts_with(&want) {
            Some(1)
        } else if have.contains(&want) {
            Some(2)
        } else if want.chars().count() >= 4 && have.chars().count() >= 3 && want.contains(&have) {
            Some(3)
        } else {
            None
        }
    };
    let distance = |e: &Element| {
        near.map_or(0, |(x, y)| {
            let (cx, cy) = e.center();
            (i32::from(cx) - i32::from(x)).pow(2) + (i32::from(cy) - i32::from(y)).pow(2)
        })
    };
    elements.iter().filter_map(|e| rank(e).map(|r| (r, e))).min_by_key(|(r, e)| (*r, distance(e), !e.enabled, e.kind == "text", e.y / 20, e.x)).map(|(_, e)| e)
}

/// Checks that every name means a control on the look the decisions were made on.
pub fn resolve(decision: Decision, look: &Look) -> Result<Decision, String> {
    match &decision.action {
        Action::Click { target: Target::Name(name, at), .. } | Action::Scroll { target: Target::Name(name, at), .. } | Action::TypeInto { target: Target::Name(name, at), .. }
            if find_named(&look.elements, name, *at).is_none() =>
        {
            Err(format!("Nothing named \"{}\" is on the screen now: use a name from the ON THE SCREEN list, or click by X Y.", short(name, 60)))
        }
        _ => Ok(decision),
    }
}

/// Where a target is clicked: a named control's exact middle; a point inside a small control means that control
/// (its middle is a surer hit than an estimated point); large areas (lists, pages) keep the exact point.
fn spot(target: &Target, elements: &[Element]) -> Result<(f32, f32), String> {
    match target {
        Target::Name(name, near) => find_named(elements, name, *near).map(Element::middle).ok_or(format!("Nothing named \"{name}\" is on the screen.")),
        Target::Point(x, y) => Ok(snap(elements, *x, *y)),
    }
}

fn snap(elements: &[Element], x: u16, y: u16) -> (f32, f32) {
    const SMALL: u32 = 20_000; // 2% of the screen (0..1000 squared)
    elements
        .iter()
        .filter(|e| x >= e.x && x <= e.x + e.w && y >= e.y && y <= e.y + e.h && u32::from(e.w) * u32::from(e.h) <= SMALL)
        .min_by_key(|e| u32::from(e.w) * u32::from(e.h))
        .map(Element::middle)
        .unwrap_or((f32::from(x), f32::from(y)))
}

pub fn parse_line(line: &str) -> Option<Decision> {
    let (head, rest) = match line.split_once(':') {
        Some((h, r)) => (h.trim().to_lowercase(), r.trim().to_string()),
        None => (line.trim().to_lowercase(), String::new()),
    };
    let mut params = rest;
    let mut wait_ms = 0;
    // Optional "(wait N)" at the end.
    if let Some(start) = params.to_lowercase().rfind("(wait ")
        && params.ends_with(')')
        && let Ok(secs) = params[start + 6..params.len() - 1].trim().parse::<f64>()
    {
        wait_ms = (secs.clamp(0.2, 30.0) * 1000.0) as u64;
        params = params[..start].trim().to_string();
    }
    let decision = |action: Action, wait_ms: u64, raw: String| Some(Decision { action, wait_ms, raw });
    match head.as_str() {
        "click" | "double_click" | "right_click" => {
            let (button, count) = match head.as_str() {
                "right_click" => (MouseButton::Right, 1),
                "double_click" => (MouseButton::Left, 2),
                _ => (MouseButton::Left, 1),
            };
            if let Some((x, y)) = point(&params) {
                return decision(Action::Click { target: Target::Point(x, y), button, count }, wait_ms, format!("{head} : {x} {y}"));
            }
            let target = name_target(&params)?;
            let raw = match &target {
                Target::Name(name, Some((x, y))) => format!("{head} : \"{name}\" @ {x} {y}"),
                Target::Name(name, None) => format!("{head} : \"{name}\""),
                Target::Point(..) => return None,
            };
            decision(Action::Click { target, button, count }, wait_ms, raw)
        }
        // scroll : "name" : down [: times]   and   scroll : down [: times] [X Y]
        "scroll" if quoted(&params).is_some() => {
            let (name, rest) = quoted(&params)?;
            let mut words = rest.split(|c: char| c == ':' || c == ',' || c.is_whitespace()).filter(|w| !w.is_empty());
            let up = match words.next().unwrap_or("down").to_lowercase().as_str() {
                "up" => true,
                "down" => false,
                _ => return None,
            };
            let times = words.next().and_then(|w| w.parse::<f64>().ok()).unwrap_or(1.0).round().clamp(1.0, f64::from(MAX_SCROLL)) as u8;
            let steps = if times > 1 { format!(" : {times}") } else { String::new() };
            let raw = format!("scroll : \"{name}\" : {}{steps}", if up { "up" } else { "down" });
            decision(Action::Scroll { target: Target::Name(name, None), up, times }, wait_ms, raw)
        }
        "scroll" => {
            let mut words = params.split(|c: char| c == ':' || c == ',' || c.is_whitespace()).filter(|w| !w.is_empty());
            let up = match words.next().unwrap_or("down").to_lowercase().as_str() {
                "up" => true,
                "down" => false,
                _ => return None,
            };
            let numbers: Vec<f64> = words.map(|w| w.trim_start_matches(['x', '×']).parse()).collect::<Result<_, _>>().ok()?;
            let (times, at) = match numbers.as_slice() {
                [] => (1.0, None),
                [t] => (*t, None),
                [x, y] => (1.0, Some((*x, *y))),
                [t, x, y] => (*t, Some((*x, *y))),
                _ => return None,
            };
            let times = times.round().clamp(1.0, f64::from(MAX_SCROLL)) as u8;
            let target = match at {
                Some((x, y)) => Target::Point(clamp(x), clamp(y)),
                None => Target::Point(500, 500),
            };
            let dir = if up { "up" } else { "down" };
            let steps = if times > 1 { format!(" : {times}") } else { String::new() };
            let raw = format!("scroll : {dir}{steps}");
            decision(Action::Scroll { target, up, times }, wait_ms, raw)
        }
        "type_into" | "type_into_enter" => {
            let enter = head == "type_into_enter";
            let (target, label, text) = match quoted(&params) {
                Some((name, rest)) => {
                    let text = rest.trim_start().strip_prefix(':')?;
                    (Target::Name(name.clone(), None), format!("\"{name}\""), text)
                }
                None => {
                    let (field, text) = params.split_once(':')?;
                    let name = field.trim().to_string();
                    if name.is_empty() || number(&name) {
                        return None;
                    }
                    (Target::Name(name.clone(), None), format!("\"{name}\""), text)
                }
            };
            let text = typed(text.trim());
            if text.is_empty() {
                return None;
            }
            decision(Action::TypeInto { target, text: text.clone(), enter }, wait_ms, format!("{head} : {label} : {}", short(&text, 60)))
        }
        "type" | "type_enter" => {
            if params.is_empty() {
                return None;
            }
            let enter = head == "type_enter";
            let text = typed(&params);
            decision(Action::Type { text, enter }, wait_ms, format!("{head} : {}", short(&params, 60)))
        }
        "hotkey" if !params.is_empty() => decision(Action::Hotkey(params.to_lowercase()), wait_ms, format!("hotkey : {}", params.to_lowercase())),
        "launch_app" if !params.is_empty() => decision(Action::LaunchApp(params.clone()), wait_ms, format!("launch_app : {params}")),
        "open_url" if ["http://", "https://", "spotify:", "ms-settings:"].iter().any(|scheme| params.starts_with(scheme)) => {
            decision(Action::OpenUrl(params.clone()), wait_ms, format!("open_url : {params}"))
        }
        "wait" => {
            let secs: f64 = params.trim().parse().ok()?;
            decision(Action::Wait, (secs.clamp(0.2, 30.0) * 1000.0) as u64, format!("wait : {secs}"))
        }
        "note" => {
            let text: String = params.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NOTE_CHARS).collect();
            if text.is_empty() {
                return None;
            }
            decision(Action::Note(text.clone()), 0, format!("note : {text}"))
        }
        "ask" if !params.is_empty() => decision(Action::Ask(params.clone()), 0, format!("ask : {params}")),
        "done" => decision(Action::Done(params.clone()), 0, format!("done : {params}")),
        "fail" => decision(Action::Fail(params.clone()), 0, format!("fail : {params}")),
        _ => None,
    }
}

// ---------------------------------------------------------------- the loop

#[derive(Default, Clone)]
struct Step {
    raw: String,
    effect: String,
    tab: bool,
    /// The control the step used, by kind and name ("button \"Sort\""), also for a click by X Y (11-11: three clicks
    /// each on two controls of Spotify's playlist page). Shown after the effect, never inside the action text.
    target: String,
    /// The state of the screen after the step (window title and the controls' names; not the pixels, which a clock or
    /// the pointer changes): the same click that leaves the same state again is going in circles. 0 = not looked at.
    after: u64,
}

/// A goal to play music: "play" as a word ("playlist", "display" do not count).
fn wants_music(goal: &str) -> bool {
    let goal = goal.to_lowercase();
    let words: Vec<&str> = goal.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    words.iter().any(|w| matches!(*w, "play" | "plays" | "playing" | "پخش")) && !words.iter().any(|w| matches!(*w, "pause" | "stop" | "متوقف"))
}

/// A player's "Artist - Song" title that names the song or artist the goal asks for ("play Believer by Imagine
/// Dragons" and "Imagine Dragons - Believer"). A goal for any music ("play something") never matches, so the model
/// still decides those.
fn song_of_goal(goal: &str, title: &str) -> bool {
    let Some((_, song)) = title.split_once(" - ") else { return false };
    // The song's own name: "Enemy - From the series Arcane" -> "Enemy", "Believer (Live)" -> "Believer".
    let song = song.split(" - ").next().unwrap_or(song).split(['(', '[']).next().unwrap_or(song);
    let words = |text: &str| -> Vec<String> {
        text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_string).collect()
    };
    const COMMON: &[&str] = &["the", "a", "an", "of", "to", "in", "on", "my", "me", "is", "it", "for", "you", "your", "and", "feat", "ft"];
    let goal = words(goal);
    let named: Vec<String> = words(song).into_iter().filter(|w| !COMMON.contains(&w.as_str())).collect();
    // Every word of the song's name is in the goal ("Waiting For Love" does not pass for "play Believer"); a goal that
    // names only the artist is left to the model.
    !named.is_empty() && named.iter().all(|w| goal.contains(w))
}

/// Spotify's window while nothing plays (while a song plays its title is "Artist - Song"). A bare "Spotify" is left
/// out: Spotify Free shows it during an ad and while it starts.
fn spotify_paused(title: &str) -> bool {
    matches!(title.trim(), "Spotify Free" | "Spotify Premium")
}

/// Actions a screen job may never take, whatever the goal says: they lose the user's unsaved work. The final test of
/// 11-06 closed about ten of the user's old unsaved Notepad tabs with "Don't save" after "zakhire nakon" was read as
/// "close without saving".
fn forbidden(action: &Action, look: &Look) -> Option<String> {
    // alt+space opens the window menu, whose "Close" closes the window (11-10: after alt+f4 was refused the model
    // closed Spotify with alt+space, c).
    const CLOSING_KEYS: &[&str] = &["ctrl+w", "ctrl+f4", "alt+f4", "ctrl+shift+w", "alt+space"];
    const CLOSING_NAMES: &[&str] = &["close", "بستن", "بستن زبانه", "بستن پنجره"];
    const DISCARD: &[&str] = &["don't save", "don\u{2019}t save", "do not save", "dont save", "discard", "ذخیره نکن", "ذخیره نشود", "ذخیره نکردن"];
    // Ending programs or the session, from Task Manager or the power menu.
    const ENDING_NAMES: &[&str] = &["end task", "end process", "پایان کار"];
    // Power words are refused only where they end the session: Start, Search, Settings, Task Manager (an app's own
    // "Restart" or a site's "Sign out" is fine).
    const POWER_NAMES: &[&str] = &["shut down", "shutdown", "restart", "sign out", "sleep", "خاموش کردن", "راه‌اندازی مجدد", "خروج از حساب"];
    const SYSTEM_REFUSAL: &str = "System tools (Task Manager, a command prompt, PowerShell, Run, the registry) are not allowed in a screen job: \
                                  they can end programs or change the system. If the goal cannot be done otherwise, reply fail.";
    let refused = match action {
        Action::LaunchApp(name) => system_tool(name, true),
        Action::Type { text, .. } | Action::TypeInto { text, .. } => system_tool(text, false),
        // Task Manager, Run and the power-user menu straight from the keyboard.
        Action::Hotkey(k) => matches!(k.as_str(), "ctrl+shift+esc" | "win+r" | "win+x"),
        _ => false,
    };
    if refused {
        return Some(SYSTEM_REFUSAL.into());
    }
    if matches!(action, Action::Type { .. } | Action::TypeInto { .. } | Action::Hotkey(_)) && system_window(&look.title) {
        return Some(SYSTEM_REFUSAL.into());
    }
    match action {
        Action::Hotkey(k) if CLOSING_KEYS.contains(&k.as_str()) => Some(
            "Closing windows or tabs is not allowed in a screen job: unsaved work could be lost. Leave them open and go on. \
             (An app the user asked to close is closed with the close_app tool, not by a screen job.)"
                .into(),
        ),
        Action::Click { target, .. } => {
            let element = clicked(target, look)?;
            let name = element.name.to_lowercase();
            let starts = |words: &[&str]| words.iter().any(|d| name.trim() == *d || name.starts_with(&format!("{d} ")));
            let system_place = matches!(look.title.trim().to_lowercase().as_str(), "start" | "search" | "settings" | "task manager");
            if starts(ENDING_NAMES) || (system_place && starts(POWER_NAMES)) {
                return Some(SYSTEM_REFUSAL.into());
            }
            if DISCARD.iter().any(|d| name.contains(d)) {
                return Some(
                    "Never answer a save dialog with \"Don't save\" or \"Discard\": that throws away the user's work. Press Cancel (escape) instead and leave the file as it is.".into(),
                );
            }
            // A tab's or window's X closes it like ctrl+w (re-test 06:03: a click on a tab's X closed a Notepad tab).
            let name = name.trim();
            (CLOSING_NAMES.iter().any(|c| name == *c || name.starts_with(&format!("{c} "))) || name.contains("close tab") || name.contains("close window"))
                .then(|| "Closing windows or tabs is not allowed in a screen job: unsaved work could be lost. Use escape to dismiss a popup.".into())
        }
        _ => None,
    }
}

/// A system tool or a command, typed into Start/Run or launched: Task Manager, a terminal, the registry, or a command
/// that ends programs or the session.
fn system_tool(text: &str, launching: bool) -> bool {
    const TOOLS: &[&str] = &[
        "task manager", "taskmgr", "cmd", "cmd.exe", "command prompt", "powershell", "windows powershell", "pwsh", "windows terminal", "regedit",
        "registry editor", "msconfig", "services.msc", "مدیر وظایف",
    ];
    // Ordinary words too ("Run" is a song): refused only as an app to launch; their windows are refused by title.
    const AMBIGUOUS: &[&str] = &["run", "terminal", "wt"];
    // Anything typed into a terminal or Run is refused by its window; these are refused anywhere (Start search too).
    const COMMANDS: &[&str] = &["taskkill", "stop-process", "shutdown /", "shutdown -", "shutdown.exe"];
    let text = text.trim().to_lowercase();
    TOOLS.contains(&text.as_str()) || (launching && AMBIGUOUS.contains(&text.as_str())) || COMMANDS.iter().any(|c| text.contains(c))
}

/// A system tool's window in front.
fn system_window(title: &str) -> bool {
    let title = title.trim().to_lowercase();
    // Exact titles: "Run Boy Run - YouTube" or "Task Manager notes.docx - Word" are ordinary windows.
    ["task manager", "command prompt", "windows powershell", "registry editor", "run", "terminal"].contains(&title.as_str())
        || title.starts_with("administrator:")
        || title.ends_with("- powershell")
        || title.ends_with("\\cmd.exe")
}

/// The control a step used (kind and name), for a named control or a point on one; empty for anything else.
fn target_of(decision: &Decision, look: &Look) -> String {
    let element = match &decision.action {
        Action::Click { target: Target::Name(name, at), .. } | Action::Scroll { target: Target::Name(name, at), .. } | Action::TypeInto { target: Target::Name(name, at), .. } => {
            find_named(&look.elements, name, *at)
        }
        Action::Click { target: Target::Point(x, y), .. } => look
            .elements
            .iter()
            .filter(|e| !e.name.is_empty() && e.x <= *x && *x <= e.x + e.w && e.y <= *y && *y <= e.y + e.h && e.w < 900)
            .min_by_key(|e| u32::from(e.w) * u32::from(e.h)),
        _ => None,
    };
    element.filter(|e| !e.name.trim().is_empty()).map(|e| format!("{} \"{}\"", e.kind, short(&e.name, 40))).unwrap_or_default()
}

/// The screen's state for telling repeats apart: the window title and the names of its controls (not text lines and
/// not pixels, which a clock, an animation or the pointer change).
fn state_of(look: &Look) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    look.title.hash(&mut hasher);
    for e in look.elements.iter().filter(|e| e.kind != "text") {
        e.name.hash(&mut hasher);
    }
    hasher.finish().max(1)
}

/// The last actions were meant to start a song ("expect : the newest song is playing"): whole words only
/// ("displays", "playlist" and "لیست پخش" do not count).
fn expects_playing(expected: &str) -> bool {
    let lower = expected.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    words.iter().any(|w| matches!(*w, "playing" | "plays" | "played"))
        || lower.contains("starts to play")
        || lower.contains("start to play")
        || ["در حال پخش", "پخش می", "پخش شود", "پخش شد"].iter().any(|p| lower.contains(p))
}

/// Going in circles: the same one or two actions again and again, or flipping between tabs.
fn detect_loop(history: &[Step]) -> Option<String> {
    let actions: Vec<&Step> = history.iter().filter(|h| !h.raw.starts_with("note :")).collect();
    // The same control clicked three times in the last five actions, each time leaving the screen in the same state
    // (11-11: the Sort button of a playlist clicked again and again). Pressing Next through an installer changes the
    // screen each time and is not a repeat; typing has its own rule below.
    let clicks: Vec<(String, u64)> = actions
        .iter()
        .rev()
        .take(5)
        .filter(|h| !h.target.is_empty() && h.after != 0 && h.raw.contains("click"))
        .map(|h| (format!("{} on {}", h.raw.split(" : ").next().unwrap_or(&h.raw), h.target), h.after))
        .collect();
    if let Some((same, _)) = clicks.iter().find(|c| clicks.iter().filter(|x| *x == *c).count() >= 3) {
        return Some(format!("You did {same} three times and the screen came back the same each time; that way is not working."));
    }
    // The same app or page asked for again and again although it is open (11-11: "open_url : spotify:search:music"
    // eight times in 55 s while Spotify's controls were not readable yet).
    let skipped: Vec<&str> = actions.iter().rev().take_while(|h| h.effect.starts_with("skipped: already opened")).map(|h| h.raw.as_str()).collect();
    if skipped.len() >= 2 {
        return Some(format!("You keep opening \"{}\" again although it is open already.", skipped[0]));
    }
    let recent: Vec<&str> = actions.iter().rev().take(6).map(|h| h.raw.as_str()).collect();
    let mut distinct = recent.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if recent.len() >= 6 && distinct.len() <= 2 {
        return Some(format!("You keep repeating {} without progress.", distinct.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" and ")));
    }
    // A wider circle: clear, type, Enter, clear, type... (training round 4: the Calculator showed 5,535 and the agent
    // kept clearing and retyping it for four minutes, 35 model calls, varying the steps a little each time).
    let wider: Vec<&str> = actions.iter().rev().take(10).map(|h| h.raw.as_str()).collect();
    let mut kinds = wider.clone();
    kinds.sort_unstable();
    kinds.dedup();
    if wider.len() >= 10 && kinds.len() <= 4 {
        return Some(format!("You keep repeating the same few steps ({}) without progress.", kinds.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(", ")));
    }
    // The same text typed again and again around different clicks and keys (11-10: a Save as path typed nine times
    // in three minutes; no two steps in a row were the same, so the rules above never fired).
    let texts: Vec<&str> = actions.iter().rev().filter_map(|h| typed_text(&h.raw)).filter(|t| t.chars().count() >= 3).collect();
    let just_typed: Vec<&str> = actions.iter().rev().take(3).filter_map(|h| typed_text(&h.raw)).collect();
    if let Some(text) = just_typed.iter().find(|t| texts.iter().filter(|x| x == t).count() >= 4) {
        return Some(format!("You typed \"{}\" four times or more and it did not work; that way is not working.", short(text, 60)));
    }
    let tab_moves = actions.iter().rev().take(5).filter(|h| h.tab || h.raw.starts_with("hotkey : ctrl+tab") || h.raw.starts_with("hotkey : ctrl+shift+tab")).count();
    (tab_moves >= 4).then(|| "You keep switching between tabs without reading them.".to_string())
}

/// What one look at the screen is asked with, beyond the screenshot.
struct Ask<'a> {
    goal: &'a str,
    /// The user's own words; the goal is the assistant's rewrite of them and can lose a limit on the way.
    said: &'a str,
    /// The planner's numbered steps for this job; empty when it gave none.
    plan: &'a str,
    history: &'a [Step],
    notes: &'a [String],
    actions: usize,
    /// What the model said its last actions would change on screen ("expect : ...").
    expected: &'a str,
    warning: &'a str,
}

/// The text of one step. The fixed rules come first and what changes last: the provider caches a prompt's unchanged
/// beginning and bills it at a fraction, so every step after the first pays little for the rules.
fn prompt(ask: &Ask, look: &Look) -> String {
    let history = ask.history;
    let recent = if history.is_empty() {
        "none yet".to_string()
    } else {
        history
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let effect = if h.effect.is_empty() { String::new() } else { format!(" -> {}", h.effect) };
                let target = if h.target.is_empty() { String::new() } else { format!(" [{}]", h.target) };
                format!("{}. {}{effect}{target}", i + 1, h.raw)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let notes = if ask.notes.is_empty() { "none yet".to_string() } else { ask.notes.iter().enumerate().map(|(i, n)| format!("{}. {n}", i + 1)).collect::<Vec<_>>().join("\n") };
    // Controls and text lines together in reading order, so a heading stands above the rows it belongs to. A
    // window's own buttons (minimize, maximize, close) are left out: a job never uses them (closing is refused).
    let mut shown: Vec<&Element> = look.elements.iter().filter(|e| !window_button(e)).collect();
    shown.sort_by_key(|e| (e.y / 20, e.x));
    // Each by its name, which the model clicks; where it is only when a name repeats or there is none.
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for e in &shown {
        *seen.entry(fold(&e.name)).or_default() += 1;
    }
    let controls = shown
        .iter()
        .map(|e| {
            let (x, y) = e.center();
            let place = if e.name.trim().is_empty() || seen.get(&fold(&e.name)).copied().unwrap_or(0) > 1 { format!(" @ {x} {y}") } else { String::new() };
            let name = if e.name.trim().is_empty() { "(no name)".to_string() } else { format!("\"{}\"", short(&e.name, 50)) };
            format!("{} {name}{place}{}", e.kind, if e.enabled { "" } else { " (disabled)" })
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut lines = vec![
        "You are a careful, fast expert operating a Windows desktop through screenshots.".to_string(),
        DECISION_RULES.to_string(),
        String::new(),
        format!("User goal: {}", serde_json::to_string(ask.goal).unwrap_or_default()),
        if ask.said.is_empty() || ask.said == ask.goal.trim() {
            String::new()
        } else {
            format!(
                "The user's own words (the goal was written from them; follow every limit in them, such as \"do not send\"): {}",
                serde_json::to_string(ask.said).unwrap_or_default()
            )
        },
        if ask.plan.is_empty() {
            String::new()
        } else {
            format!("THE PLAN (written for this job by the planner; do its next step that is not done yet):\n{}", ask.plan)
        },
        format!("Recent actions (oldest first, with their effect on the screen; {} actions so far):\n{recent}", ask.actions),
        format!("YOUR NOTES (facts already found; never look these up again):\n{notes}"),
    ];
    if !ask.expected.is_empty() {
        lines.push(format!(
            "YOU EXPECTED after your last actions: {}\nCheck it on this screenshot FIRST. If it did not happen, those actions did not work: fix that \
             before going on, and never act as if it worked.",
            ask.expected
        ));
    }
    if !ask.warning.is_empty() {
        lines.push(ask.warning.to_string());
    }
    if !look.title.is_empty() {
        lines.push(format!("Active window: {}", look.title));
    }
    if !controls.is_empty() {
        lines.push(format!(
            "ON THE SCREEN, top to bottom: the controls (kind and name) and \"text\" lines (the words shown there: headings, labels, \
             values). Click any of them by its name; \"@ X Y\" is where one is when its name repeats or it has none:\n{controls}"
        ));
    }
    lines.retain(|l| !l.is_empty());
    lines.join("\n")
}

/// A window's own title-bar button: minimize, maximize, restore or close.
fn window_button(e: &Element) -> bool {
    const NAMES: &[&str] = &["minimize", "maximize", "restore", "restore down", "close", "کوچک کردن", "بزرگ کردن", "بازگردانی", "بستن"];
    e.kind == "button" && NAMES.contains(&e.name.trim().to_lowercase().as_str())
}

/// The "expect : ..." line of a reply: what its actions should change on screen, checked on the next look.
fn expectation(answer: &str) -> String {
    answer
        .lines()
        .map(|l| l.trim_matches(|c: char| c == '`' || c == '*' || c == '-' || c == '•' || c.is_whitespace()))
        .find_map(|l| {
            let (head, rest) = l.split_once(':')?;
            head.trim().eq_ignore_ascii_case("expect").then(|| short(rest.trim(), 160))
        })
        .unwrap_or_default()
}

const DECISION_RULES: &str = "\
Study the current screenshot and choose the next action. Follow THE PLAN: do its next step that is not done yet,
adapting names and values to what the screen shows; when the screen shows a step cannot work, do what reaches the
goal. (The assistant's own island is left out of your screenshots and lets your clicks through.) The goal, the plan,
your recent actions, notes, the active window and the controls on the screen come after these rules.

OUTPUT FORMAT (one action per line, no explanation, no markdown):
- click : \"name\"              (the control or text with this name, copied from the ON THE SCREEN list; also
                                double_click : \"name\", right_click : \"name\"; when several have the name, add the
                                place of the one you mean: click : \"More options\" @ 512 330)
- type_into : \"name\" : text    (click that field, replace its text, type; type_into_enter also presses Enter)
- scroll : \"name\" : down : 3   (scroll that list or page 3 steps of a third of a page; 1 to 10; or up)
- click : X Y                  (only for what has no name in the list; also double_click : X Y, right_click : X Y)
- type : text                  (where the focus is; \\n for a new line); type_enter : text also presses Enter
- hotkey : ctrl+l              (enter, tab, escape, ctrl+a, alt+left, f5, media_play_pause ...)
- scroll : down : 3            (or up; optionally followed by X Y of the area)
- launch_app : appName | open_url : https://... (or a spotify: or ms-settings: link) | wait : seconds
- note : text                  (save a fact you found, e.g. a price with its source)
- ask : question              (before anything that cannot be undone: send, post, buy, pay, delete)
- done : what is now on screen in a few words | fail : reason
- expect : what your actions will change on screen (a last line after screen actions); the next look checks it
The screen is watched after each action until it settles, so you rarely need wait.
Up to 3 lines at once when the next steps need no new look: only the first may be a click, scroll, type_into,
launch_app or open_url; the next ones only type, type_enter, hotkey or wait. done, fail, note and ask stand alone.
X and Y are 0 to 1000 of the screenshot, read off the yellow rulers along the top (X) and left (Y) edges.

RULES:
- Click by the exact name in the list: the click lands on the very middle of that control. X Y only for what the
  list does not have.
- Read the words on the screen: rows under a heading such as \"Recommended\", \"Suggested\", \"Sponsored\" or \"Ad\" are
  NOT part of the list above it.
- Never repeat an action whose effect says NO visible change or FAILED; do something different.
- Type only where a field is visible and focused. In Notepad and other editors press ctrl+n first: never type into
  an existing file. After typing, check the text appeared.
- NEVER close a window or tab, and NEVER choose \"Don't save\" or \"Discard\" (press escape instead).
- ask before sending, posting, buying, paying or deleting. If the goal or the user's own words say prepare, draft
  or do not send (nafrest, ersal nakon), never press Send: reply done when the text is in place.
- Never click play/pause twice (the second click pauses); a music goal is done as soon as the right song plays.
- Trust the \"Active window\" line. If the app the goal needs is not in front, launch_app it.
- done only when the current screenshot shows the goal reached; describe only what you see. fail only when the goal
  is impossible (missing app, no access, repeated dead ends).";

pub struct DesktopTask {
    pub ops: Arc<dyn DesktopOps>,
    /// Train mode: steps that worked before for a similar goal, and lessons.
    pub learner: Option<Arc<crate::learn::Learner>>,
    /// App tips: the user's skills folder and the built-in ones; None: built-in ones only.
    pub skills: Option<Arc<crate::skills::Skills>>,
    /// A second opinion (Settings > Models > Second check): is the goal really reached, and is a click hard to undo.
    pub judge: Option<Arc<dyn Judge>>,
}

/// The judge objects to "done" only when it is this sure the goal is not reached: the screenshot does not show
/// everything (a copied text, a saved file), so a doubtful answer is not an objection. 11-11: a false "shows 4140"
/// over a Calculator showing 0 got 21% on the whole (scaled-down) screen, and true claims 88-97%; an objection costs
/// one step, and comes at most once.
const JUDGE_NOT_DONE: f64 = 0.3;
/// A click is asked about when the judge is this sure it is hard to undo.
const JUDGE_RISKY: f64 = 0.85;
/// How long a freshly launched app is given after its window appears before keys are typed into it.
const LAUNCH_INPUT_DELAY_MS: u64 = 900;
const JUDGE_DONE_QUESTION: &str = "Is the goal reached, as far as this screenshot shows?";
const JUDGE_RISKY_QUESTION: &str = "Could this click delete, send, buy, post, close without saving, or shut something down (something hard to undo)?";

/// How long the planner may take before the job goes on without a plan.
const PLAN_TIMEOUT: Duration = Duration::from_secs(15);
/// New plans in one job when the executor is stuck.
const MAX_REPLANS: usize = 2;

/// The control a click lands on.
fn clicked<'a>(target: &Target, look: &'a Look) -> Option<&'a Element> {
    match target {
        Target::Point(x, y) => look.elements.iter().find(|e| e.x <= *x && *x <= e.x + e.w && e.y <= *y && *y <= e.y + e.h),
        Target::Name(name, at) => find_named(&look.elements, name, *at),
    }
}

#[derive(Debug)]
enum Stop {
    Cancelled,
    Failed(String),
}

impl DesktopTask {
    async fn blocking<T: Send + 'static>(&self, f: impl FnOnce(&dyn DesktopOps) -> Result<T, String> + Send + 'static) -> Result<T, String> {
        let ops = self.ops.clone();
        tokio::task::spawn_blocking(move || f(ops.as_ref())).await.map_err(|e| format!("desktop worker stopped: {e}"))?
    }

    /// The name of the control a click lands on, when the judge is sure the click is hard to undo: the user is asked
    /// first. Each name is judged once in a job; right after the user allowed something, one click passes unasked.
    async fn risky_click(&self, action: &Action, look: &Look, goal: &str, judged: &mut Vec<(String, bool)>, approved: &mut bool, ctx: &ToolCtx) -> Option<String> {
        let judge = self.judge.as_ref()?;
        let Action::Click { target, .. } = action else { return None };
        let element = clicked(target, look)?;
        let name = element.name.trim();
        if name.is_empty() || std::mem::take(approved) {
            return None;
        }
        let key = name.to_lowercase();
        let risky = match judged.iter().find(|(n, _)| *n == key) {
            Some((_, risky)) => *risky,
            None => {
                let state = format!(
                    "A desktop agent is working on this goal for the user: {goal}\nThe window in front is \"{}\".\nIt is about to click the {} named \"{name}\".",
                    look.title, element.kind
                );
                let probability = judge.yes_no(&state, JUDGE_RISKY_QUESTION, None).await?;
                (ctx.log)(format!("second check of the click on \"{}\": {:.0}% hard to undo", short(name, 40), probability * 100.0));
                let risky = probability >= JUDGE_RISKY;
                judged.push((key, risky));
                risky
            }
        };
        risky.then(|| name.to_string())
    }

    async fn pause(ms: u64, ctx: &ToolCtx) -> Result<(), Stop> {
        tokio::select! {
            _ = ctx.cancel.cancelled() => Err(Stop::Cancelled),
            _ = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
        }
    }

    async fn look(&self, ctx: &ToolCtx) -> Result<Look, Stop> {
        let mut last = String::new();
        for attempt in 0..3u64 {
            match self.blocking(|ops| ops.look()).await {
                Ok(look) if !look.jpeg.is_empty() => return Ok(look),
                Ok(_) => last = "empty screenshot".into(),
                Err(e) => last = e,
            }
            Self::pause(250 * (attempt + 1), ctx).await?;
        }
        Err(Stop::Failed(format!("Screen capture kept failing ({last}).")))
    }

    async fn fingerprint(&self) -> u64 {
        self.blocking(|ops| ops.fingerprint()).await.unwrap_or(0)
    }

    async fn title(&self) -> String {
        self.blocking(|ops| Ok(ops.active_title())).await.unwrap_or_default()
    }

    async fn own_in_front(&self) -> bool {
        self.blocking(|ops| Ok(ops.own_window_in_front())).await.unwrap_or(false)
    }

    /// Waits until the screen has settled after an action: at least `min`; when a new window is expected, until it
    /// comes to the front; then until two looks in a row are equal (and, if a change is expected, one was seen).
    /// Never longer than `max`. Returns how long it waited.
    async fn settle(&self, settle: Settle, before: (u64, &str), ctx: &ToolCtx) -> Result<Duration, Stop> {
        let started = Instant::now();
        let max = Duration::from_millis(settle.max_ms);
        Self::pause(settle.min_ms, ctx).await?;
        if settle.min_ms >= settle.max_ms {
            return Ok(started.elapsed());
        }
        if settle.new_window {
            while started.elapsed() < max && self.title().await == before.1 {
                Self::pause(POLL_MS, ctx).await?;
            }
        }
        let mut last = self.fingerprint().await;
        let mut changed = last != before.0;
        while started.elapsed() < max {
            Self::pause(POLL_MS, ctx).await?;
            let now = self.fingerprint().await;
            changed |= now != before.0;
            if now == last && (changed || !settle.expect_change) {
                break;
            }
            last = now;
        }
        Ok(started.elapsed())
    }

    async fn act(&self, decision: &Decision, look: &Look) -> Result<(), String> {
        if let Some(why) = forbidden(&decision.action, look) {
            return Err(why);
        }
        // Keys meant for an app must not land in the assistant's own window (final test: Notepad did not come to the
        // front, and ctrl+n and the text went into "AnyTask").
        let keys = matches!(&decision.action, Action::Type { .. } | Action::TypeInto { .. })
            || matches!(&decision.action, Action::Hotkey(k) if !k.starts_with("media_") && !k.starts_with("volume_") && k != "win" && k != "escape");
        if keys && self.own_in_front().await {
            let front = self.title().await;
            return Err(format!(
                "The assistant's own window (\"{front}\") is in front, not the app: bring the app forward first (launch_app it again or click its window)."
            ));
        }
        let at = |target: &Target| spot(target, &look.elements);
        match decision.action.clone() {
            Action::Click { target, button, count } => {
                let (x, y) = at(&target)?;
                self.blocking(move |o| o.click_exact(x, y, button, count)).await
            }
            Action::Scroll { target, up, times } => {
                let (x, y) = at(&target)?;
                let (x, y) = (x.round() as u16, y.round() as u16);
                self.blocking(move |o| {
                    for step in 0..times.max(1) {
                        if step > 0 {
                            // The app moves its list between the notches; one long burst is dropped by some.
                            std::thread::sleep(Duration::from_millis(40));
                        }
                        o.scroll(x, y, up)?;
                    }
                    Ok(())
                })
                .await
            }
            Action::TypeInto { target, text, enter } => {
                let (x, y) = at(&target)?;
                let text: String = text.chars().take(MAX_TYPED).collect();
                self.blocking(move |o| {
                    o.click_exact(x, y, MouseButton::Left, 1)?;
                    std::thread::sleep(Duration::from_millis(120));
                    o.hotkey("ctrl+a")?;
                    o.type_text(&text)?;
                    if enter { o.hotkey("enter") } else { Ok(()) }
                })
                .await
            }
            Action::Type { text, enter } => {
                let text: String = text.chars().take(MAX_TYPED).collect();
                self.blocking(move |o| {
                    o.type_text(&text)?;
                    if enter { o.hotkey("enter") } else { Ok(()) }
                })
                .await
            }
            Action::Hotkey(combo) => self.blocking(move |o| o.hotkey(&combo)).await,
            Action::LaunchApp(name) => self.blocking(move |o| o.launch_app(&name)).await,
            Action::OpenUrl(url) => self.blocking(move |o| o.open_url(&url)).await,
            Action::Wait | Action::Note(_) | Action::Ask(_) | Action::Done(_) | Action::Fail(_) => Ok(()),
        }
    }

    async fn decide(&self, system: String, look: &Look, ctx: &ToolCtx) -> Result<String, String> {
        use base64::Engine;
        let request = GenerateRequest {
            system,
            messages: vec![Message {
                role: Role::User,
                parts: vec![Part::text("The current screenshot:"), Part::image("image/jpeg", base64::engine::general_purpose::STANDARD.encode(&look.jpeg))],
            }],
            tools: Vec::new(),
            allow_tools: false,
            max_output_tokens: 400,
            // A screen step needs a quick look, not deliberation: thinking off is several times faster.
            thinking: Some(Thinking::Low),
        };
        // The answer is a few words, so a long wait is a stuck connection or a busy server, not writing. Asking a
        // second time alongside usually answers first (logs: a step stuck for 25 s was answered 5 s after asking
        // again); the first ask keeps running, and whichever answers first wins.
        let first = ctx.model.generate(&request);
        tokio::pin!(first);
        tokio::select! {
            _ = ctx.cancel.cancelled() => return Err("cancelled".into()),
            answer = &mut first => return answer.map(|a| a.text()).map_err(|e| e.to_string()),
            _ = tokio::time::sleep(HEDGE_AFTER) => {}
        }
        (ctx.log)(format!("model: no answer within {} s, asking a second time alongside", HEDGE_AFTER.as_secs()));
        let second = ctx.model.generate(&request);
        tokio::pin!(second);
        let deadline = tokio::time::sleep(DECISION_TIMEOUT);
        tokio::pin!(deadline);
        let (mut first_failed, mut second_failed) = (false, false);
        loop {
            let failure = tokio::select! {
                _ = ctx.cancel.cancelled() => return Err("cancelled".into()),
                _ = &mut deadline => return Err(format!("no answer within {} s", (HEDGE_AFTER + DECISION_TIMEOUT).as_secs())),
                answer = &mut first, if !first_failed => match answer {
                    Ok(a) => return Ok(a.text()),
                    Err(e) => { first_failed = true; e.to_string() }
                },
                answer = &mut second, if !second_failed => match answer {
                    Ok(a) => return Ok(a.text()),
                    Err(e) => { second_failed = true; e.to_string() }
                },
            };
            if first_failed && second_failed {
                return Err(failure);
            }
        }
    }

    /// Asks the planner for the job's steps (text only, no screenshot). Empty when it fails or gives no steps: the
    /// executor then works from the goal alone, as before.
    async fn plan(&self, planning: &super::planner::Planning<'_>, ctx: &ToolCtx) -> Result<Vec<String>, Stop> {
        let request = GenerateRequest {
            system: super::planner::prompt(planning),
            messages: vec![Message { role: Role::User, parts: vec![Part::text("Write the plan.")] }],
            tools: Vec::new(),
            allow_tools: false,
            max_output_tokens: 500,
            thinking: Some(Thinking::Low),
        };
        let t = Instant::now();
        let answer = tokio::select! {
            _ = ctx.cancel.cancelled() => return Err(Stop::Cancelled),
            answer = tokio::time::timeout(PLAN_TIMEOUT, ctx.model.generate(&request)) => answer,
        };
        let steps = match answer {
            Ok(Ok(reply)) => super::planner::steps(&reply.text()),
            Ok(Err(e)) => {
                (ctx.log)(format!("planner failed: {e}; going on without a plan"));
                return Ok(Vec::new());
            }
            Err(_) => {
                (ctx.log)(format!("planner: no answer within {} s; going on without a plan", PLAN_TIMEOUT.as_secs()));
                return Ok(Vec::new());
            }
        };
        (ctx.log)(format!("plan {:.1}s: {}", t.elapsed().as_secs_f32(), if steps.is_empty() { "(no steps)".to_string() } else { steps.join(" | ") }));
        Ok(steps)
    }

    async fn run_goal(&self, goal: &str, ctx: &ToolCtx) -> Result<Value, Stop> {
        let started = Instant::now();
        let mut history: Vec<Step> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut previous: Option<(u64, bool)> = None; // fingerprint before the last actions, whether they should change it
        // Apps and links already opened in this task: opening them again only adds windows or tabs.
        let mut opened: Vec<String> = Vec::new();
        let (mut no_effect, mut loop_warnings, mut vision_failures, mut action_failures, mut actions, mut looks) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let learner = self.learner.clone().filter(|l| l.enabled());
        // The planner's memory: runs that worked for similar goals and lessons. Only the planner reads it.
        let memory = learner.as_ref().map(|l| l.desktop_hints(goal)).unwrap_or_default();
        let builtin_skills;
        let skills = match &self.skills {
            Some(skills) => skills.as_ref(),
            None => {
                builtin_skills = crate::skills::Skills::new(None);
                &builtin_skills
            }
        };
        // What the last actions should have changed, checked on the next look ("expect : ...").
        let mut expected = String::new();
        if !memory.is_empty() {
            (ctx.log)(format!("planner memory: {}", short(&memory, 160)));
        }
        // 11-08: "Open YouTube in a browser" became launch_app Chrome, which this PC does not have (it uses Vivaldi).
        let mut pc = match self.blocking(|o| Ok(o.default_browser())).await {
            Ok(Some(browser)) => format!(
                "THIS PC: the web browser is {browser}: use it for web pages (launch_app : {browser}), or open a page in it directly with open_url : https://..."
            ),
            _ => String::new(),
        };
        // The real folders, so a Save as box gets a path that exists (11-10: the model typed C:\Users\Admin\Desktop).
        if let Ok(home) = std::env::var("USERPROFILE") {
            let line = format!("The user's folder is {home} (Desktop: {home}\\Desktop, Documents: {home}\\Documents, Downloads: {home}\\Downloads).");
            pc = if pc.is_empty() { format!("THIS PC: {line}") } else { format!("{pc}\n{line}") };
        }
        // Actions that ran, for the playbook of this goal.
        let mut trace: Vec<String> = Vec::new();
        // "fail" answers sent back for another try (not stuck, steps left).
        let mut fail_pushbacks = 0usize;
        let finish = |success: bool, message: String, notes: &[String], looks: usize, actions: usize, trace: &[String], window: &str| {
            let seconds = started.elapsed().as_secs_f32();
            (ctx.log)(format!("{} in {seconds:.1}s ({looks} looks, {actions} actions)", if success { "done" } else { "stopped" }));
            if let Some(learner) = &learner {
                learner.record_desktop(goal, trace, success, window, seconds, chrono::Utc::now().timestamp_millis());
            }
            json!({ "success": success, "message": message, "notes": notes })
        };

        // A job starts where the user is, not in the assistant's own window: typing a request leaves "AnyTask" in front,
        // and a job that starts there cannot reach the app (11-06: "read this tab" never reached the browser).
        let about_assistant = goal.to_lowercase().contains("anytask");
        // The job got to another program's window at least once; until then, done cannot be true.
        let mut reached_app = false;
        let mut own_pushbacks = 0usize;
        let mut play_pushbacks = 0usize;
        // The second check: "done" is sent back at most once; each clicked name is judged once; a click the user just
        // allowed is not asked about again.
        let mut judge_objected = false;
        // The number check objects at most once.
        let mut number_checked = false;
        let mut judged: Vec<(String, bool)> = Vec::new();
        let mut approved = false;
        // The window in front when the job began: a Spotify title already showing the song is not news.
        let title_at_start = self.title().await;
        // What played before the job: only a song that starts during it ends the job early.
        let song_before = self.blocking(|o| Ok(o.now_playing())).await.ok().flatten().filter(|n| n.0).map(|n| n.1).unwrap_or_default();
        // What played at the last look, and whether an action meant to start a song is still waiting for one.
        let mut playing_before = song_before.clone();
        let mut awaiting_song = false;
        if !about_assistant
            && self.own_in_front().await
            && let Ok(Some(window)) = self.blocking(|o| Ok(o.user_window())).await
        {
            let _ = self.blocking(move |o| o.bring_to_front(window)).await;
            Self::pause(300, ctx).await?;
            (ctx.log)(format!("started from the user's window \"{}\", not AnyTask's own", self.title().await));
        }

        // The planner first: the steps from the goal alone, with the app tips and its memory.
        (ctx.progress)(tr("Planning the steps…", "برنامه‌ریزی قدم‌ها…").into());
        let said = ctx.request.trim();
        let window = self.title().await;
        let mut plan_steps = self.plan(&super::planner::Planning { goal, said, pc: &pc, tips: &skills.for_job(goal, &window), memory: &memory, window: &window, happened: "" }, ctx).await?;
        let mut plan = super::planner::numbered(&plan_steps);
        let mut replans = 0usize;
        // Opening the app or page the plan starts in needs no look: it runs on the first look without a model call.
        let mut opening = super::planner::opening(&plan_steps);

        for step in 1..=MAX_STEPS {
            if started.elapsed() > MAX_RUNTIME {
                return Ok(finish(false, "The desktop task reached its time limit before completion.".into(), &notes, looks, actions, &trace, ""));
            }
            (ctx.progress)(if step == 1 { tr("Looking at the screen…", "نگاه به صفحه…").into() } else { tr("Checking the result…", "بررسی نتیجه…").into() });
            let t = Instant::now();
            let look = self.look(ctx).await?;
            looks += 1;
            let own_in_front = self.own_in_front().await;
            reached_app |= !own_in_front;
            // What plays now (asked once per look, only for music goals).
            let playing = if wants_music(goal) { self.blocking(|o| Ok(o.now_playing())).await.ok().flatten().filter(|n| n.0).map(|n| n.1).unwrap_or_default() } else { String::new() };
            // The asked song plays: done without another model round (11-10: the title already read "Imagine Dragons -
            // Believer" and the model took 20 s to say so).
            if actions > 0 && !playing.is_empty() && playing != song_before && song_of_goal(goal, &playing) {
                let song = playing.clone();
                (ctx.log)(format!("the asked song plays (\"{song}\"): done"));
                return Ok(finish(true, format!("Completed. \"{song}\" is playing."), &notes, looks, actions, &trace, &look.title));
            }
            // The same seen from Spotify's own window, which is titled "Artist - Song" only while it plays: Windows'
            // media session may be following another player (11-11: a YouTube tab from an earlier job kept playing,
            // so "Enemy" played in Spotify and the model was asked again, 10 s more).
            if actions > 0
                && wants_music(goal)
                && look.title != title_at_start
                && song_of_goal(goal, &look.title)
                && self.blocking(|o| Ok(o.app_window("Spotify").is_some_and(|w| w == o.foreground()))).await.unwrap_or(false)
            {
                (ctx.log)(format!("Spotify plays the asked song (\"{}\"): done", look.title));
                return Ok(finish(true, format!("Completed. \"{}\" is playing.", look.title), &notes, looks, actions, &trace, &look.title));
            }
            // A song started after actions meant to start one ("expect : the newest song is playing"), for a goal the
            // song's name cannot be checked against ("the last song of my playlist"): the model is told plainly, so it
            // ends the job if it is the right song instead of clicking on (11-11 21:12: JAY-Z started, then five more
            // clicks on the playlist's sort menu). Only the model can tell whether it is the right one.
            let mut song_news = String::new();
            if wants_music(goal) {
                if awaiting_song && !playing.is_empty() && playing != playing_before && playing != song_before {
                    song_news = format!(
                        "A NEW SONG STARTED after your last actions: \"{playing}\". If it is the song the goal asks for, reply done now and \
                         click nothing else. If it is not, start the right one (never press play/pause on this one)."
                    );
                    awaiting_song = false;
                    (ctx.log)(format!("a song started as the last actions expected (\"{playing}\"): asking the model to confirm"));
                }
                playing_before = playing.clone();
            }
            let texts = look.elements.iter().filter(|e| e.kind == "text").count();
            (ctx.log)(format!("look {:.2}s: {} controls, {texts} texts, window \"{}\"", t.elapsed().as_secs_f32(), look.elements.len() - texts, look.title));
            if let (Some((before, expect)), Some(last)) = (previous.take(), history.last_mut()) {
                let changed = look.fingerprint != before;
                last.after = state_of(&look);
                // Effects already decided (skipped, failed, no window) are kept.
                if last.effect.is_empty() {
                    last.effect = if !expect {
                        "ok".into()
                    } else if changed {
                        "screen changed".into()
                    } else if last.raw.starts_with("scroll") {
                        // Said plainly, so the model stops scrolling a list that has ended.
                        "NO visible change: that list is at its end already (or the mouse was not over it)".into()
                    } else {
                        "NO visible change".into()
                    };
                }
                no_effect = if expect && !changed { no_effect + 1 } else { 0 };
            }

            let mut warning = String::new();
            if !song_news.is_empty() {
                warning.push_str(&song_news);
                warning.push('\n');
            }
            if history.last().is_some_and(|h| h.raw.starts_with("(unreadable reply")) {
                // GapGPT, 11-08: the song played and it wrote "the song is playing" three times without "done :".
                warning.push_str(
                    "Your last reply was not an action line. Start every line with an action name; when the goal is reached on screen \
                     reply exactly: done : <what is on screen>.\n",
                );
            }
            if no_effect >= 1 {
                warning.push_str(&format!(
                    "Your last {} had NO visible effect. Do not repeat it; aim at a different spot or use another method (keyboard shortcut, scroll, other control).\n",
                    if no_effect > 1 { format!("{no_effect} actions") } else { "action".into() }
                ));
            }
            let looping = detect_loop(&history);
            loop_warnings = if looping.is_some() { loop_warnings + 1 } else { 0 };
            if let Some(l) = &looping {
                if loop_warnings >= MAX_LOOP_WARNINGS {
                    // Warned and still repeating: going on only burns time (training round 3: open_url x10,
                    // one click x9, the Calculator's C x11 over the result it was asked to read).
                    let why = format!("Stopped: {l} The goal was not reached; tell the user what was done and what is left, do not say it is done.");
                    return Ok(finish(false, why, &notes, looks, actions, &trace, &look.title));
                }
                warning.push_str(&format!("WARNING: {l} STOP doing that. Read what you need, save it with note, then move on."));
                if replans < MAX_REPLANS {
                    replans += 1;
                    let happened = format!("{}\nThe problem: {l}", steps_so_far(&history));
                    let fresh = self
                        .plan(&super::planner::Planning { goal, said, pc: &pc, tips: &skills.for_job(goal, &look.title), memory: "", window: &look.title, happened: &happened }, ctx)
                        .await?;
                    if !fresh.is_empty() {
                        plan_steps = fresh;
                        plan = super::planner::numbered(&plan_steps);
                        warning.push_str("\nTHE PLAN WAS REWRITTEN for where the job is now: follow the new plan.");
                    }
                }
            }

            (ctx.progress)(tr("Deciding the next step…", "تصمیم برای قدم بعدی…").into());
            let t = Instant::now();
            let ask = Ask { goal, said, plan: &plan, history: &history, notes: &notes, actions, expected: &expected, warning: warning.trim() };
            let asked = prompt(&ask, &look);
            expected.clear();
            let decided = match opening.take() {
                Some(open) => {
                    (ctx.log)(format!("the plan opens with {}: run without a model call", open.raw));
                    Ok(open.raw)
                }
                None => self.decide(asked, &look, ctx).await,
            };
            let answer = match decided {
                Ok(a) => {
                    vision_failures = 0;
                    a
                }
                Err(e) if e == "cancelled" => return Err(Stop::Cancelled),
                Err(e) => {
                    vision_failures += 1;
                    (ctx.log)(format!("model failed: {e}"));
                    // Errors that asking again cannot fix (no key, quota used up, bad model id) end the task now.
                    let permanent = ["HTTP 400", "HTTP 401", "HTTP 403", "HTTP 404", "HTTP 429", "No API key"].iter().any(|p| e.contains(p));
                    if permanent || vision_failures >= MAX_VISION_FAILURES {
                        // Said plainly, so the assistant does not hand the same job over again and again: tonight it
                        // called desktop_task four times in 33 s into the same network error, dropping parts of the
                        // goal on each rewrite.
                        let why = format!(
                            "The screen model could not be used ({e}), so nothing more was done and completion was not verified. Calling \
                             desktop_task again now will fail the same way: tell the user what failed instead."
                        );
                        return Ok(finish(false, why, &notes, looks, actions, &trace, &look.title));
                    }
                    Self::pause(600, ctx).await?;
                    continue;
                }
            };
            let batch = match parse_decisions(&answer)
                .into_iter()
                .map(|d| {
                    let raw = d.raw.clone();
                    resolve(d, &look).map_err(|why| (raw, why))
                })
                .collect::<Result<Vec<_>, _>>() {
                Ok(batch) => batch,
                Err((raw, why)) => {
                    (ctx.log)(format!("model {:.1}s: {raw} -> not run: {why}", t.elapsed().as_secs_f32()));
                    history.push(Step { raw, effect: format!("NOT RUN: {why}"), tab: false, ..Default::default() });
                    trim(&mut history);
                    continue;
                }
            };
            (ctx.log)(format!("model {:.1}s: {}", t.elapsed().as_secs_f32(), if batch.is_empty() { short(&answer, 80) } else { batch.iter().map(|d| d.raw.as_str()).collect::<Vec<_>>().join(" | ") }));
            let Some(first) = batch.first() else {
                history.push(Step { raw: format!("(unreadable reply: {})", short(&answer, 40)), effect: "invalid; reply with \"action : params\" lines".into(), tab: false, ..Default::default() });
                continue;
            };

            match &first.action {
                Action::Done(seen) if own_in_front && !reached_app && !about_assistant => {
                    // The job never got out of the assistant's own window, so nothing happened in an app (11-06: "done:
                    // the address bar appears selected and copied" with "AnyTask" in front, then an old clipboard link
                    // became the answer). A job that reached the app and ended with AnyTask in front (the app closed
                    // its window, or was minimized) is believed.
                    if own_pushbacks >= MAX_FAIL_PUSHBACKS {
                        let why = "Not done: the assistant's own window stayed in front, so nothing happened in the app. Tell the user it did not work.";
                        return Ok(finish(false, why.into(), &notes, looks, actions, &trace, &look.title));
                    }
                    own_pushbacks += 1;
                    (ctx.log)(format!("done not accepted: AnyTask's own window is in front ({})", short(seen, 80)));
                    history.push(Step {
                        raw: first.raw.clone(),
                        effect: "NOT ACCEPTED: the assistant's own window \"AnyTask\" is in front, so nothing happened in the app. Bring the \
                                 app to the front (launch_app it) and do the goal there; reply fail if you cannot."
                            .into(),
                        tab: false,
                        ..Default::default()
                    });
                    trim(&mut history);
                    continue;
                }
                Action::Done(seen) if wants_music(goal) && spotify_paused(&look.title) && self.blocking(|o| Ok(o.now_playing())).await.ok().flatten().map(|n| n.0) != Some(true) => {
                    // 11-08: "done: Everybody Knows is playing; to play Shadmehr I must click Bi Ehsas" with Spotify's
                    // title "Spotify Free", which it shows only while nothing plays.
                    if play_pushbacks >= MAX_FAIL_PUSHBACKS {
                        let why = "Not done: nothing plays, Spotify still shows paused. Tell the user the song did not start.";
                        return Ok(finish(false, why.into(), &notes, looks, actions, &trace, &look.title));
                    }
                    play_pushbacks += 1;
                    (ctx.log)(format!("done not accepted: Spotify shows \"{}\", nothing plays ({})", look.title, short(seen, 80)));
                    history.push(Step {
                        raw: first.raw.clone(),
                        effect: "NOT ACCEPTED: nothing plays (Spotify's title shows \"Spotify Free\"/\"Spotify Premium\" only while paused). \
                                 Start the right song (click its play button by name: click : \"Play SONG by ARTIST\"), then reply done."
                            .into(),
                        tab: false,
                        ..Default::default()
                    });
                    trim(&mut history);
                    continue;
                }
                Action::Done(seen) if !number_checked && {
                    let claimed = seen.clone();
                    // Numbers the job already knew are not checked: the goal's, the user's words and the notes (a value
                    // read on an earlier screen is no longer shown when the job ends).
                    let known = format!("{goal}\n{}\n{}", ctx.request, notes.join("\n"));
                    let controls = format!("{}\n{}", look.title, look.elements.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join("\n"));
                    // The elements hold the screen's text lines too (UI Automation's and OCR's, up to a limit); the
                    // screen is read in full only when they do not show the number, and nothing is judged when it cannot be read.
                    let screen = if unseen_number(&claimed, &known, &controls).is_some() { self.blocking(|o| Ok(o.screen_text())).await.ok().flatten() } else { None };
                    let visible = screen.map(|text| format!("{controls}\n{text}"));
                    // Windows OCR reads only the user's profile languages (no Persian here): a Persian screen cannot be judged.
                    let readable = visible.as_deref().is_some_and(|v| !v.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c)));
                    let unseen = if readable { visible.as_deref().and_then(|v| unseen_number(&claimed, &known, v)) } else { None };
                    unseen.is_some_and(|n| {
                        number_checked = true;
                        (ctx.log)(format!("done not accepted: it reports {n}, which is nowhere on the screen ({})", short(&claimed, 60)));
                        true
                    })
                } => {
                    history.push(Step {
                        raw: first.raw.clone(),
                        effect: "NOT ACCEPTED YET: a number you report is not visible on the screen. Read the result from the screen. If the \
                                 screen does show it, reply done again with it exactly as shown. If the display shows something else (the \
                                 typed keys were lost), redo that input; never send a message again."
                            .into(),
                        tab: false,
                        ..Default::default()
                    });
                    trim(&mut history);
                    continue;
                }
                Action::Done(seen) => {
                    // A second look by the judge, once in a job: "done" with a screen that clearly does not show it.
                    if !judge_objected && let Some(judge) = &self.judge {
                        let state = format!(
                            "A desktop agent worked on this goal for the user: {goal}\nIt now says the goal is reached: {seen}\nThe screenshot shows the screen right now."
                        );
                        if let Some(probability) = judge.yes_no(&state, JUDGE_DONE_QUESTION, Some(&look.jpeg)).await {
                            (ctx.log)(format!("second check of done: {:.0}% reached ({})", probability * 100.0, short(seen, 60)));
                            if probability < JUDGE_NOT_DONE {
                                judge_objected = true;
                                history.push(Step {
                                    raw: first.raw.clone(),
                                    effect: "NOT ACCEPTED YET: a second check of this screenshot says the goal does not look reached. Look at the \
                                             screen again: finish what is missing, or reply done again if you can see that it is reached."
                                        .into(),
                                    tab: false,
                                    ..Default::default()
                                });
                                trim(&mut history);
                                continue;
                            }
                        }
                    }
                    (ctx.progress)(short(&first.raw, 70));
                    let found = if notes.is_empty() { String::new() } else { format!(" Found: {}", notes.join("; ")) };
                    let seen = short(seen, 200);
                    let seen = seen.trim_end_matches(['.', '。', ' ']);
                    let message = if seen.is_empty() { format!("The desktop task was completed.{found}") } else { format!("Completed. On screen: {seen}.{found}") };
                    return Ok(finish(true, message, &notes, looks, actions, &trace, &look.title));
                }
                Action::Fail(reason) => {
                    // Not stuck and steps left: send it back for a different approach instead of giving up.
                    if looping.is_none() && fail_pushbacks < MAX_FAIL_PUSHBACKS && step + 3 < MAX_STEPS {
                        fail_pushbacks += 1;
                        (ctx.log)(format!("fail not accepted yet ({fail_pushbacks}/{MAX_FAIL_PUSHBACKS}): {}", short(reason, 100)));
                        if replans < MAX_REPLANS {
                            replans += 1;
                            let happened = format!("{}\nThe executor wants to give up: {}", steps_so_far(&history), short(reason, 200));
                            let fresh = self
                                .plan(&super::planner::Planning { goal, said, pc: &pc, tips: &skills.for_job(goal, &look.title), memory: "", window: &look.title, happened: &happened }, ctx)
                                .await?;
                            if !fresh.is_empty() {
                                plan_steps = fresh;
                                plan = super::planner::numbered(&plan_steps);
                            }
                        }
                        history.push(Step {
                            raw: first.raw.clone(),
                            effect: "NOT ACCEPTED: you are not stuck and have steps left. Try a clearly different way (launch_app the app, \
                                     a keyboard shortcut, search, scroll, open_url). Reply fail again only if it is truly impossible."
                                .into(),
                            tab: false,
                            ..Default::default()
                        });
                        trim(&mut history);
                        continue;
                    }
                    let reason = if reason.is_empty() { "The task could not be completed." } else { reason };
                    return Ok(finish(false, reason.to_string(), &notes, looks, actions, &trace, &look.title));
                }
                Action::Note(text) => {
                    if notes.len() < MAX_NOTES {
                        notes.push(text.clone());
                    }
                    history.push(Step { raw: first.raw.clone(), effect: "saved".into(), tab: false, ..Default::default() });
                    trim(&mut history);
                    continue;
                }
                Action::Ask(question) => {
                    let front = self.blocking(|o| Ok(o.foreground())).await.unwrap_or(0);
                    let allowed = ctx.confirm.confirm(tr("Allow this?", "اجازه هست؟"), question).await;
                    // Answering on the island brings the island to the front: give the focus back to the app, or the
                    // next keys go to the island.
                    if front != 0 {
                        let _ = self.blocking(move |o| o.bring_to_front(front)).await;
                        Self::pause(300, ctx).await?;
                    }
                    approved = allowed;
                    let effect = if allowed { "the user said YES; go ahead" } else { "the user said NO; do not do it (choose another way or fail)" };
                    history.push(Step { raw: first.raw.clone(), effect: effect.into(), tab: false, ..Default::default() });
                    trim(&mut history);
                    continue;
                }
                _ => {}
            }

            expected = expectation(&answer);
            if !expected.is_empty() {
                (ctx.log)(format!("expects: {}", short(&expected, 120)));
            }
            if wants_music(goal) && expects_playing(&expected) {
                awaiting_song = true;
            }
            let before_look = look.fingerprint;
            let mut expect = false;
            for (i, decision) in batch.iter().enumerate() {
                if let Action::LaunchApp(name) = &decision.action
                    && opened.iter().any(|o| o.eq_ignore_ascii_case(name.trim()))
                {
                    // Asked for again ("launch_app : Notepad", later "launch_app : notepad"): its window is brought
                    // to the front instead of opening a second one. A window that is gone is launched again.
                    let app = name.trim().to_string();
                    if let Ok(Some(window)) = self.blocking(move |o| Ok(o.app_window(&app))).await {
                        let _ = self.blocking(move |o| o.bring_to_front(window)).await;
                        history.push(Step { raw: decision.raw.clone(), effect: "it was open already: brought to the front, not opened again".into(), tab: false, ..Default::default() });
                        trim(&mut history);
                        continue;
                    }
                }
                if let Action::LaunchApp(name) = &decision.action
                    && !opened.iter().any(|o| o.eq_ignore_ascii_case(name.trim()))
                {
                    opened.push(name.trim().to_string());
                }
                if matches!(decision.action, Action::OpenUrl(_)) {
                    if opened.contains(&decision.raw) {
                        history.push(Step {
                            raw: decision.raw.clone(),
                            effect: "skipped: already opened in this task; it is open now. Work in it: click or type in what the screenshot shows \
                                     (its controls may come a moment later; text on screen can be clicked by its words or by X Y), do not open it again"
                                .into(),
                            tab: false,
                            ..Default::default()
                        });
                        trim(&mut history);
                        continue;
                    }
                    opened.push(decision.raw.clone());
                }
                (ctx.progress)(short(&decision.raw, 70));
                actions += 1;
                let tab = matches!(&decision.action, Action::Click { target, .. } if clicked(target, &look).is_some_and(|e| e.kind == "tabitem"));
                history.push(Step { raw: decision.raw.clone(), effect: if i + 1 < batch.len() { "done".into() } else { String::new() }, tab, target: target_of(decision, &look), after: 0 });
                trim(&mut history);
                let before = (self.fingerprint().await, self.title().await);
                // Launching an app whose window is already in front only brings it forward again: no new window
                // comes and maybe nothing changes, so waiting for either ran to the 6 s limit (10-31 23:41, 11-03 16:48).
                let in_front = match &decision.action {
                    Action::LaunchApp(name) => {
                        let name = name.clone();
                        self.blocking(move |o| {
                            let front = o.foreground();
                            Ok(front != 0 && o.app_window(&name) == Some(front))
                        })
                        .await
                        .unwrap_or(false)
                    }
                    _ => false,
                };
                if let Some(name) = self.risky_click(&decision.action, &look, goal, &mut judged, &mut approved, ctx).await {
                    let front = self.blocking(|o| Ok(o.foreground())).await.unwrap_or(0);
                    let allowed = ctx.confirm.confirm(tr("Allow this?", "اجازه هست؟"), &format!("{} \"{}\"", tr("Click", "کلیک روی"), short(&name, 60))).await;
                    // Answering on the island brings the island to the front: give the focus back to the app.
                    if front != 0 {
                        let _ = self.blocking(move |o| o.bring_to_front(front)).await;
                        Self::pause(300, ctx).await?;
                    }
                    if !allowed {
                        (ctx.log)(format!("{} not run: the user said no", decision.raw));
                        if let Some(last) = history.last_mut() {
                            last.effect = "NOT RUN: the user said NO to this click; do not do it (choose another way or fail)".into();
                        }
                        expect = false;
                        break;
                    }
                }
                let t = Instant::now();
                if let Err(e) = self.act(decision, &look).await {
                    if ctx.cancel.is_cancelled() {
                        return Err(Stop::Cancelled);
                    }
                    action_failures += 1;
                    (ctx.log)(format!("{} failed: {e}", decision.raw));
                    if action_failures >= MAX_ACTION_FAILURES {
                        return Ok(finish(false, "Desktop actions repeatedly failed; completion was not verified.".into(), &notes, looks, actions, &trace, &look.title));
                    }
                    if let Some(last) = history.last_mut() {
                        last.effect = format!("FAILED ({}); choose a different approach", short(&e, 160));
                    }
                    expect = false;
                    break;
                }
                action_failures = 0;
                trace.push(decision.raw.clone());
                let settle = decision.settle();
                let wait = if in_front { Settle { new_window: false, expect_change: false, ..settle } } else { settle };
                let mut waited = self.settle(wait, (before.0, &before.1), ctx).await?;
                let mut title = self.title().await;
                if matches!(decision.action, Action::OpenUrl(_)) && loading_title(&title) {
                    // The page is still loading: wait for its real title, then for the screen to settle again.
                    let loading = Instant::now();
                    while loading.elapsed() < PAGE_LOAD && loading_title(&title) {
                        Self::pause(POLL_MS, ctx).await?;
                        title = self.title().await;
                    }
                    let calm = Settle { min_ms: 300, max_ms: 3000, expect_change: false, new_window: false };
                    waited += self.settle(calm, (self.fingerprint().await, &title), ctx).await?;
                }
                (ctx.log)(format!("{} took {:.2}s, screen settled after {:.2}s (window \"{title}\")", decision.raw, (t.elapsed() - waited).as_secs_f32(), waited.as_secs_f32()));
                expect = settle.expect_change;
                if settle.new_window && i + 1 < batch.len() && !fresh_document(&title) && !launched_app_ready(&decision.action, &title) {
                    // Typing right after a launch is only safe into a new empty document; anything else (an old
                    // file, a page, nothing new at all) needs a look first.
                    if let Some(last) = history.last_mut() {
                        last.effect = format!("\"{title}\" came to the front, not a new empty document; the next steps were NOT run");
                    }
                    break;
                }
                if i + 1 < batch.len() && title != before.1 && launched_app_ready(&decision.action, &title) {
                    // Its window is up before it takes keys (11-11: "345*12" typed 1 s after the launch was lost and
                    // the display stayed 0).
                    Self::pause(LAUNCH_INPUT_DELAY_MS, ctx).await?;
                }
                let typed = matches!(decision.action, Action::Type { enter: false, .. });
                let window_changed = !typed && title != before.1 && !fresh_document(&title) && !launched_app_ready(&decision.action, &title);
                if i + 1 < batch.len() && (matches!(decision.action, Action::OpenUrl(_)) || window_changed) {
                    // A page opened or the window changed: what comes next must be checked on a new look first.
                    if let Some(last) = history.last_mut() {
                        last.effect = format!("the window is now \"{title}\"; the next steps were NOT run: look first");
                    }
                    break;
                }
            }
            previous = Some((before_look, expect));
        }
        Ok(finish(false, "The step limit was reached before completion could be verified.".into(), &notes, looks, actions, &trace, ""))
    }
}

/// What the executor did so far, for the planner when the job is stuck.
fn steps_so_far(history: &[Step]) -> String {
    if history.is_empty() {
        return "nothing yet".into();
    }
    history
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let target = if h.target.is_empty() { String::new() } else { format!(" [{}]", h.target) };
            format!("{}. {}{target} -> {}", i + 1, h.raw, if h.effect.is_empty() { "?" } else { h.effect.as_str() })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn trim(history: &mut Vec<Step>) {
    if history.len() > HISTORY {
        history.remove(0);
    }
}

#[async_trait]
impl Tool for DesktopTask {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "desktop_task".into(),
            description: "Operates the screen with the mouse and keyboard to reach a goal in desktop apps (click, type, play something \
                          in an app, fill a form). It is slow and visible, so use it only when no background tool can do the job (to only \
                          open an app, use open_app). Give one complete goal in the user's words, e.g. \"open Notepad and write: hello\"."
                .into(),
            parameters: json!({ "type": "object", "properties": { "goal": { "type": "string" } }, "required": ["goal"] }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("On the screen:", "کار روی صفحه:"), short(arg_str(args, "goal").unwrap_or(""), 50))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let goal = arg_str(&args, "goal").ok_or("goal is required.")?;
        (ctx.log)(format!("goal: {goal}"));
        match self.run_goal(goal, ctx).await {
            Ok(result) => Ok(result),
            Err(Stop::Cancelled) => Err("Stopped by the user.".into()),
            Err(Stop::Failed(message)) => Err(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Confirmer, DenyAll};
    use anytask_llm::{GenerateResponse, Generator, LlmError};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn a_point_inside_a_small_control_clicks_its_center() {
        let el = |x, y, w, h| Element::new("button", "", true, (x, y, w, h));
        let mut elements = vec![el(400, 30, 100, 30), el(0, 100, 1000, 800)];
        assert_eq!(snap(&elements, 410, 40), (450.0, 45.0), "small button: its center");
        assert_eq!(snap(&elements, 300, 500), (300.0, 500.0), "inside the big page area: the exact point");
        assert_eq!(snap(&elements, 900, 20), (900.0, 20.0), "nothing there: the exact point");
        elements[0].mid = Some((449.6, 45.3));
        assert_eq!(snap(&elements, 410, 40), (449.6, 45.3), "the control's own exact middle, not the rounded one");
    }

    #[test]
    fn parses_v1_style_decisions() {
        let d = parse_decision("```\n- click : \"Play\"\n```").unwrap();
        assert_eq!(d.action, Action::Click { target: Target::Name("Play".into(), None), button: MouseButton::Left, count: 1 });
        assert!(parse_decision("click_id : 12").is_none() && parse_decision("click : #12").is_none(), "controls have no numbers");
        assert_eq!(parse_decision("double_click : 120, 1400").unwrap().action, Action::Click { target: Target::Point(120, 1000), button: MouseButton::Left, count: 2 });
        let t = parse_decision("type_into_enter : \"Message\" : salam\\ndonya").unwrap();
        assert_eq!(t.action, Action::TypeInto { target: Target::Name("Message".into(), None), text: "salam\ndonya".into(), enter: true });
        assert!(parse_decision("type_into_enter : 3 : salam").is_none(), "no numbered fields");
        assert_eq!(parse_decision("hotkey : Ctrl+L (wait 2.5)").unwrap(), Decision { action: Action::Hotkey("ctrl+l".into()), wait_ms: 2500, raw: "hotkey : ctrl+l".into() });
        assert!(parse_decision("scroll_id : 4 : up").is_none());
        let far = parse_decision("scroll : \"Songs\" : down : 4").unwrap();
        assert_eq!((far.action, far.raw.as_str()), (Action::Scroll { target: Target::Name("Songs".into(), None), up: false, times: 4 }, "scroll : \"Songs\" : down : 4"));
        assert_eq!(parse_decision("scroll : down").unwrap().action, Action::Scroll { target: Target::Point(500, 500), up: false, times: 1 });
        assert_eq!(parse_decision("scroll : down 300 600").unwrap().action, Action::Scroll { target: Target::Point(300, 600), up: false, times: 1 });
        assert_eq!(parse_decision("scroll : up : 3 300 600").unwrap().action, Action::Scroll { target: Target::Point(300, 600), up: true, times: 3 });
        assert_eq!(parse_decision("scroll : down : 50").unwrap().action, Action::Scroll { target: Target::Point(500, 500), up: false, times: MAX_SCROLL });
        assert!(parse_decision("scroll : sideways").is_none());
        assert_eq!(parse_decision("Thinking...\ndone : playing Waka Waka").unwrap().action, Action::Done("playing Waka Waka".into()));
        assert!(parse_decision("open_url : file:///c:/x").is_none(), "only web and spotify links");
        assert!(parse_decision("I will click the button").is_none());
    }

    #[test]
    fn batches_only_steps_that_need_no_new_look() {
        let raws = |t: &str| parse_decisions(t).into_iter().map(|d| d.raw).collect::<Vec<_>>();
        assert_eq!(raws("launch_app : notepad\nhotkey : ctrl+n\ntype : hi"), ["launch_app : notepad", "hotkey : ctrl+n", "type : hi"]);
        assert_eq!(raws("launch_app : notepad\nlaunch_app : paint"), ["launch_app : notepad"], "one new window per answer");
        assert_eq!(raws("hotkey : ctrl+n\ntype : hi\ndone : written"), ["hotkey : ctrl+n", "type : hi"]);
        assert_eq!(raws("type : a\nopen_url : https://x.com\ntype : b"), ["type : a"]);
        assert!(fresh_document("Untitled - Notepad") && fresh_document("*Untitled - Notepad") && fresh_document("New Tab - Google Chrome"));
        assert!(!fresh_document("notes.txt - Notepad") && !fresh_document("state.rs - Visual Studio Code"));
        let launch = |name: &str| Action::LaunchApp(name.into());
        assert!(launched_app_ready(&launch("Calculator"), "Calculator"));
        assert!(!launched_app_ready(&launch("Notepad"), "notes.txt - Notepad"), "a document needs a look first");
        assert!(!launched_app_ready(&launch("Calculator"), "Spotify Free"), "another window came up");
        assert!(!launched_app_ready(&launch("Telegram"), "Telegram"), "typing + Enter in a chat app sends a message");
        assert!(!launched_app_ready(&launch("Spotify"), "Spotify Premium"));
        assert_eq!(raws("click : \"A\"\nclick : \"B\""), ["click : \"A\""], "a second click needs a new look");
        assert_eq!(raws("type : a\ntype : b\ntype : c\ntype : d"), ["type : a", "type : b", "type : c"], "at most three");
        assert_eq!(raws("note : price 5\ntype : x"), ["note : price 5"], "note stands alone");
    }

    #[test]
    fn a_tab_that_shows_its_bare_address_is_still_loading() {
        assert!(loading_title("mail.google.com - Vivaldi"));
        assert!(loading_title("New Tab - Google Chrome"));
        assert!(!loading_title("Gmail - Vivaldi"));
        assert!(!loading_title("صندوق ورودی (۱٬۲۰۸) - me@gmail.com - Gmail - Vivaldi"));
    }

    #[test]
    fn each_action_has_its_own_settle_time() {
        let settle = |t: &str| parse_decision(t).unwrap().settle();
        assert!(settle("launch_app : notepad").new_window);
        assert_eq!(settle("launch_app : notepad").max_ms, 6000);
        assert!(!settle("hotkey : media_play_pause").expect_change);
        assert!(settle("hotkey : media_play_pause").new_window, "waits for the player's title to change");
        assert!(!settle("hotkey : volume_up").new_window);
        assert_eq!(settle("wait : 2").min_ms, 2000);
        assert_eq!(settle("click : \"Play\" (wait 3)").min_ms, 3000);
        assert!(settle("type : x").max_ms < settle("type_enter : x").max_ms);
    }

    #[test]
    fn spots_loops() {
        let step = |raw: &str| Step { raw: raw.into(), effect: String::new(), tab: false, ..Default::default() };
        let same: Vec<Step> = (0..6).map(|i| step(if i % 2 == 0 { "click : \"A\"" } else { "click : \"B\"" })).collect();
        assert!(detect_loop(&same).unwrap().contains("click : \"A\""));
        let varied: Vec<Step> = (0..6).map(|i| step(&format!("click : \"{i}\""))).collect();
        assert!(detect_loop(&varied).is_none());
        let circle: Vec<Step> = ["click : \"C\"", "type_enter : 123*45", "click : \"=\"", "click : \"C\"", "type_enter : 123*45", "click : \"CE\"", "click : \"C\"", "type_enter : 123*45", "click : \"=\"", "click : \"C\""]
            .iter()
            .map(|r| Step { raw: r.to_string(), effect: String::new(), tab: false, ..Default::default() })
            .collect();
        assert!(detect_loop(&circle).unwrap().contains("same few steps"), "a wider circle of steps is a loop too");
        let saved = parse_line(r#"type_into : "File name" : C:\Users\minea\Desktop\night-shift-test\kharid2.txt"#).unwrap().action;
        assert_eq!(saved, Action::TypeInto { target: Target::Name("File name".into(), None), text: r"C:\Users\minea\Desktop\night-shift-test\kharid2.txt".into(), enter: false }, "a path keeps its \\n");
        assert_eq!(parse_line(r"type : salam\ndonya").unwrap().action, Action::Type { text: "salam\ndonya".into(), enter: false });
        assert_eq!(parse_line(r"type : Items:\nmilk").unwrap().action, Action::Type { text: "Items:\nmilk".into(), enter: false }, "a label is not a path");
        assert!(windows_path(r"%USERPROFILE%\Desktop\a.txt") && windows_path(r"save to D:\notes\x.txt") && !windows_path("50% off\\n"));
        let retyping: Vec<Step> = (0..12)
            .map(|i| Step { raw: [r#"type_into : "File name" : C:\x\kharid2.txt"#, "hotkey : enter", "click : \"Save\"", "hotkey : ctrl+s"][i % 4].to_string(), effect: String::new(), tab: false, ..Default::default() })
            .chain((0..4).map(|i| Step { raw: ["hotkey : esc", "hotkey : alt+d", "type_enter : C:\\x", r#"type_into : "File name:" : C:\x\kharid2.txt"#][i].to_string(), effect: String::new(), tab: false, ..Default::default() }))
            .collect();
        assert!(detect_loop(&retyping).unwrap().contains("four times"), "the same text typed again and again");
    }

    /// A fake screen: every action changes the picture except clicks at (900, 900). A launched app needs a few
    /// polls before its window comes to the front.
    #[derive(Default)]
    struct FakeScreen {
        log: Mutex<Vec<String>>,
        frame: Mutex<u64>,
        title: Mutex<String>,
        app_opening: Mutex<u32>,
        /// Title of the window a launch brings forward (default "Untitled - Notepad").
        opens: String,
        /// The launched app's window is already in front: a launch changes nothing.
        in_front: bool,
        /// Title of the user's own window behind the assistant's (empty: none).
        user_title: String,
        /// What the media session says: playing, "Artist - Song" (None: unknown).
        playing: Option<(bool, String)>,
        /// `playing` is only reported once an app was launched (before that nothing plays).
        playing_after_launch: bool,
        /// What OCR would read on the screen (None: cannot read).
        text: Option<String>,
        /// When an app was last launched, and how long after it the first keys were typed.
        launched_at: Mutex<Option<tokio::time::Instant>>,
        typed_after: Mutex<Option<Duration>>,
    }

    impl FakeScreen {
        fn changed(&self) {
            *self.frame.lock().unwrap() += 1;
        }
    }

    impl DesktopOps for FakeScreen {
        fn screen_text(&self) -> Option<String> {
            self.text.clone()
        }
        fn look(&self) -> Result<Look, String> {
            let elements = vec![Element::new("edit", "Text editor", true, (100, 100, 800, 600))];
            Ok(Look { jpeg: vec![0xFF, 0xD8], title: self.active_title(), elements, fingerprint: *self.frame.lock().unwrap() })
        }
        fn fingerprint(&self) -> Result<u64, String> {
            Ok(*self.frame.lock().unwrap())
        }
        fn active_title(&self) -> String {
            let mut opening = self.app_opening.lock().unwrap();
            if *opening > 0 {
                *opening -= 1;
                if *opening == 0 {
                    *self.title.lock().unwrap() = if self.opens.is_empty() { "Untitled - Notepad".into() } else { self.opens.clone() };
                    *self.frame.lock().unwrap() += 1;
                }
            }
            self.title.lock().unwrap().clone()
        }
        fn click(&self, x: u16, y: u16, button: MouseButton, count: u8) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("click {x},{y} {button:?} x{count}"));
            if (x, y) != (900, 900) {
                self.changed();
            }
            Ok(())
        }
        fn scroll(&self, _: u16, _: u16, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn type_text(&self, text: &str) -> Result<(), String> {
            let title = self.title.lock().unwrap().clone();
            self.log.lock().unwrap().push(format!("type {text} into {title}"));
            if let Some(at) = *self.launched_at.lock().unwrap() {
                self.typed_after.lock().unwrap().get_or_insert(at.elapsed());
            }
            self.changed();
            Ok(())
        }
        fn hotkey(&self, combo: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("key {combo}"));
            self.changed();
            Ok(())
        }
        fn launch_app(&self, name: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("launch {name}"));
            *self.launched_at.lock().unwrap() = Some(tokio::time::Instant::now());
            if !self.in_front {
                *self.app_opening.lock().unwrap() = 4;
            }
            Ok(())
        }
        fn open_url(&self, url: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("open {url}"));
            self.changed();
            Ok(())
        }
        fn foreground(&self) -> isize {
            1
        }
        fn app_window(&self, name: &str) -> Option<isize> {
            // In front already (1), or launched during this test and somewhere behind (2).
            let launched = self.log.lock().unwrap().iter().any(|l| l.eq_ignore_ascii_case(&format!("launch {name}")));
            if self.in_front { Some(1) } else { launched.then_some(2) }
        }
        fn user_window(&self) -> Option<isize> {
            (!self.user_title.is_empty()).then_some(7)
        }
        fn own_window_in_front(&self) -> bool {
            self.title.lock().unwrap().as_str() == "AnyTask"
        }
        fn now_playing(&self) -> Option<(bool, String)> {
            if self.playing_after_launch && !self.log.lock().unwrap().iter().any(|l| l.starts_with("launch")) {
                return Some((false, String::new()));
            }
            self.playing.clone()
        }
        fn bring_to_front(&self, window: isize) -> Result<(), String> {
            if window == 7 {
                *self.title.lock().unwrap() = self.user_title.clone();
                self.changed();
            }
            Ok(())
        }
    }

    /// Scripted screen steps. The planner's ask (text only, no screenshot) gets `plans` in turn (no plan once they
    /// run out) and is kept apart in `planned`.
    #[derive(Default)]
    struct Script {
        replies: Mutex<VecDeque<String>>,
        prompts: Mutex<Vec<String>>,
        plans: Mutex<VecDeque<String>>,
        planned: Mutex<Vec<String>>,
    }

    /// The planner's ask: text only.
    fn planning(request: &GenerateRequest) -> bool {
        request.messages[0].parts.len() == 1
    }

    #[async_trait]
    impl Generator for Script {
        async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
            if planning(request) {
                self.planned.lock().unwrap().push(request.system.clone());
                let plan = self.plans.lock().unwrap().pop_front().unwrap_or_default();
                return Ok(GenerateResponse { parts: vec![Part::text(plan)], finish_reason: None, usage: None });
            }
            assert!(matches!(request.messages[0].parts[1], Part::Image { .. }), "the screenshot is sent");
            assert_eq!(request.thinking, Some(Thinking::Low), "screen steps answer without thinking");
            self.prompts.lock().unwrap().push(request.system.clone());
            let reply = self.replies.lock().unwrap().pop_front().unwrap_or_else(|| "fail : script ended".into());
            Ok(GenerateResponse { parts: vec![Part::text(reply)], finish_reason: None, usage: None })
        }
        fn label(&self) -> String {
            "Script".into()
        }
    }

    struct AllowAll;
    #[async_trait]
    impl Confirmer for AllowAll {
        async fn confirm(&self, _: &str, _: &str) -> bool {
            true
        }
    }

    type Lines = Arc<Mutex<Vec<String>>>;

    fn make_ctx(model: Arc<dyn Generator>, confirm: Arc<dyn Confirmer>) -> (ToolCtx, Lines, Lines) {
        let progress: Lines = Arc::default();
        let log: Lines = Arc::default();
        let (p, l) = (progress.clone(), log.clone());
        let ctx = ToolCtx {
            cancel: CancellationToken::new(),
            confirm,
            now_ms: 0,
            model,
            progress: Arc::new(move |s: String| p.lock().unwrap().push(s)),
            log: Arc::new(move |s: String| l.lock().unwrap().push(s)),
            request: String::new(),
        };
        (ctx, progress, log)
    }

    fn script(replies: &[&str]) -> Arc<Script> {
        Arc::new(Script { replies: Mutex::new(replies.iter().map(|s| s.to_string()).collect()), ..Script::default() })
    }

    fn planned_script(plans: &[&str], replies: &[&str]) -> Arc<Script> {
        let model = script(replies);
        *model.plans.lock().unwrap() = plans.iter().map(|s| s.to_string()).collect();
        model
    }

    /// A model whose first ask never answers (a stuck connection); later asks answer at once.
    struct StuckFirst {
        asks: Mutex<u32>,
    }

    #[async_trait]
    impl Generator for StuckFirst {
        async fn generate(&self, request: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
            if planning(request) {
                return Ok(GenerateResponse { parts: vec![Part::text("")], finish_reason: None, usage: None });
            }
            let ask = {
                let mut asks = self.asks.lock().unwrap();
                *asks += 1;
                *asks
            };
            if ask == 1 {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            Ok(GenerateResponse { parts: vec![Part::text("done : the editor is open")], finish_reason: None, usage: None })
        }
        fn label(&self) -> String {
            "stuck".into()
        }
    }

    /// A model that cannot be reached.
    struct Down;

    #[async_trait]
    impl Generator for Down {
        async fn generate(&self, _: &GenerateRequest) -> Result<GenerateResponse, LlmError> {
            Err(LlmError::Network("error sending request for url (https://example.test)".into()))
        }
        fn label(&self) -> String {
            "down".into()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreachable_screen_model_says_not_to_try_again_now() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let (ctx, _, _) = make_ctx(Arc::new(Down), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "write salam" }), &ctx).await.unwrap();
        assert_eq!(result["success"], false);
        let message = result["message"].as_str().unwrap();
        assert!(message.contains("error sending request") && message.contains("Calling desktop_task again now will fail the same way"), "{message}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_stuck_screen_step_is_asked_again_alongside() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = Arc::new(StuckFirst { asks: Mutex::new(0) });
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let started = tokio::time::Instant::now();
        let result = task.run(json!({ "goal": "look at the editor" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        assert_eq!(*model.asks.lock().unwrap(), 2);
        assert!(started.elapsed() < HEDGE_AFTER + Duration::from_secs(2), "the second ask answered: {:?}", started.elapsed());
        assert!(log.lock().unwrap().iter().any(|l| l.contains("asking a second time alongside")));
    }

    /// A second opinion with fixed answers: `done` for "is the goal reached", `risky` for "is this click hard to undo".
    struct FixedJudge {
        done: f64,
        risky: f64,
        asked: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl Judge for FixedJudge {
        async fn yes_no(&self, state: &str, question: &str, jpeg: Option<&[u8]>) -> Option<f64> {
            self.asked.lock().unwrap().push(format!("{question} | image: {}", jpeg.is_some()));
            assert!(state.contains("Text"), "the goal is in the state: {state}");
            Some(if question == JUDGE_DONE_QUESTION { self.done } else { self.risky })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_done_the_second_check_doubts_is_sent_back_once_with_the_screenshot() {
        let judge = Arc::new(FixedJudge { done: 0.05, risky: 0.0, asked: Mutex::default() });
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: Some(judge.clone()), skills: None };
        let model = script(&["done : written", "done : the text is in the editor"]);
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "Text: write salam in the editor" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "the second done is believed: {result}");
        assert_eq!(judge.asked.lock().unwrap().as_slice(), [format!("{JUDGE_DONE_QUESTION} | image: true")], "asked once, with the screenshot");
        assert!(log.lock().unwrap().iter().any(|l| l.contains("second check of done: 5% reached")));
        assert!(model.prompts.lock().unwrap().iter().any(|p| p.contains("a second check of this screenshot")), "the model was told why");
    }

    #[tokio::test(start_paused = true)]
    async fn a_likely_done_or_no_judge_ends_the_job_at_once() {
        let judge = Arc::new(FixedJudge { done: 0.6, risky: 0.0, asked: Mutex::default() });
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: Some(judge), skills: None };
        let model = script(&["done : written"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        assert_eq!(task.run(json!({ "goal": "Text: write salam" }), &ctx).await.unwrap()["success"], true);
        assert_eq!(model.prompts.lock().unwrap().len(), 1, "a doubtful (not clearly wrong) done is not sent back");
    }

    #[tokio::test(start_paused = true)]
    async fn a_click_the_second_check_finds_hard_to_undo_waits_for_the_user() {
        let judge = Arc::new(FixedJudge { done: 1.0, risky: 0.97, asked: Mutex::default() });
        let screen = Arc::new(FakeScreen::default());
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: Some(judge.clone()), skills: None };
        let model = script(&["click : \"Text editor\"", "click : \"Text editor\"", "done : nothing clicked"]);
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        task.run(json!({ "goal": "Text: clear the editor" }), &ctx).await.unwrap();
        assert!(!screen.log.lock().unwrap().iter().any(|l| l.starts_with("click")), "the user said no, so nothing was clicked: {:?}", screen.log.lock().unwrap());
        let risky_asks = judge.asked.lock().unwrap().iter().filter(|q| q.starts_with("Could this click")).count();
        assert_eq!(risky_asks, 1, "the same control is judged once");
        assert!(log.lock().unwrap().iter().any(|l| l.contains("not run: the user said no")));

        // Allowed: the click runs.
        let screen = Arc::new(FakeScreen::default());
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: Some(judge), skills: None };
        let (ctx, _, _) = make_ctx(script(&["click : \"Text editor\"", "done : clicked"]), Arc::new(AllowAll));
        task.run(json!({ "goal": "Text: clear the editor" }), &ctx).await.unwrap();
        assert!(screen.log.lock().unwrap().iter().any(|l| l.starts_with("click")), "{:?}", screen.log.lock().unwrap());
    }

    #[test]
    fn a_reported_number_must_be_on_the_screen() {
        let goal = "Open the Calculator app, calculate 345 multiplied by 12, press Enter, and read the result.";
        assert_eq!(unseen_number("Calculator shows the result 4140", goal, "Calculator
0 =
0").as_deref(), Some("4140"));
        assert_eq!(unseen_number("Calculator shows 4,140 as the result of 345 * 12", goal, "Calculator
345 × 12 =
4,140"), None, "grouped digits match");
        assert_eq!(unseen_number("shows ۴۱۴۰", goal, "4,140"), None, "Persian digits match");
        assert_eq!(unseen_number("the result of 345 * 12 is shown", goal, "Calculator"), None, "the goal's own numbers are not checked");
        assert_eq!(unseen_number("playing Believer", goal, ""), None, "no number, nothing to check");
        assert_eq!(unseen_number("dollar 2,599,000 rial", "read the dollar price", "دلار | 2,599,000 | (0%) 0"), None);
    }

    #[test]
    fn steps_name_their_control_and_the_same_click_leaving_the_same_screen_is_a_loop() {
        let el = |row: u16, name: &str| Element::new("button", name, true, (100, 100 + row * 30, 80, 20));
        let look = Look { elements: vec![el(16, "Sort"), el(18, "Play Dewier's Playlist")], ..Default::default() };
        assert_eq!(target_of(&parse_decision("click : \"Sort\"").unwrap(), &look), "button \"Sort\"");
        assert_eq!(target_of(&parse_decision("click : 140 640").unwrap(), &look), "button \"Play Dewier's Playlist\"");
        assert_eq!(target_of(&parse_decision("hotkey : enter").unwrap(), &look), "");
        let step = |raw: &str, target: &str, after: u64| Step { raw: raw.into(), effect: "screen changed".into(), target: target.into(), after, ..Default::default() };
        // Sort clicked three times (by its name and by its place), the menu open again each time: a loop.
        let sort = vec![step("click : \"Sort\"", "button \"Sort\"", 7), step("click : \"Search\"", "button \"Search\"", 8), step("click : 140 590", "button \"Sort\"", 7), step("click : \"Sort\"", "button \"Sort\"", 7)];
        assert!(detect_loop(&sort).unwrap().contains("Sort"));
        // Next through an installer: a new page each time, not a loop.
        let next: Vec<Step> = (1..=5).map(|i| step("click : \"Next\"", "button \"Next\"", i)).collect();
        assert!(detect_loop(&next).is_none(), "the screen changed each time");
        // Steps in one batch were not looked at between them (after = 0): never counted.
        let batch: Vec<Step> = (0..3).map(|_| step("click : \"5\"", "button \"5\"", 0)).collect();
        assert!(detect_loop(&batch).is_none());
        // Typing the same text again and again is still caught when the field has a name.
        let typing: Vec<Step> = (0..4).map(|i| step("type_into : \"File name:\" : C:\\x\\kharid2.txt", "edit \"File name:\"", 10 + i)).chain([step("hotkey : enter", "", 20)]).collect();
        assert!(detect_loop(&typing).unwrap().contains("kharid2.txt"));
        assert!(expects_playing("the newest song in Anythings playlist is playing"));
        assert!(expects_playing("آهنگ در حال پخش است"));
        assert!(!expects_playing("Dewier's Playlist is open"));
        assert!(!expects_playing("the playlist page displays its songs"));
        assert!(!expects_playing("لیست پخش باز است"));
        // The history shows the control after the effect, outside the action the model might copy.
        let ask = Ask { goal: "g", said: "", plan: "", history: &sort[..1], notes: &[], actions: 1, expected: "", warning: "" };
        assert!(prompt(&ask, &Look::default()).contains("1. click : \"Sort\" -> screen changed [button \"Sort\"]"));
    }

    #[test]
    fn opening_the_same_app_again_and_again_is_a_loop() {
        let skipped = |raw: &str| Step { raw: raw.into(), effect: "skipped: already opened in this task; it is open now".into(), tab: false, ..Default::default() };
        let opened = || Step { raw: "open_url : spotify:search:music".into(), effect: "done".into(), tab: false, ..Default::default() };
        assert!(detect_loop(&[opened(), skipped("open_url : spotify:search:music")]).is_none(), "once is a slip");
        let looping = detect_loop(&[opened(), skipped("open_url : spotify:search:music"), skipped("open_url : spotify:search:music")]);
        assert!(looping.unwrap().contains("again although it is open"));
    }

    #[test]
    fn the_expect_line_is_read() {
        assert_eq!(expectation("click : \"Search\"
expect : the search results are shown"), "the search results are shown");
        assert_eq!(expectation("- **Expect: a new tab opens**"), "a new tab opens");
        assert_eq!(expectation("click : \"Search\""), "");
    }

    #[test]
    fn controls_are_clicked_by_name_on_their_exact_middle() {
        let el = |kind: &str, name: &str, x, y| Element::new(kind, name, true, (x, y, 60, 20));
        let mut exact = el("button", "Play", 500, 900);
        exact.mid = Some((529.4, 910.6));
        let look = Look {
            elements: vec![
                exact,
                el("button", "Play Believer by Imagine Dragons", 300, 400),
                el("button", "More options", 700, 400),
                el("button", "More options", 700, 500),
                el("edit", "What do you want to play?", 400, 50),
                el("text", "Believer", 330, 400),
                el("button", "", 900, 50),
                el("button", "Minimize", 940, 0),
                el("button", "Close", 970, 0),
            ],
            ..Default::default()
        };
        // Where each line clicks: the named control's own middle.
        let spot_of = |line: &str| {
            let decision = resolve(parse_decision(line).unwrap(), &look)?;
            match &decision.action {
                Action::Click { target, .. } | Action::TypeInto { target, .. } | Action::Scroll { target, .. } => spot(target, &look.elements),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(spot_of("click : \"Play\""), Ok((529.4, 910.6)), "the same name before longer ones, on its exact middle");
        assert_eq!(spot_of("click : Play Believer"), Ok((330.0, 410.0)), "a name without quotes, by its start");
        assert_eq!(spot_of("double_click : \"believer\""), Ok((360.0, 410.0)), "the same name first, even a text line (the song's title)");
        assert_eq!(spot_of("click : \"Play Believer\""), Ok((330.0, 410.0)), "a control before a text line when both only start with it");
        assert_eq!(spot_of("click : \"More options\" @ 730 505"), Ok((730.0, 510.0)), "the nearest of two");
        assert_eq!(spot_of("type_into_enter : \"What do you want to play?\" : Believer"), Ok((430.0, 60.0)));
        assert_eq!(spot_of("scroll : \"More options\" : down : 3"), Ok((730.0, 410.0)));
        assert_eq!(spot_of("click : \"Play Believer by Imagine Dra\u{2026}\""), Ok((330.0, 410.0)), "a name the list shortened");
        assert!(spot_of("click : \"Shuffle\"").unwrap_err().contains("Nothing named \"Shuffle\""));
        assert_eq!(parse_decision("click : 120 400").unwrap().action, Action::Click { target: Target::Point(120, 400), button: MouseButton::Left, count: 1 });
        // The list: names only, places where a name repeats or there is none.
        let ask = Ask { goal: "play Believer", said: "", plan: "", history: &[], notes: &[], actions: 0, expected: "", warning: "" };
        let text = prompt(&ask, &look);
        assert!(text.contains("button \"Play Believer by Imagine Dragons\"\n"), "{text}");
        assert!(text.contains("button \"More options\" @ 730 410") && text.contains("button \"More options\" @ 730 510"), "{text}");
        assert!(text.contains("button (no name) @ 930 60"), "{text}");
        assert!(!text.contains("Minimize") && !text.contains("\"Close\""), "the window's own buttons are not listed: {text}");
    }

    #[test]
    fn text_lines_stand_between_the_controls_in_reading_order() {
        let el = |kind: &str, name: &str, y| Element::new(kind, name, true, (100, y, 600, 20));
        let look = Look { elements: vec![el("dataitem", "12 Last song", 400), el("dataitem", "Suggested song", 600), el("text", "Recommended", 500)], ..Default::default() };
        let ask = Ask { goal: "play the last song of the playlist", said: "", plan: "", history: &[], notes: &[], actions: 0, expected: "", warning: "" };
        let text = prompt(&ask, &look);
        let at = |s: &str| text.find(s).unwrap_or_else(|| panic!("{s} missing in {text}"));
        assert!(at("dataitem \"12 Last song\"") < at("text \"Recommended\""));
        assert!(at("text \"Recommended\"") < at("dataitem \"Suggested song\""));
        assert!(!text.contains("1 dataitem"), "no numbers: controls are clicked by name");
    }

    #[tokio::test(start_paused = true)]
    async fn the_next_look_checks_what_the_last_actions_were_expected_to_do() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = script(&["click : \"Text editor\"
type : hi
expect : hi is in the editor", "done : hi is in the editor"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        task.run(json!({ "goal": "Open Telegram and write hi to Ali" }), &ctx).await.unwrap();
        let prompts = model.prompts.lock().unwrap();
        assert!(!prompts[0].contains("YOU EXPECTED"));
        assert!(prompts[1].contains("YOU EXPECTED after your last actions: hi is in the editor"), "{}", prompts[1]);
        // The fixed rules come first (cached by the provider), the job and the screen after them.
        assert!(prompts[0].starts_with("You are a careful, fast expert"));
        assert!(prompts[0].find("OUTPUT FORMAT").unwrap() < prompts[0].find("User goal:").unwrap());
        assert!(prompts[0].find("User goal:").unwrap() < prompts[0].find("ON THE SCREEN, top to bottom").unwrap());
        assert!(!prompts[0].contains("TIPS FOR THE APPS IN THIS JOB"), "the executor gets the plan, not the tips");
        assert!(model.planned.lock().unwrap()[0].contains("TIPS FOR THE APPS IN THIS JOB:
# Telegram"), "the app's tips go to the planner");
    }

    #[tokio::test(start_paused = true)]
    async fn the_planner_writes_the_steps_and_the_executor_follows_them() {
        // A real title: an empty one reads as a page still loading, which is waited for in real time.
        let screen = Arc::new(FakeScreen { title: Mutex::new("Spotify - Search".into()), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = planned_script(
            &["1. open_url : spotify:search:Believer Imagine Dragons\n2. click : \"Play Believer by Imagine Dragons\"\n3. done when the title shows the song"],
            &["done : Believer plays"],
        );
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "play Believer by Imagine Dragons in Spotify" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        // The opening link ran on the first look without asking the executor: one screen ask in all.
        assert!(screen.log.lock().unwrap().iter().any(|a| a.contains("spotify:search:Believer")), "{:?}", screen.log.lock().unwrap());
        let prompts = model.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1, "the opening step needed no model call");
        assert!(prompts[0].contains("THE PLAN (written for this job by the planner; do its next step that is not done yet):\n1. open_url : spotify:search:Believer Imagine Dragons\n2. click"), "{}", prompts[0]);
        assert!(prompts[0].contains("1. open_url : spotify:search:Believer Imagine Dragons ->"), "the opening step is in the executor's history");
        assert!(!prompts[0].contains("CHOOSING THE WAY"), "the long rules stay with the planner");
        let planned = model.planned.lock().unwrap();
        assert_eq!(planned.len(), 1);
        assert!(planned[0].starts_with("You plan a job") && planned[0].contains("THE GOAL: \"play Believer by Imagine Dragons in Spotify\""));
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("plan ") && l.contains("open_url : spotify:search:Believer")));
    }

    #[tokio::test(start_paused = true)]
    async fn the_planners_memory_never_reaches_the_executor() {
        let learner = Arc::new(crate::learn::Learner::in_memory());
        learner.record_desktop("play Dirty Cash in the Spotify app", &["open_url : spotify:search:Dirty Cash".into(), "double_click_id : 41".into()], true, "PAWSA - Dirty Cash", 12.0, 1);
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: Some(learner), judge: None, skills: None };
        let model = planned_script(&[""], &["done : the editor is open"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        task.run(json!({ "goal": "play Shakira in the Spotify app" }), &ctx).await.unwrap();
        assert!(model.planned.lock().unwrap()[0].contains("FROM EARLIER RUNS ON THIS PC:"), "the planner reads what worked before");
        let prompts = model.prompts.lock().unwrap();
        assert!(!prompts[0].contains("EARLIER RUNS") && !prompts[0].contains("Dirty Cash") && !prompts[0].contains("THE PLAN (written"), "{}", prompts[0]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stuck_executor_gets_a_new_plan() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = planned_script(&["1. click : \"Text editor\"\n2. done", "1. hotkey : ctrl+n\n2. type : hi"], &["fail : I cannot find it", "done : hi is written"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "write hi in the editor" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        let planned = model.planned.lock().unwrap();
        assert_eq!(planned.len(), 2, "asked again after the executor wanted to give up");
        assert!(planned[1].contains("THE JOB IS UNDER WAY AND STUCK") && planned[1].contains("I cannot find it"), "{}", planned[1]);
        assert!(model.prompts.lock().unwrap()[1].contains("1. hotkey : ctrl+n\n2. type : hi"), "the executor follows the new plan");
    }

    #[tokio::test(start_paused = true)]
    async fn a_job_starts_from_the_users_window_not_the_assistants() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("AnyTask".into()), user_title: "Example Domain - Vivaldi".into(), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["done : the page is open"]);
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "copy the address of the open page" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        assert!(model.prompts.lock().unwrap()[0].contains("Active window: Example Domain - Vivaldi"));
        assert!(log.lock().unwrap().iter().any(|l| l.contains("started from the user's window")));
    }

    #[tokio::test(start_paused = true)]
    async fn done_with_the_assistants_own_window_in_front_is_not_believed() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("AnyTask".into()), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["done : the address bar appears selected and copied", "done : copied", "done : copied"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "In the browser, copy the address" }), &ctx).await.unwrap();
        assert_eq!(result["success"], false, "{result}");
        assert!(result["message"].as_str().unwrap().contains("own window stayed in front"));
        assert!(model.prompts.lock().unwrap()[1].contains("NOT ACCEPTED: the assistant's own window"));
    }

    #[tokio::test(start_paused = true)]
    async fn done_after_the_app_was_reached_is_believed_with_anytask_in_front() {
        // The app closed or minimized its window at the end, so the assistant's window came to the front.
        let screen = Arc::new(FakeScreen { title: Mutex::new("Untitled - Notepad".into()), opens: "AnyTask".into(), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : Run", "done : the dialog closed"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "run the command" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
    }

    #[tokio::test(start_paused = true)]
    async fn done_for_a_song_is_not_believed_while_spotify_shows_paused() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("Spotify Free".into()), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["done : to play Shadmehr I must click Bi Ehsas", "click : \"Text editor\"", "done : playing", "done : playing"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "In the Spotify app, search for Shadmehr and play a song" }), &ctx).await.unwrap();
        assert!(model.prompts.lock().unwrap()[1].contains("NOT ACCEPTED: nothing plays"));
        // The fake's title never changes: nothing ever played, so it is not reported as done.
        assert_eq!(result["success"], false, "{result}");
        assert!(result["message"].as_str().unwrap().contains("Not done: nothing plays"), "{result}");
        assert!(wants_music("Play Enemy") && wants_music("پخش کن") && !wants_music("Pause the music in Spotify") && !wants_music("Open Notepad"));
        assert!(!wants_music("In Spotify, open my playlist Chill") && !wants_music("Open Spotify") && !wants_music("Change the display"));
        assert!(spotify_paused("Spotify Premium") && !spotify_paused("Spotify") && !spotify_paused("Shadmehr Aghili - Bi Ehsas"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_asked_song_playing_ends_the_job_without_another_model_round() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("Spotify Free".into()), opens: "Imagine Dragons - Believer".into(), playing: Some((true, "Imagine Dragons - Believer".into())), playing_after_launch: true, ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : Spotify"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "In Spotify, play the song Believer by Imagine Dragons" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        assert_eq!(model.prompts.lock().unwrap().len(), 1, "no second model round");
        assert!(song_of_goal("Open Spotify, search for Enemy and play it", "Imagine Dragons - Enemy - From the series Arcane League of Legends"));
        assert!(!song_of_goal("Open Spotify, search for Believer and play it", "Avicii - Waiting For Love"), "only shared filler words");
        assert!(!song_of_goal("play Believer by Imagine Dragons", "Imagine Dragons - Thunder"), "the artist alone is not the song");
        assert!(!song_of_goal("play a song by Shadmehr", "Shadmehr Aghili - Bi Ehsas"), "artist-only goals go to the model");
        assert!(!song_of_goal("play some music", "Sigrid - Everybody Knows"), "any music: the model decides");
    }

    #[tokio::test(start_paused = true)]
    async fn a_done_with_a_number_the_screen_does_not_show_is_sent_back_once() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("Calculator".into()), text: Some("Calculator\nStandard\n0 =\n0".into()), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["done : Calculator shows 4140", "type_enter : 345*12", "done : Calculator shows 4140"]);
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "In Calculator, compute 345*12" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        assert!(log.lock().unwrap().iter().any(|l| l.contains("it reports 4140, which is nowhere on the screen")));
        assert!(model.prompts.lock().unwrap()[1].contains("a number you report is not visible"), "the model was told why");
        assert!(screen.log.lock().unwrap().iter().any(|l| l.starts_with("type 345*12")), "it typed again: {:?}", screen.log.lock().unwrap());
    }

    /// Spotify in front; a click starts the song, so its window takes the song's title.
    struct SpotifyInFront(FakeScreen);
    impl DesktopOps for SpotifyInFront {
        fn look(&self) -> Result<Look, String> {
            self.0.look()
        }
        fn click(&self, _: u16, _: u16, _: MouseButton, _: u8) -> Result<(), String> {
            *self.0.title.lock().unwrap() = "Imagine Dragons - Enemy (with JID) - from the series Arcane League of Legends".into();
            self.0.changed();
            Ok(())
        }
        fn scroll(&self, x: u16, y: u16, up: bool) -> Result<(), String> {
            self.0.scroll(x, y, up)
        }
        fn type_text(&self, text: &str) -> Result<(), String> {
            self.0.type_text(text)
        }
        fn hotkey(&self, combo: &str) -> Result<(), String> {
            self.0.hotkey(combo)
        }
        fn launch_app(&self, name: &str) -> Result<(), String> {
            self.0.launch_app(name)
        }
        fn open_url(&self, url: &str) -> Result<(), String> {
            self.0.open_url(url)
        }
        fn fingerprint(&self) -> Result<u64, String> {
            self.0.fingerprint()
        }
        fn active_title(&self) -> String {
            self.0.active_title()
        }
        fn foreground(&self) -> isize {
            1
        }
        fn app_window(&self, name: &str) -> Option<isize> {
            name.eq_ignore_ascii_case("spotify").then_some(1)
        }
        fn now_playing(&self) -> Option<(bool, String)> {
            // Windows follows a browser tab that plays something else.
            Some((true, "Ed Sheeran - Perfect (Official Music Video)".into()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_song_in_spotifys_title_ends_the_job_when_another_player_holds_the_media_session() {
        let screen = Arc::new(SpotifyInFront(FakeScreen { title: Mutex::new("Spotify Free".into()), ..FakeScreen::default() }));
        let task = DesktopTask { ops: screen, learner: None, judge: None, skills: None };
        let model = script(&["double_click : \"Text editor\"", "done : should not be needed"]);
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "In Spotify, search for 'Enemy by Imagine Dragons' and play the song." }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        assert_eq!(model.prompts.lock().unwrap().len(), 1, "no second model round");
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("Spotify plays the asked song")));
    }

    #[tokio::test(start_paused = true)]
    async fn an_app_asked_for_twice_is_opened_once() {
        let screen = Arc::new(FakeScreen::default());
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : Notepad", "launch_app : notepad", "done : Notepad is open"]);
        let (ctx, _, _) = make_ctx(model, Arc::new(DenyAll));
        task.run(json!({ "goal": "Open Notepad" }), &ctx).await.unwrap();
        let launches = screen.log.lock().unwrap().iter().filter(|l| l.to_lowercase() == "launch notepad").count();
        assert_eq!(launches, 1, "{:?}", screen.log.lock().unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn keys_wait_a_moment_after_a_fresh_calculator_launch() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("Example - Vivaldi".into()), opens: "Calculator".into(), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let (ctx, _, _) = make_ctx(script(&["launch_app : Calculator\ntype_enter : 12*3", "done : 36"]), Arc::new(DenyAll));
        task.run(json!({ "goal": "In Calculator, compute 12*3" }), &ctx).await.unwrap();
        let after = screen.typed_after.lock().unwrap().expect("typed");
        assert!(after >= Duration::from_millis(LAUNCH_INPUT_DELAY_MS), "typed {after:?} after the launch");
    }

    #[tokio::test(start_paused = true)]
    async fn keys_typed_right_after_launching_calculator_run() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("Example - Vivaldi".into()), opens: "Calculator".into(), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : Calculator
type_enter : 987*65", "done : 64155"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "In Calculator, compute 987*65" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        assert!(screen.log.lock().unwrap().iter().any(|l| l == "type 987*65 into Calculator"), "{:?}", screen.log.lock().unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn the_screen_model_knows_the_pcs_browser() {
        struct WithBrowser(FakeScreen);
        impl DesktopOps for WithBrowser {
            fn look(&self) -> Result<Look, String> {
                self.0.look()
            }
            fn click(&self, x: u16, y: u16, b: MouseButton, n: u8) -> Result<(), String> {
                self.0.click(x, y, b, n)
            }
            fn scroll(&self, x: u16, y: u16, up: bool) -> Result<(), String> {
                self.0.scroll(x, y, up)
            }
            fn type_text(&self, t: &str) -> Result<(), String> {
                self.0.type_text(t)
            }
            fn hotkey(&self, c: &str) -> Result<(), String> {
                self.0.hotkey(c)
            }
            fn launch_app(&self, n: &str) -> Result<(), String> {
                self.0.launch_app(n)
            }
            fn open_url(&self, u: &str) -> Result<(), String> {
                self.0.open_url(u)
            }
            fn default_browser(&self) -> Option<String> {
                Some("Vivaldi".into())
            }
        }
        let task = DesktopTask { ops: Arc::new(WithBrowser(FakeScreen::default())), learner: None, judge: None, skills: None };
        let model = script(&["done : the editor is open"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        task.run(json!({ "goal": "Open YouTube in a browser" }), &ctx).await.unwrap();
        let prompt = &model.planned.lock().unwrap()[0];
        assert!(prompt.contains("THIS PC: the web browser is Vivaldi"), "the planner picks the way, so it knows the browser");
        assert!(!prompt.contains("FROM EARLIER RUNS ON THIS PC"), "it is not an earlier run");
    }

    #[tokio::test(start_paused = true)]
    async fn a_goal_about_anytask_itself_may_end_in_its_window() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("AnyTask".into()), user_title: "Notes - Notepad".into(), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["done : AnyTask settings are open"]);
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "Open the AnyTask settings" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true, "{result}");
        assert!(!log.lock().unwrap().iter().any(|l| l.contains("started from the user's window")), "it stays in AnyTask");
    }

    #[tokio::test(start_paused = true)]
    async fn fail_pushbacks_do_not_use_up_the_own_window_check() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("AnyTask".into()), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["fail : cannot see it", "fail : still not", "done : copied"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "In the browser, copy the address" }), &ctx).await.unwrap();
        assert_eq!(result["success"], false, "{result}");
        assert!(model.prompts.lock().unwrap()[3].contains("NOT ACCEPTED: the assistant's own window"));
    }

    #[tokio::test(start_paused = true)]
    async fn launching_an_app_already_in_front_does_not_wait_for_a_new_window() {
        let screen = Arc::new(FakeScreen { title: Mutex::new("Spotify Free".into()), in_front: true, ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : Spotify", "done : Spotify is in front"]);
        let (ctx, _, log) = make_ctx(model, Arc::new(DenyAll));
        let started = tokio::time::Instant::now();
        let result = task.run(json!({ "goal": "open spotify" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true);
        assert!(started.elapsed() < Duration::from_secs(2), "no 6 s wait for a window that is already there: {:?}", started.elapsed());
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("launch_app : Spotify took")));
    }

    #[tokio::test(start_paused = true)]
    async fn a_batch_waits_for_the_app_before_typing() {
        let screen = Arc::new(FakeScreen::default());
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : notepad\ntype : salam", "done : salam is written in Notepad"]);
        let (ctx, _, log) = make_ctx(model.clone(), Arc::new(DenyAll));
        let started = tokio::time::Instant::now();
        let result = task.run(json!({ "goal": "open notepad and write salam" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(*screen.log.lock().unwrap(), vec!["launch notepad", "type salam into Untitled - Notepad"], "typed only once Notepad was in front");
        assert_eq!(model.prompts.lock().unwrap().len(), 2, "launch and typing in one decision, then confirm");
        assert!(started.elapsed() < Duration::from_secs(5), "settling, not fixed long waits: {:?}", started.elapsed());
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("launch_app : notepad took")));
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_type_into_an_old_file_that_opened() {
        let screen = Arc::new(FakeScreen { opens: "notes.txt - Notepad".into(), ..FakeScreen::default() });
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : notepad\ntype : salam", "fail : stop here"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        task.run(json!({ "goal": "open notepad and write salam" }), &ctx).await.unwrap();
        assert_eq!(*screen.log.lock().unwrap(), vec!["launch notepad"], "nothing typed into notes.txt");
        assert!(model.prompts.lock().unwrap()[1].contains("not a new empty document; the next steps were NOT run"));
    }

    #[test]
    fn closing_windows_and_discarding_work_are_never_allowed() {
        let dialog = Look {
            jpeg: vec![0xFF, 0xD8],
            title: "Notepad".into(),
            elements: vec![
                Element::new("button", "Don't save", true, (400, 500, 100, 40)),
                Element::new("button", "Cancel", true, (520, 500, 100, 40)),
            ],
            fingerprint: 0,
        };
        let click = |target| Action::Click { target, button: MouseButton::Left, count: 1 };
        let named = |name: &str| Target::Name(name.into(), None);
        assert!(forbidden(&Action::Hotkey("ctrl+w".into()), &dialog).is_some());
        assert!(forbidden(&Action::Hotkey("alt+f4".into()), &dialog).is_some());
        assert!(forbidden(&click(named("Don't save")), &dialog).unwrap().contains("Cancel"), "Don't save by its name");
        assert!(forbidden(&click(Target::Point(450, 520)), &dialog).is_some(), "Don't save by its place");
        assert!(forbidden(&click(named("Cancel")), &dialog).is_none(), "Cancel is fine");
        let tab = Look { elements: vec![Element::new("button", "Close Tab", true, (10, 10, 20, 20))], ..dialog.clone() };
        assert!(forbidden(&click(named("Close Tab")), &tab).is_some(), "a tab's X closes it too");
        assert!(forbidden(&Action::Hotkey("ctrl+n".into()), &dialog).is_none());
        // 11-10: the window menu, Task Manager and its End task.
        assert!(forbidden(&Action::Hotkey("alt+space".into()), &dialog).is_some());
        assert!(forbidden(&Action::LaunchApp("Task Manager".into()), &dialog).is_some());
        assert!(forbidden(&Action::Type { text: "Task Manager".into(), enter: true }, &dialog).is_some());
        assert!(forbidden(&Action::Type { text: "taskkill /im spotify.exe".into(), enter: true }, &dialog).is_some());
        let task_manager = Look {
            title: "Task Manager".into(),
            elements: vec![Element::new("button", "End task", true, (10, 10, 20, 20))],
            ..dialog.clone()
        };
        assert!(forbidden(&Action::Type { text: "Spotify".into(), enter: true }, &task_manager).is_some());
        assert!(forbidden(&click(named("End task")), &Look { title: "Something".into(), ..task_manager.clone() }).is_some());
        assert!(forbidden(&Action::LaunchApp("Spotify".into()), &dialog).is_none());
        assert!(forbidden(&Action::Type { text: "run the tests".into(), enter: false }, &dialog).is_none(), "words, not the Run box");
        assert!(forbidden(&Action::Type { text: "Run".into(), enter: true }, &dialog).is_none(), "a song called Run");
        assert!(forbidden(&Action::LaunchApp("Run".into()), &dialog).is_some());
        assert!(forbidden(&Action::Hotkey("ctrl+shift+esc".into()), &dialog).is_some());
        let video = Look { title: "Run Boy Run - Woodkid - YouTube - Vivaldi".into(), ..dialog.clone() };
        assert!(forbidden(&Action::Hotkey("ctrl+l".into()), &video).is_none(), "an ordinary window whose title starts with Run");
        let game = Look { elements: vec![Element::new("button", "Restart", true, (10, 10, 20, 20))], ..dialog.clone() };
        assert!(forbidden(&click(named("Restart")), &game).is_none(), "an app's own Restart");
        assert!(forbidden(&click(named("Restart")), &Look { title: "Start".into(), ..game.clone() }).is_some(), "Restart in the Start menu");
    }

    #[tokio::test(start_paused = true)]
    async fn an_agent_that_keeps_repeating_itself_is_stopped() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = script(&["click : \"Text editor\""; 30]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "read the result" }), &ctx).await.unwrap();
        assert_eq!(result["success"], false);
        // The same control again and again, the screen the same after each click: caught before six repeats.
        assert!(result["message"].as_str().unwrap().contains("Stopped: You did click on edit \"Text editor\" three times"), "{result}");
        assert!(model.prompts.lock().unwrap().len() <= 10, "stopped soon after the warnings: {}", model.prompts.lock().unwrap().len());
    }

    #[tokio::test(start_paused = true)]
    async fn reaches_the_goal_with_named_controls_and_notes() {
        let screen = Arc::new(FakeScreen::default());
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["note : editor is open", "type_into : \"Text editor\" : salam", "done : salam is written"]);
        let (ctx, progress, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "write salam" }), &ctx).await.unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["notes"][0], "editor is open");
        assert_eq!(*screen.log.lock().unwrap(), vec!["click 500,400 Left x1", "key ctrl+a", "type salam into "]);
        let prompts = model.prompts.lock().unwrap();
        assert!(prompts[1].contains("1. editor is open"), "notes are shown in later prompts");
        assert!(prompts[2].contains("type_into : \"Text editor\" : salam -> screen changed [edit \"Text editor\"]"), "the step names its control: {}", prompts[2]);
        assert!(prompts[0].contains("edit \"Text editor\""));
        assert!(progress.lock().unwrap().iter().any(|p| p.starts_with("type_into : \"Text editor\"")));
    }

    #[tokio::test(start_paused = true)]
    async fn an_action_without_effect_is_reported_to_the_model() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = script(&["click : 900 900", "fail : stuck"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        let result = task.run(json!({ "goal": "x" }), &ctx).await.unwrap();
        assert_eq!(result["success"], false);
        let prompts = model.prompts.lock().unwrap();
        assert!(prompts[1].contains("-> NO visible change"));
        assert!(prompts[1].contains("had NO visible effect"));
    }

    #[tokio::test(start_paused = true)]
    async fn asks_before_irreversible_steps() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = script(&["ask : Send the message to Ali?", "done : sent"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(AllowAll));
        task.run(json!({ "goal": "send hi to Ali" }), &ctx).await.unwrap();
        assert!(model.prompts.lock().unwrap()[1].contains("the user said YES"));

        let model = script(&["ask : Delete the file?", "fail : not allowed"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        task.run(json!({ "goal": "delete x" }), &ctx).await.unwrap();
        assert!(model.prompts.lock().unwrap()[1].contains("the user said NO"));
    }

    #[tokio::test(start_paused = true)]
    async fn the_users_own_words_reach_the_screen_model() {
        // The voice model wrote the goal and left out "do not send" (seen in the log: the email was sent).
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = script(&["done : the email is ready"]);
        let (mut ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        ctx.request = "ye email baraye ostadam benevis, tu gmail amadash kon vali nafrest".into();
        task.run(json!({ "goal": "Open Gmail compose with the subject and the body" }), &ctx).await.unwrap();
        let prompt = model.prompts.lock().unwrap()[0].clone();
        assert!(prompt.contains("The user's own words") && prompt.contains("vali nafrest"), "{prompt}");

        // When the goal is the user's words, they are not repeated.
        let model = script(&["done : ok"]);
        let (mut ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        ctx.request = "open notepad".into();
        task.run(json!({ "goal": "open notepad" }), &ctx).await.unwrap();
        assert!(!model.prompts.lock().unwrap()[0].contains("The user's own words"));
    }

    #[tokio::test(start_paused = true)]
    async fn does_not_open_the_same_app_twice() {
        let screen = Arc::new(FakeScreen::default());
        let task = DesktopTask { ops: screen.clone(), learner: None, judge: None, skills: None };
        let model = script(&["launch_app : notepad", "launch_app : notepad", "done : open"]);
        let (ctx, _, _) = make_ctx(model.clone(), Arc::new(DenyAll));
        task.run(json!({ "goal": "open notepad" }), &ctx).await.unwrap();
        assert_eq!(*screen.log.lock().unwrap(), vec!["launch notepad"]);
        assert!(model.prompts.lock().unwrap()[2].contains("it was open already: brought to the front"));
    }

    #[tokio::test(start_paused = true)]
    async fn stops_when_cancelled() {
        let task = DesktopTask { ops: Arc::new(FakeScreen::default()), learner: None, judge: None, skills: None };
        let model = script(&["wait : 30", "done : never"]);
        let (ctx, _, _) = make_ctx(model, Arc::new(DenyAll));
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            cancel.cancel();
        });
        assert_eq!(task.run(json!({ "goal": "x" }), &ctx).await.unwrap_err(), "Stopped by the user.");
    }
}
