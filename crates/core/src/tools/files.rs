//! File tools inside the user's folders (see paths.rs for what is allowed).

use super::{Tool, ToolCtx, arg_str, short};
use crate::lang::tr;
use crate::paths::{UserFolders, normalize};
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio_util::sync::CancellationToken;

const MAX_READ_BYTES: u64 = 512 * 1024;
const MAX_LIST: usize = 100;
/// How far find_files goes below each folder: deep enough for "Documents/work/2026/reports", not a whole drive.
const FIND_MAX_DEPTH: usize = 5;
const FIND_MAX_MATCHES: usize = 20;
/// A voice answer cannot wait longer than this for a search; a cut search says so (truncated) instead.
const FIND_BUDGET: Budget = Budget { time: Duration::from_secs(3), entries: 20_000 };
/// Folders full of program files, never the user's own documents: walking them only uses up the budget.
const FIND_SKIP: &[&str] = &["node_modules", ".git", "appdata", "$recycle.bin", "system volume information"];
const WRITE_EXTENSIONS: &[&str] = &[
    "txt", "md", "json", "csv", "log", "html", "css", "js", "ts", "jsx", "tsx", "py", "yaml", "yml", "xml", "rtf", "tsv", "ini", "conf", "env",
    "svg", "sql", "sh", "bat", "ps1", "c", "cpp", "h", "java", "go", "rs", "php", "rb", "toml",
];
/// Files that run when opened (a double click on a .py runs it where Python is installed): writing one asks first.
const SCRIPT_EXTENSIONS: &[&str] = &["bat", "ps1", "sh", "cmd", "py", "js"];

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub struct ListDirectory {
    pub folders: Arc<UserFolders>,
}

#[async_trait]
impl Tool for ListDirectory {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "list_directory".into(),
            description: "Lists a folder in the user's storage (up to 100 entries). Accepts Desktop, Downloads, Documents, Music, Pictures, Videos, home, or a path.".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string", "description": "e.g. Desktop or Downloads/projects" } }, "required": ["path"] }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Looking in folder", "دیدن پوشه"), short(arg_str(args, "path").unwrap_or("Desktop"), 40))
    }

    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        let path = self.folders.resolve(arg_str(&args, "path").unwrap_or("Desktop"));
        if let Some(why) = self.folders.blocked(&path, true) {
            return Err(format!("Access denied: {why}."));
        }
        let mut entries: Vec<Value> = Vec::new();
        let mut total = 0;
        for entry in std::fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?.flatten() {
            total += 1;
            if entries.len() < MAX_LIST {
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                entries.push(json!({ "name": entry.file_name().to_string_lossy(), "type": if is_dir { "folder" } else { "file" } }));
            }
        }
        Ok(json!({ "folder": path.display().to_string(), "total": total, "entries": entries }))
    }
}

/// Finds a file by words of its name across all the user's folders in one call, so "file gozaresh ro peyda kon"
/// does not take one list_directory per folder (or a guess).
pub struct FindFiles {
    pub folders: Arc<UserFolders>,
}

