//! The planner of a screen job: what to use and in which steps, decided before anything is clicked. It is a second
//! agent beside the executor (desktop.rs), which looks at the screen and acts:
//!
//! - The planner sees no screenshot, only the goal, the user's own words, this PC, the tips for the job's apps and
//!   its own memory (runs that worked for similar goals, lessons from mistakes). It knows the long rules (the best way
//!   to do things on Windows and in each app) and writes a short numbered plan.
//! - The executor gets the plan and only the rules it needs on every step, so each step is quicker and has less to
//!   get wrong. When it gets stuck the planner is asked again with what happened.
//!
//! Their memories are kept apart. A saved run's steps went straight into the executor before and were copied
//! blindly, wrong turns and old box numbers included (11-12: the user cleared the learned runs and the jobs got much
//! better). Now only the planner reads the saved runs, as hints for a fresh plan; the executor keeps only what it
//! sees and does in this job (its recent actions and notes).

use super::desktop::{Action, Decision, parse_line};
use super::short;

/// At most this many steps in a plan.
const MAX_STEPS: usize = 10;

pub const RULES: &str = "\
You plan a job on the user's Windows PC. Another agent (the executor) will do it: it looks at the screen after every
step and clicks controls by their names. You do not see the screen. Write the shortest sure way to reach the goal.

OUTPUT: a numbered list of 2 to 8 steps, nothing else. Write a step as an executor action when you know it exactly,
otherwise as a short instruction. The last step says how the executor can SEE that the goal is reached.
Executor actions: launch_app : Name | open_url : https://... (also spotify: and ms-settings: links) |
click : \"name of the control\" | double_click : \"name\" | type_into : \"field name\" : text |
type : text | type_enter : text | hotkey : ctrl+l | scroll : \"list name\" : down : 3 | ask : question | done : ...
Example for \"play Believer by Imagine Dragons in Spotify\":
1. open_url : spotify:search:Believer Imagine Dragons
2. click : \"Play Believer by Imagine Dragons\" (the song's own play button in the results; if there is none,
   double_click the song's row)
3. done when the window title shows \"Imagine Dragons - Believer\" (it plays)

CHOOSING THE WAY (most direct first):
- A web page: open_url with the full address (a search: https://www.google.com/search?q=WORDS; YouTube search:
  https://www.youtube.com/results?search_query=WORDS). Never type an address into a page.
- An app: launch_app : Name as in the Start menu. An app that is open comes to the front the same way. If the app
  is not on this PC (a web app), open its site with open_url instead.
- Spotify: open_url : spotify:search:SONG ARTIST (it opens Spotify too), then the song's play button, named
  \"Play SONG by ARTIST\". Spotify's window title shows \"Artist - Song\" while music plays and \"Spotify Free\" or
  \"Spotify Premium\" while paused.
- Windows Settings pages: open_url : ms-settings:bluetooth (ms-settings:display, ms-settings:sound,
  ms-settings:network-wifi, ms-settings:windowsupdate, ms-settings:apps ...).
- Calculator: launch_app : Calculator, then type_enter : 123*45 (Enter shows the result); never press = again.
- Email in Gmail: open_url : https://mail.google.com/mail/?view=cm&fs=1&to=ADDRESS&su=SUBJECT&body=TEXT (spaces
  as %20, new lines as %0A); the compose window opens filled in. Send only if the goal says send, and ask first.