#[async_trait]
impl Tool for FindFiles {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find_files".into(),
            description: "Finds files by name in the user's folders (Desktop, Documents, Downloads, Pictures, Music, Videos and their subfolders), \
                          newest first, up to 20, with path, size and modified time. Every word of name must be in the file name (any language). \
                          Use it to find a file before open or read_text_file when you do not know its folder."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Words of the file name, e.g. گزارش or report. * for any name (with ext or modified_within_days)." },
                    "ext": { "type": "string", "description": "File type, e.g. pdf (several: docx,doc)." },
                    "modified_within_days": { "type": "number", "description": "Only files changed in the last N days, e.g. 2 for something from yesterday." },
                    "folder": { "type": "string", "description": "Search only here, e.g. Downloads or Documents/work." }
                },
                "required": ["name"]
            }),
        }
    }

    /// The words and the filters, so the log shows what was searched for ("جستجوی فایل * (pdf، ۲ روز)").
    fn label(&self, args: &Value) -> String {
        let mut filters: Vec<String> = Vec::new();
        if let Some(ext) = arg_str(args, "ext") {
            filters.push(ext.to_string());
        }
        // 0 or less means no limit (the search ignores it), so it is not shown as "0 روز".
        let limit = match args.get("modified_within_days") {
            Some(Value::String(s)) => fold_name(s).trim().parse::<f64>().ok(),
            Some(v) => v.as_f64(),
            None => None,
        };
        if let Some(days) = args.get("modified_within_days").filter(|_| limit.is_some_and(|d| d.is_finite() && d > 0.0)) {
            filters.push(format!("{} {}", days.as_str().map(str::to_string).unwrap_or_else(|| days.to_string()), tr("days", "روز")));
        }
        let name = short(arg_str(args, "name").unwrap_or("*"), 40);
        let search = tr("Searching files:", "جستجوی فایل");
        if filters.is_empty() { format!("{search} {name}") } else { format!("{search} {name} ({})", filters.join(tr(", ", "، "))) }
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let query = Query::parse(&args, SystemTime::now())?;
        let roots = match arg_str(&args, "folder") {
            Some(folder) => {
                let path = self.folders.resolve(folder);
                if let Some(why) = self.folders.blocked(&path, true) {
                    return Err(format!("Access denied: {why}."));
                }
                if !path.is_dir() {
                    return Err(format!("{}: no such folder.", path.display()));
                }
                vec![path]
            }
            None => search_roots(&self.folders),
        };
        let (folders, cancel, walk) = (self.folders.clone(), ctx.cancel.clone(), roots.clone());
        let found = tokio::task::spawn_blocking(move || search(&folders, &walk, &query, &cancel, FIND_BUDGET))
            .await
            .map_err(|e| format!("file search stopped: {e}"))??;
        let matches: Vec<Value> = found
            .hits
            .iter()
            .take(FIND_MAX_MATCHES)
            .map(|hit| {
                if hit.folder {
                    json!({ "path": hit.path.display().to_string(), "type": "folder", "modified": local_time(hit.modified) })
                } else {
                    json!({ "path": hit.path.display().to_string(), "size": human_size(hit.size), "modified": local_time(hit.modified) })
                }
            })
            .collect();
        let mut result = json!({ "matches": matches, "found": found.hits.len(), "searched": found.folders, "truncated": found.truncated });
        let mut note = Vec::new();
        if found.hits.is_empty() {
            let searched: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
            note.push(format!("No file matched. Searched {} and their subfolders.", searched.join(", ")));
        }
        if found.truncated {
            note.push("The search stopped early (time or size limit); narrow it with folder, ext or modified_within_days.".into());
        }
        if !note.is_empty() {
            result["note"] = json!(note.join(" "));
        }
        Ok(result)
    }
}

/// What a file must look like to match: every name word in its folded name, one of the extensions, new enough.
struct Query {
    words: Vec<String>,
    exts: Vec<String>,
    since: Option<SystemTime>,
}

impl Query {
    fn parse(args: &Value, now: SystemTime) -> Result<Self, String> {
        let words: Vec<String> = fold_name(arg_str(args, "name").unwrap_or(""))
            .split(|c: char| c.is_whitespace() || "_-.,،;:()[]*?\"'".contains(c))
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect();
        let exts: Vec<String> = fold_name(arg_str(args, "ext").unwrap_or(""))
            .split(|c: char| c.is_whitespace() || ",،|;".contains(c))
            .map(|e| e.trim_start_matches(['*', '.']).to_string())
            .filter(|e| !e.is_empty())
            .collect();
        let days = match args.get("modified_within_days") {
            Some(Value::String(s)) => fold_name(s).trim().parse::<f64>().ok(),
            Some(v) => v.as_f64(),
            None => None,
        };
        let since = days.filter(|d| d.is_finite() && *d > 0.0).and_then(|d| now.checked_sub(Duration::from_secs_f64(d.min(36_500.0) * 86_400.0)));
        // "*" alone means any file: the newest ones come back (11-11: the model sent it twice for "my latest
        // download" and was refused both times before adding a number of days).
        let any = arg_str(args, "name").is_some_and(|n| n.trim() == "*");
        if words.is_empty() && exts.is_empty() && since.is_none() && !any {
            return Err("name is required: words of the file name (or * with ext or modified_within_days).".into());
        }
        Ok(Self { words, exts, since })
    }