- Writing in Notepad: launch_app : notepad, hotkey : ctrl+n (a new empty document; Notepad reopens old files),
  type : the text. To save: hotkey : ctrl+s, then in the Save dialog type_into : \"File name\" : the full path
  (use the user's real folders below), then hotkey : enter. Any app: ctrl+s saves, ctrl+shift+s saves as.
- Search inside an app: its search box (often named \"Search\"), or ctrl+f / ctrl+k; type_enter the words, then open
  the right result.
- Chat apps (Telegram, WhatsApp): search the contact, open the chat, click the message box, type the message, and
  ask before sending. Never send when the user said to only prepare or not to send.
- A browser's own pages (settings, history, passwords): open the browser (launch_app), hotkey : ctrl+l, then
  type_enter : vivaldi://settings (chrome://settings, edge://settings for those browsers).
- Long lists: scroll the list by its name, several steps at once (scroll : \"name\" : down : 6), then look.
- Prefer keyboard shortcuts and direct links over hunting for small buttons.

RULES FOR EVERY PLAN:
- Never close a window or tab, never choose \"Don't save\" or \"Discard\", never use Task Manager, a terminal or Run.
- Plan an ask step before anything that cannot be undone: send, post, buy, pay, delete.
- Do exactly what the goal and the user's own words say, nothing more; keep every limit they give.
- Names of controls as the app shows them, in the app's language. Values (songs, names, text to type) exactly as
  the user gave them; you may add the artist or a spelling the app would know.
- FROM EARLIER RUNS are hints, not orders: reuse a way that worked when it fits this goal; control names and values
  change from job to job.";

/// What the planner is asked with.
pub struct Planning<'a> {
    pub goal: &'a str,
    pub said: &'a str,
    /// This PC: the browser, the user's folders.
    pub pc: &'a str,
    /// Tips for the apps of this job (skills.rs).
    pub tips: &'a str,
    /// The planner's own memory: runs that worked for similar goals, lessons (learn.rs).
    pub memory: &'a str,
    /// The window in front now.
    pub window: &'a str,
    /// For a new plan in the middle of a job: what was done and why it is stuck. Empty for the first plan.
    pub happened: &'a str,
}

/// The planner's whole prompt: the fixed rules first (the provider caches an unchanged beginning), the job after.
pub fn prompt(p: &Planning) -> String {
    let mut lines = vec![RULES.to_string(), String::new(), format!("THE GOAL: {}", serde_json::to_string(p.goal).unwrap_or_default())];
    if !p.said.is_empty() && p.said != p.goal.trim() {
        lines.push(format!("The user's own words (follow every limit in them): {}", serde_json::to_string(p.said).unwrap_or_default()));
    }
    if !p.pc.is_empty() {
        lines.push(p.pc.to_string());
    }
    if !p.window.is_empty() {
        lines.push(format!("The window in front now: {}", p.window));
    }
    if !p.tips.is_empty() {
        lines.push(format!("TIPS FOR THE APPS IN THIS JOB:\n{}", p.tips));
    }
    if !p.memory.is_empty() {
        lines.push(format!("FROM EARLIER RUNS ON THIS PC:\n{}", p.memory));
    }
    if !p.happened.is_empty() {
        lines.push(format!(
            "THE JOB IS UNDER WAY AND STUCK. What the executor did so far and what happened:\n{}\nWrite a NEW plan from where the job is now, \
             a clearly different way where the old one failed.",
            p.happened
        ));
    }
    lines.join("\n")
}

/// The plan's steps from the reply: its numbered or bulleted lines (a step's indented second line joins it).
pub fn steps(reply: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in reply.lines() {
        let trimmed = line.trim().trim_matches(|c| c == '`' || c == '*').trim();
        if trimmed.is_empty() {
            continue;
        }
        let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
        let numbered = digits > 0 && trimmed[digits..].starts_with(['.', ')']);
        let body = if numbered {
            trimmed[digits + 1..].trim()
        } else if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("• ")) {
            rest.trim()
        } else if line.starts_with([' ', '\t']) && !out.is_empty() {
            let last = out.last_mut().expect("a step");
            last.push(' ');
            last.push_str(trimmed);
            continue;
        } else {
            continue;
        };
        if !body.is_empty() && out.len() < MAX_STEPS {
            out.push(short(body, 240));
        }
    }
    out
}

/// The plan as the executor reads it.
pub fn numbered(steps: &[String]) -> String {
    steps.iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("\n")
}

/// The step that opens what the job starts in, when the plan begins with one: it runs before the first model call,
/// since opening an app or a page needs no look at the screen. Among leading launch_app and open_url steps the link
/// wins (a spotify: or web link opens its app itself; a launch and then a link in one go would need a look between).
pub fn opening(steps: &[String]) -> Option<Decision> {
    let leading: Vec<Decision> = steps
        .iter()
        .map_while(|s| parse_line(s.split(" (").next().unwrap_or(s).trim()).filter(|d| matches!(d.action, Action::LaunchApp(_) | Action::OpenUrl(_))))
        .collect();
    leading.iter().find(|d| matches!(d.action, Action::OpenUrl(_))).or(leading.first()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plan_is_read_from_numbered_lines() {
        let reply = "Here is the plan:\n1. open_url : spotify:search:Believer Imagine Dragons\n2. click : \"Play Believer by Imagine Dragons\" (its play button;\n   double_click the row if there is none)\n3) done when the title shows \"Imagine Dragons - Believer\"\n";
        let plan = steps(reply);
        assert_eq!(plan.len(), 3, "{plan:?}");
        assert_eq!(plan[1], "click : \"Play Believer by Imagine Dragons\" (its play button; double_click the row if there is none)");
        assert_eq!(numbered(&plan).lines().next(), Some("1. open_url : spotify:search:Believer Imagine Dragons"));
        assert!(steps("I cannot plan this.").is_empty());
        assert_eq!(steps("- launch_app : Calculator\n- type_enter : 2+2").len(), 2);
    }

    #[test]
    fn the_opening_step_runs_without_a_look() {
        let plan = |lines: &[&str]| lines.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let open = |lines: &[&str]| opening(&plan(lines)).map(|d| d.raw);
        assert_eq!(open(&["launch_app : Spotify", "open_url : spotify:search:Believer", "click : \"Play\""]).as_deref(), Some("open_url : spotify:search:Believer"), "the link opens the app itself");
        assert_eq!(open(&["launch_app : Calculator (it opens empty)", "type_enter : 2+2"]).as_deref(), Some("launch_app : Calculator"));
        assert_eq!(open(&["open_url : ms-settings:bluetooth", "click : \"Add device\""]).as_deref(), Some("open_url : ms-settings:bluetooth"));
        assert_eq!(open(&["click : \"Search\"", "open_url : https://x.com"]), None, "only a plan that starts by opening something");
        assert_eq!(open(&["Open the browser's settings"]), None);
    }

    #[test]
    fn the_prompt_has_the_rules_first_and_a_stuck_job_asks_for_a_new_way() {
        let p = Planning { goal: "play Believer", said: "believer ro play kon", pc: "THIS PC: the web browser is Vivaldi", tips: "# Spotify\n- tip", memory: "", window: "Spotify Free", happened: "" };
        let text = prompt(&p);
        assert!(text.starts_with("You plan a job"));
        assert!(text.find("CHOOSING THE WAY").unwrap() < text.find("THE GOAL").unwrap());
        assert!(text.contains("The user's own words (follow every limit in them): \"believer ro play kon\""));
        assert!(text.contains("TIPS FOR THE APPS IN THIS JOB:\n# Spotify") && !text.contains("FROM EARLIER RUNS ON THIS PC:\n") && !text.contains("STUCK"));
        let again = prompt(&Planning { happened: "1. click : \"Play\" -> NO visible change", ..p });
        assert!(again.contains("THE JOB IS UNDER WAY AND STUCK") && again.contains("NO visible change"));
    }
}