    /// A folder named like the words, when no file type was asked for.
    fn matches_folder(&self, name: &str, modified: Option<SystemTime>) -> bool {
        self.exts.is_empty() && !self.words.is_empty() && self.matches_name(name, modified)
    }

    fn matches(&self, name: &str, modified: Option<SystemTime>) -> bool {
        let folded = fold_name(name);
        let ext = extension(Path::new(&folded));
        if !self.exts.is_empty() && !self.exts.contains(&ext) {
            return false;
        }
        // Desktop shortcuts are not the file (and open refuses them); they come back only when asked for by type.
        if self.exts.is_empty() && matches!(ext.as_str(), "lnk" | "url") {
            return false;
        }
        self.matches_name(name, modified)
    }

    fn matches_name(&self, name: &str, modified: Option<SystemTime>) -> bool {
        let folded = fold_name(name);
        if !self.words.iter().all(|w| folded.contains(w.as_str())) {
            return false;
        }
        match self.since {
            Some(since) => modified.is_some_and(|m| m >= since),
            None => true,
        }
    }
}

/// The same folding as the page filter (anytask-web html.rs), so the user's way of typing still matches the file's:
/// lower case, Persian/Arabic digits as ASCII ("فاکتور ۱۴۰۵" matches 1405), Arabic ي/ك as Persian ی/ک, and no
/// half-space ("گزارش‌ها" matches "گزارشها").
fn fold_name(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| *c != '\u{200c}')
        .map(|c| match c {
            '۰'..='۹' => char::from(b'0' + (c as u32 - '۰' as u32) as u8),
            '٠'..='٩' => char::from(b'0' + (c as u32 - '٠' as u32) as u8),
            'ي' | 'ى' => 'ی',
            'ك' => 'ک',
            _ => c,
        })
        .collect()
}

/// The user's folders that exist and are allowed, each once: a folder inside another one (or the same folder under
/// two names) is already covered by the outer walk.
fn search_roots(folders: &UserFolders) -> Vec<PathBuf> {
    let mut usable: Vec<PathBuf> = Vec::new();
    for root in folders.roots().iter().map(|r| normalize(r)) {
        if root.is_dir() && folders.blocked(&root, true).is_none() && !usable.contains(&root) {
            usable.push(root);
        }
    }
    usable.iter().filter(|r| !usable.iter().any(|outer| outer != *r && r.starts_with(outer))).cloned().collect()
}

#[derive(Clone, Copy)]
struct Budget {
    time: Duration,
    entries: usize,
}

struct Hit {
    path: PathBuf,
    size: u64,
    /// When it last changed or arrived here, whichever is later.
    modified: SystemTime,
    folder: bool,
}

/// When a file last changed or arrived: a file copied from a phone or a USB stick, or taken out of a zip, keeps its
/// old modified time, so "the pdf I downloaded yesterday" is found by its creation time here.
fn arrived(meta: &std::fs::Metadata) -> Option<SystemTime> {
    match (meta.modified().ok(), meta.created().ok()) {
        (Some(m), Some(c)) => Some(m.max(c)),
        (m, c) => m.or(c),
    }
}

#[derive(Default)]
struct Found {
    /// Newest first.
    hits: Vec<Hit>,
    folders: usize,
    truncated: bool,
}

/// Walks the roots breadth first (every folder's top level before anything deep, so a cut search has still seen
/// the likely places), under the same access rules as the other file tools.
fn search(folders: &UserFolders, roots: &[PathBuf], query: &Query, cancel: &CancellationToken, budget: Budget) -> Result<Found, String> {
    let started = Instant::now();
    let mut queue: VecDeque<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    let mut found = Found::default();
    let mut seen = 0;
    'walk: while let Some((dir, depth)) = queue.pop_front() {
        if cancel.is_cancelled() {
            return Err("Stopped".into());
        }
        let Ok(list) = std::fs::read_dir(&dir) else { continue };
        found.folders += 1;
        for entry in list.flatten() {
            seen += 1;
            if seen > budget.entries || started.elapsed() >= budget.time {
                found.truncated = true;
                break 'walk;
            }
            // Links and junctions ("My Music" inside old Documents folders) can loop or leave the user's folders.
            let Ok(kind) = entry.file_type() else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_symlink() || hidden(&name, &meta) {
                continue;
            }
            let when = arrived(&meta);
            if kind.is_dir() {
                let path = entry.path();
                if depth < FIND_MAX_DEPTH && !FIND_SKIP.contains(&name.to_lowercase().as_str()) && folders.blocked(&path, true).is_none() {
                    // A folder is found by its name too ("aksaye shomal ro baz kon"), but not when a file type is asked for.
                    if query.matches_folder(&name, when) {
                        found.hits.push(Hit { path: path.clone(), size: 0, modified: when.unwrap_or(SystemTime::UNIX_EPOCH), folder: true });
                    }
                    queue.push_back((path, depth + 1));
                }
            } else if query.matches(&name, when) {
                found.hits.push(Hit { path: entry.path(), size: meta.len(), modified: when.unwrap_or(SystemTime::UNIX_EPOCH), folder: false });
            }
        }
    }
    found.hits.sort_by_key(|hit| std::cmp::Reverse(hit.modified));
    Ok(found)
}

/// Dot names, hidden entries ($RECYCLE.BIN, desktop.ini, Thumbs.db, app caches) and system files belong to programs,
/// not the user. A folder with only the system flag is still walked: Windows can set that flag on an ordinary folder
/// that was given a custom icon.
fn hidden(name: &str, meta: &std::fs::Metadata) -> bool {
    name.starts_with('.') || hidden_flag(meta)
}

#[cfg(windows)]
fn hidden_flag(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    let attributes = meta.file_attributes();
    attributes & HIDDEN != 0 || (attributes & SYSTEM != 0 && !meta.is_dir())
}

#[cfg(not(windows))]
fn hidden_flag(_: &std::fs::Metadata) -> bool {
    false
}

/// "1.4 MB": a size the user understands when it is read out.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if size < 10.0 { format!("{size:.1} {}", UNITS[unit]) } else { format!("{size:.0} {}", UNITS[unit]) }
}

/// "2026-11-04 18:22" in the user's time zone.
fn local_time(at: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(at).format("%Y-%m-%d %H:%M").to_string()
}

pub struct ReadTextFile {
    pub folders: Arc<UserFolders>,
}

#[async_trait]
impl Tool for ReadTextFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_text_file".into(),
            description: "Reads a text file (up to 512 KB) from the user's storage, e.g. Desktop/notes.txt.".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string" } }, "required": ["path"] }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Reading", "خواندن"), short(arg_str(args, "path").unwrap_or(""), 40))
    }

    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        let path = self.folders.resolve(arg_str(&args, "path").ok_or("path is required.")?);
        if let Some(why) = self.folders.blocked(&path, true) {
            return Err(format!("Access denied: {why}."));
        }
        let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.is_dir() {
            return Err("That is a folder; use list_directory.".into());
        }
        if meta.len() > MAX_READ_BYTES {
            return Err("The file is larger than 512 KB.".into());
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let text = String::from_utf8(bytes).map_err(|_| "Not a text file.".to_string())?;
        Ok(json!({ "path": path.display().to_string(), "content": text }))
    }
}

pub struct WriteTextFile {
    pub folders: Arc<UserFolders>,
    /// The folder the user has open in front (File Explorer), for "save it here"; None: no such window, or this
    /// system cannot tell. May wait for the system, so it is called off the async runtime.
    pub here: Option<Here>,
}

/// Finds the folder the user is looking at.
pub type Here = Arc<dyn Fn() -> Option<std::path::PathBuf> + Send + Sync>;

/// "here/list.txt", "inja/list.txt", "اینجا/list.txt": the file name after the word for "here", if the path starts
/// with it.
fn here_name(path: &str) -> Option<&str> {
    let slashed = path.trim().trim_start_matches(['.', '/', '\\']);
    ["here", "inja", "اینجا"].iter().find_map(|word| {
        let rest = slashed.strip_prefix(word).or_else(|| slashed.get(..word.len()).filter(|h| h.eq_ignore_ascii_case(word)).map(|_| &slashed[word.len()..]))?;
        rest.strip_prefix(['/', '\\']).map(str::trim).filter(|name| !name.is_empty())
    })
}

#[async_trait]
impl Tool for WriteTextFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_text_file".into(),
            description: "Writes a text file in the user's storage (creates folders as needed), e.g. Desktop/prices.txt. Set append to add to the end. \
                          For \"here\" (inja, اینجا) use here/<name>: the folder open in File Explorer, or the Desktop when none is open. \
                          Overwriting an existing file or writing a script asks the user first."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                    "append": { "type": "boolean", "description": "Add to the end instead of replacing." }
                },
                "required": ["path", "content"]
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        format!("{} {}", tr("Writing", "نوشتن"), short(arg_str(args, "path").unwrap_or(""), 40))
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        let asked = arg_str(&args, "path").ok_or("path is required.")?;
        let mut note = None;
        let path = match here_name(asked) {
            Some(name) => {
                let find = self.here.clone();
                let folder = match find {
                    Some(find) => tokio::task::spawn_blocking(move || find()).await.ok().flatten(),
                    None => None,
                };
                match folder {
                    Some(folder) => self.folders.resolve(&folder.join(name).display().to_string()),
                    None => {
                        note = Some("No File Explorer folder is open, so it was saved on the Desktop.");
                        self.folders.resolve(&format!("Desktop/{name}"))
                    }
                }
            }
            None => self.folders.resolve(asked),
        };
        let content = args.get("content").and_then(Value::as_str).ok_or("content is required.")?;
        let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);
        if let Some(why) = self.folders.blocked(&path, false) {
            return Err(format!("Access denied: {why}."));
        }
        let ext = extension(&path);
        if !ext.is_empty() && !WRITE_EXTENSIONS.contains(&ext.as_str()) {
            return Err(format!("Writing .{ext} files is not allowed."));
        }
        let exists = path.exists();
        if exists && path.is_dir() {
            return Err("That is a folder.".into());
        }
        // Adding to an existing file changes it too, so it asks as well and shows which file: training round 3 added
        // to an old Desktop\kharid.txt instead of the one in night-shift-test without asking.
        if exists || SCRIPT_EXTENSIONS.contains(&ext.as_str()) {
            let title = if SCRIPT_EXTENSIONS.contains(&ext.as_str()) {
                "Write a script file?"
            } else if append {
                "Add to this file?"
            } else {
                "Overwrite this file?"
            };
            if !ctx.confirm.confirm(title, &path.display().to_string()).await {
                return Err("The user did not allow it.".into());
            }
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        if append {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| e.to_string())?;
            file.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
        } else {
            std::fs::write(&path, content).map_err(|e| e.to_string())?;
        }
        let mut out = json!({ "path": path.display().to_string(), "written": content.chars().count(), "appended": append });
        if let Some(note) = note {
            out["note"] = json!(note);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{Confirmer, DenyAll};
    use std::path::PathBuf;

    struct AllowAll;
    #[async_trait]
    impl Confirmer for AllowAll {
        async fn confirm(&self, _: &str, _: &str) -> bool {
            true
        }
    }

    fn sandbox(name: &str) -> Arc<UserFolders> {
        let home = std::env::temp_dir().join(format!("anytask-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let desktop = home.join("Desktop");
        std::fs::create_dir_all(&desktop).unwrap();
        Arc::new(UserFolders {
            desktop,
            downloads: home.join("Downloads"),
            documents: home.join("Documents"),
            pictures: home.join("Pictures"),
            music: home.join("Music"),
            videos: home.join("Videos"),
            home: PathBuf::from(&home),
        })
    }

    fn ctx(confirm: Arc<dyn Confirmer>) -> ToolCtx {
        crate::tools::test_support::ctx(confirm)
    }

    #[tokio::test]
    async fn write_read_list_and_overwrite_needs_permission() {
        let folders = sandbox("rw");
        let write = WriteTextFile { folders: folders.clone(), here: None };
        let allow = ctx(Arc::new(AllowAll));
        let deny = ctx(Arc::new(DenyAll));

        write.run(json!({ "path": "Desktop/prices.txt", "content": "gold: 1\n" }), &deny).await.unwrap();
        let refused = write.run(json!({ "path": "Desktop/prices.txt", "content": "dollar: 2\n", "append": true }), &deny).await.unwrap_err();
        assert!(refused.contains("did not allow"), "adding to an existing file asks too");
        write.run(json!({ "path": "Desktop/prices.txt", "content": "dollar: 2\n", "append": true }), &allow).await.unwrap();
        let err = write.run(json!({ "path": "Desktop/prices.txt", "content": "x" }), &deny).await.unwrap_err();
        assert!(err.contains("did not allow"));

        let read = ReadTextFile { folders: folders.clone() }.run(json!({ "path": "Desktop/prices.txt" }), &allow).await.unwrap();
        assert_eq!(read["content"], "gold: 1\ndollar: 2\n");

        let list = ListDirectory { folders: folders.clone() }.run(json!({ "path": "Desktop" }), &allow).await.unwrap();
        assert_eq!(list["entries"][0]["name"], "prices.txt");
    }

    #[tokio::test]
    async fn here_is_the_open_folder_or_the_desktop() {
        let folders = sandbox("here");
        let open = folders.downloads.join("lists");
        let found = open.clone();
        let write = WriteTextFile { folders: folders.clone(), here: Some(Arc::new(move || Some(found.clone()))) };
        let c = ctx(Arc::new(DenyAll));
        let out = write.run(json!({ "path": "here/prices.txt", "content": "a" }), &c).await.unwrap();
        assert_eq!(out["path"], json!(open.join("prices.txt").display().to_string()));
        assert!(out.get("note").is_none());
        let none = WriteTextFile { folders: folders.clone(), here: Some(Arc::new(|| None)) };
        let out = none.run(json!({ "path": "Inja/prices.txt", "content": "a" }), &c).await.unwrap();
        assert_eq!(out["path"], json!(folders.desktop.join("prices.txt").display().to_string()));
        assert!(out["note"].as_str().unwrap().contains("Desktop"));
        assert_eq!(here_name("here.txt"), None);
        assert_eq!(here_name("اینجا/لیست.txt"), Some("لیست.txt"));
    }

    #[tokio::test]
    async fn refuses_bad_targets() {
        let folders = sandbox("bad");
        let write = WriteTextFile { folders: folders.clone(), here: None };
        let c = ctx(Arc::new(AllowAll));
        assert!(write.run(json!({ "path": "Desktop/app.exe", "content": "x" }), &c).await.unwrap_err().contains(".exe"));
        assert!(write.run(json!({ "path": "Desktop/../.ssh/x.txt", "content": "x" }), &c).await.unwrap_err().contains("hidden"));
        let script = write.run(json!({ "path": "Desktop/run.bat", "content": "echo" }), &ctx(Arc::new(DenyAll))).await.unwrap_err();
        assert!(script.contains("did not allow"));
    }

    /// Creates a file (and its folders) changed `hours_ago` hours ago.
    fn touch(path: &Path, hours_ago: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
        let when = SystemTime::now() - Duration::from_secs(hours_ago * 3600);
        let times = std::fs::FileTimes::new().set_modified(when);
        // The search goes by the later of modified and created, so both are set.
        #[cfg(windows)]
        let times = std::os::windows::fs::FileTimesExt::set_created(times, when);
        std::fs::File::options().write(true).open(path).unwrap().set_times(times).unwrap();
    }

    #[tokio::test]
    async fn folders_copied_files_and_shortcuts() {
        let f = sandbox("find-kinds");
        touch(&f.pictures.join("سفر شمال").join("1.jpg"), 1);
        touch(&f.desktop.join("سفر شمال.lnk"), 1);
        let trip = find(&f, json!({ "name": "شمال" })).await.unwrap();
        assert_eq!(trip["matches"].as_array().unwrap().len(), 1, "the folder, not the shortcut: {trip}");
        assert_eq!(trip["matches"][0]["type"], "folder");
        assert!(find(&f, json!({ "name": "شمال", "ext": "jpg" })).await.unwrap()["matches"].as_array().unwrap().is_empty(), "no folders when a type is asked for");

        // Copied from a phone a moment ago, but last changed a year ago: it arrived today.
        let copied = f.downloads.join("scan.pdf");
        touch(&copied, 0);
        let old = std::fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(365 * 86_400));
        std::fs::File::options().write(true).open(&copied).unwrap().set_times(old).unwrap();
        assert_eq!(names(&find(&f, json!({ "name": "*", "ext": "pdf", "modified_within_days": 1 })).await.unwrap()), ["scan.pdf"]);
    }

    async fn find(folders: &Arc<UserFolders>, args: Value) -> Result<Value, String> {
        FindFiles { folders: folders.clone() }.run(args, &ctx(Arc::new(DenyAll))).await
    }

    /// The file names of the matches, in order.
    fn names(result: &Value) -> Vec<String> {
        result["matches"].as_array().unwrap().iter().map(|m| Path::new(m["path"].as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[tokio::test]
    async fn finds_persian_names_however_they_are_typed() {
        let f = sandbox("find-fa");
        touch(&f.documents.join("کار").join("گزارش ماهانه.pdf"), 1);
        touch(&f.desktop.join("فاکتور ۱۴۰۵.xlsx"), 2);
        touch(&f.downloads.join("invoice 1405.txt"), 3);
        touch(&f.downloads.join("گزارش مالي.docx"), 4);
        touch(&f.desktop.join("نامه\u{200c}ها.txt"), 5);
        touch(&f.desktop.join("unrelated.txt"), 6);

        let report = find(&f, json!({ "name": "گزارش" })).await.unwrap();
        assert_eq!(names(&report), ["گزارش ماهانه.pdf", "گزارش مالي.docx"]);
        assert_eq!(report["matches"][0]["path"], f.documents.join("کار").join("گزارش ماهانه.pdf").display().to_string());
        assert_eq!(report["matches"][0]["size"], "1 B");
        assert_eq!(report["truncated"], false);
        assert!(report["searched"].as_u64().unwrap() >= 4, "{report}");

        assert_eq!(names(&find(&f, json!({ "name": "فاکتور 1405" })).await.unwrap()), ["فاکتور ۱۴۰۵.xlsx"], "Persian digits in the file");
        assert_eq!(names(&find(&f, json!({ "name": "۱۴۰۵" })).await.unwrap()), ["فاکتور ۱۴۰۵.xlsx", "invoice 1405.txt"], "Persian digits in the query");
        assert_eq!(names(&find(&f, json!({ "name": "مالی" })).await.unwrap()), ["گزارش مالي.docx"], "Arabic yeh");
        assert_eq!(names(&find(&f, json!({ "name": "نامهها" })).await.unwrap()), ["نامه\u{200c}ها.txt"], "half-space");
        assert_eq!(names(&find(&f, json!({ "name": "گزارش ماهانه" })).await.unwrap()), ["گزارش ماهانه.pdf"], "every word must match");
    }

    #[tokio::test]
    async fn type_age_and_folder_narrow_the_search() {
        let f = sandbox("find-filter");
        touch(&f.downloads.join("report.pdf"), 20);
        touch(&f.downloads.join("report.docx"), 1);
        touch(&f.documents.join("old report.pdf"), 24 * 10);
        touch(&f.desktop.join("Report.PDF"), 30);

        assert_eq!(names(&find(&f, json!({ "name": "report", "ext": "pdf" })).await.unwrap()), ["report.pdf", "Report.PDF", "old report.pdf"]);
        assert_eq!(names(&find(&f, json!({ "name": "report", "ext": ".PDF" })).await.unwrap()).len(), 3);
        assert_eq!(names(&find(&f, json!({ "name": "report", "ext": "docx,doc" })).await.unwrap()), ["report.docx"]);
        assert_eq!(names(&find(&f, json!({ "name": "report.pdf" })).await.unwrap()), ["report.pdf", "Report.PDF", "old report.pdf"]);

        let recent = find(&f, json!({ "name": "*", "ext": "pdf", "modified_within_days": 2 })).await.unwrap();
        assert_eq!(names(&recent), ["report.pdf", "Report.PDF"], "the 10-day-old file is left out");
        assert_eq!(names(&find(&f, json!({ "name": "report", "modified_within_days": "۱" })).await.unwrap()), ["report.docx", "report.pdf"]);

        assert_eq!(names(&find(&f, json!({ "name": "report", "folder": "Downloads" })).await.unwrap()), ["report.docx", "report.pdf"]);
        assert!(find(&f, json!({ "name": "report", "folder": "Downloads/none" })).await.unwrap_err().contains("no such folder"));
        assert!(find(&f, json!({ "name": "  " })).await.unwrap_err().contains("name is required"));
        assert!(!names(&find(&f, json!({ "name": "*" })).await.unwrap()).is_empty(), "* alone: the newest files of any kind");
    }

    #[tokio::test]
    async fn newest_twenty_come_back_and_no_match_says_where_it_looked() {
        let f = sandbox("find-many");
        for i in 0..25 {
            touch(&f.documents.join(format!("note {i}.txt")), i);
        }
        let notes = find(&f, json!({ "name": "note" })).await.unwrap();
        let got = names(&notes);
        assert_eq!(got.len(), 20);
        assert_eq!(got[0], "note 0.txt");
        assert_eq!(got[19], "note 19.txt");
        assert_eq!(notes["found"], 25);
        let times: Vec<&str> = notes["matches"].as_array().unwrap().iter().map(|m| m["modified"].as_str().unwrap()).collect();
        assert!(times.windows(2).all(|w| w[0] >= w[1]), "newest first: {times:?}");
        assert_eq!(times[0].len(), "2026-11-05 10:30".len());

        let none = find(&f, json!({ "name": "budget" })).await.unwrap();
        assert_eq!(none["matches"], json!([]));
        let note = none["note"].as_str().unwrap();
        assert!(note.contains("No file matched") && note.contains(&f.documents.display().to_string()) && note.contains(&f.desktop.display().to_string()), "{note}");
        assert!(!note.contains(&f.music.display().to_string()), "folders that do not exist are not searched: {note}");
    }

    #[tokio::test]
    async fn program_folders_hidden_folders_and_deep_folders_are_skipped() {
        let f = sandbox("find-skip");
        touch(&f.documents.join("app").join("report.txt"), 1);
        touch(&f.documents.join("app").join("node_modules").join("pkg").join("report.js"), 1);
        touch(&f.documents.join("app").join(".git").join("report"), 1);
        touch(&f.documents.join(".cache").join("report.tmp"), 1);
        touch(&f.documents.join("a/b/c/d/report 5.txt"), 2);
        touch(&f.documents.join("a/b/c/d/e/f/report 7.txt"), 1);
        assert_eq!(names(&find(&f, json!({ "name": "report" })).await.unwrap()), ["report.txt", "report 5.txt"]);
    }

    #[tokio::test]
    async fn blocked_places_stay_blocked() {
        let f = sandbox("find-blocked");
        touch(&f.desktop.join("secret notes.txt"), 1);
        touch(&f.home.join("AppData").join("Roaming").join("secret.txt"), 1);
        touch(&f.home.join(".ssh").join("secret"), 1);
        assert!(find(&f, json!({ "name": "secret", "folder": "~/.ssh" })).await.unwrap_err().contains("Access denied"));
        assert!(find(&f, json!({ "name": "secret", "folder": "~/AppData/Roaming" })).await.unwrap_err().contains("Access denied"));
        assert!(find(&f, json!({ "name": "secret", "folder": "Desktop/../.ssh" })).await.unwrap_err().contains("Access denied"));
        #[cfg(windows)]
        assert!(find(&f, json!({ "name": "secret", "folder": "C:\\Windows" })).await.unwrap_err().contains("Access denied"));
        let home = find(&f, json!({ "name": "secret", "folder": "home" })).await.unwrap();
        assert_eq!(names(&home), ["secret notes.txt"], "walking home still skips AppData and dot folders");
    }

    #[test]
    fn a_cut_or_stopped_search_says_so() {
        let f = sandbox("find-budget");
        for i in 0..10 {
            touch(&f.desktop.join(format!("photo {i}.jpg")), i);
        }
        let query = Query::parse(&json!({ "name": "photo" }), SystemTime::now()).unwrap();
        let roots = search_roots(&f);
        let small = Budget { time: Duration::from_secs(3), entries: 4 };
        let cut = search(&f, &roots, &query, &CancellationToken::new(), small).unwrap();
        assert!(cut.truncated);
        assert_eq!(cut.hits.len(), 4);
        let whole = search(&f, &roots, &query, &CancellationToken::new(), FIND_BUDGET).unwrap();
        assert!(!whole.truncated);
        assert_eq!(whole.hits.len(), 10);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(search(&f, &roots, &query, &cancel, FIND_BUDGET).err().as_deref(), Some("Stopped"));
    }

    #[test]
    fn sizes_read_like_a_person_says_them() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(25 * 1024 * 1024), "25 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }
}
