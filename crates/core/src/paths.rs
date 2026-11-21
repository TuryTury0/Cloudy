//! Where the assistant may read and write. Ported from v1 (filesystem.js + main.js path policy):
//! - names like "Desktop", "Downloads", "دسکتاپ" and relative paths resolve inside the user's folders,
//! - system folders, hidden configuration folders and AppData are off limits,
//! - the root of a known folder (the whole Desktop) cannot be overwritten or deleted.

use std::path::{Component, Path, PathBuf};

const ALIASES: &[(&str, Folder)] = &[
    ("desktop", Folder::Desktop),
    ("دسکتاپ", Folder::Desktop),
    ("صفحه دسکتاپ", Folder::Desktop),
    ("downloads", Folder::Downloads),
    ("download", Folder::Downloads),
    ("دانلود", Folder::Downloads),
    ("دانلودها", Folder::Downloads),
    ("documents", Folder::Documents),
    ("document", Folder::Documents),
    ("docs", Folder::Documents),
    ("اسناد", Folder::Documents),
    ("pictures", Folder::Pictures),
    ("photos", Folder::Pictures),
    ("تصاویر", Folder::Pictures),
    ("music", Folder::Music),
    ("موزیک", Folder::Music),
    ("videos", Folder::Videos),
    ("movies", Folder::Videos),
    ("ویدیوها", Folder::Videos),
    ("home", Folder::Home),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Desktop,
    Downloads,
    Documents,
    Pictures,
    Music,
    Videos,
    Home,
}

/// The user's real folders (OneDrive redirection included, via the OS).
#[derive(Debug, Clone)]
pub struct UserFolders {
    pub home: PathBuf,
    pub desktop: PathBuf,
    pub downloads: PathBuf,
    pub documents: PathBuf,
    pub pictures: PathBuf,
    pub music: PathBuf,
    pub videos: PathBuf,
}

impl UserFolders {
    pub fn detect() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let or = |p: Option<PathBuf>, name: &str| p.unwrap_or_else(|| home.join(name));
        Self {
            desktop: or(dirs::desktop_dir(), "Desktop"),
            downloads: or(dirs::download_dir(), "Downloads"),
            documents: or(dirs::document_dir(), "Documents"),
            pictures: or(dirs::picture_dir(), "Pictures"),
            music: or(dirs::audio_dir(), "Music"),
            videos: or(dirs::video_dir(), "Videos"),
            home,
        }
    }

    fn folder(&self, f: Folder) -> &Path {
        match f {
            Folder::Desktop => &self.desktop,
            Folder::Downloads => &self.downloads,
            Folder::Documents => &self.documents,
            Folder::Pictures => &self.pictures,
            Folder::Music => &self.music,
            Folder::Videos => &self.videos,
            Folder::Home => &self.home,
        }
    }

    pub(crate) fn roots(&self) -> [&Path; 6] {
        [&self.desktop, &self.downloads, &self.documents, &self.pictures, &self.music, &self.videos]
    }

    /// Resolves what the user said ("Desktop/notes.txt", "دانلود", "C:\\x", "report.txt") to an absolute path.
    pub fn resolve(&self, input: &str) -> PathBuf {
        let raw = input.trim().trim_matches(|c| c == '"' || c == '\'');
        if raw.is_empty() {
            return self.desktop.clone();
        }
        let slashed = raw.replace('\\', "/");
        let lower = slashed.to_lowercase();
        let (head, rest) = match lower.find('/') {
            Some(i) => (&lower[..i], Some(&slashed[i + 1..])),
            None => (lower.as_str(), None),
        };
        if let Some((_, folder)) = ALIASES.iter().find(|(name, _)| *name == head) {
            let base = self.folder(*folder).to_path_buf();
            return normalize(&rest.map(|r| base.join(r)).unwrap_or(base));
        }
        if let Some(rest) = slashed.strip_prefix("~/") {
            return normalize(&self.home.join(rest));
        }
        let path = Path::new(raw);
        if path.is_absolute() {
            return normalize(path);
        }
        normalize(&self.desktop.join(raw))
    }

    /// Why a path may not be used, or None when it is fine. `whole_folder_ok` allows reading/listing a root
    /// such as the Desktop itself. Windows paths are compared without regard to case ("C:\USERS\x\AppData" is still
    /// AppData too), and a path that exists is checked again as the place it really leads to (a junction).
    pub fn blocked(&self, path: &Path, whole_folder_ok: bool) -> Option<&'static str> {
        let p = normalize(path);
        if let Some(why) = self.blocked_as_written(&p, whole_folder_ok) {
            return Some(why);
        }
        match std::fs::canonicalize(&p) {
            Ok(real) if real != p => self.blocked_as_written(&real, whole_folder_ok),
            _ => None,
        }
    }

    fn blocked_as_written(&self, p: &Path, whole_folder_ok: bool) -> Option<&'static str> {
        // "C:notes" names a place relative to the working folder of the program (System32 when it starts with
        // Windows): never a user folder.
        if matches!(p.components().next(), Some(Component::Prefix(_))) && !p.has_root() {
            return Some("a drive without a folder path is off limits");
        }
        let text = folded(p);
        if text.len() <= 3 || text.ends_with(':') || p.parent().is_none() {
            return Some("a drive root is off limits");
        }
        let system_roots: Vec<String> = [std::env::var("SystemRoot").ok(), std::env::var("ProgramFiles").ok(), std::env::var("ProgramFiles(x86)").ok(), std::env::var("ProgramData").ok()]
            .into_iter()
            .flatten()
            .chain(["c:\\windows".to_string(), "/etc".into(), "/usr".into(), "/bin".into(), "/system".into()])
            .map(|s| s.to_lowercase())
            .collect();
        if system_roots.iter().any(|root| text == *root || text.starts_with(&format!("{root}\\")) || text.starts_with(&format!("{root}/"))) {
            return Some("system folders are off limits");
        }
        let home = folded(&self.home);
        if text == home && !whole_folder_ok {
            return Some("the home folder itself cannot be changed");
        }
        let under_home = text.strip_prefix(&format!("{home}\\")).or_else(|| text.strip_prefix(&format!("{home}/")));
        if let Some(rel) = under_home {
            let first = rel.split(['\\', '/']).next().unwrap_or_default();
            if first.starts_with('.') || first == "appdata" || first == "library" {
                return Some("hidden configuration folders are off limits");
            }
        }
        if !whole_folder_ok && self.roots().iter().any(|root| folded(root) == text) {
            return Some("a whole user folder cannot be changed");
        }
        None
    }
}

/// A path as lower-case text with backslashes and without the \\?\ prefix, for comparing places the way Windows
/// does (its paths ignore case).
fn folded(path: &Path) -> String {
    let text = normalize(path).to_string_lossy().to_lowercase();
    let text = if cfg!(windows) { text.replace('/', "\\") } else { text };
    let text = text.strip_prefix(r"\\?\unc\").map(|rest| format!(r"\\{rest}")).unwrap_or(text);
    let text = text.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(text);
    text.trim_end_matches(['\\', '/']).to_string()
}

/// Lexical normalisation (no filesystem access): removes "." and resolves "..".
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> UserFolders {
        let home = PathBuf::from(if cfg!(windows) { "C:\\Users\\ermia" } else { "/home/ermia" });
        UserFolders {
            desktop: home.join("OneDrive").join("Desktop"),
            downloads: home.join("Downloads"),
            documents: home.join("Documents"),
            pictures: home.join("Pictures"),
            music: home.join("Music"),
            videos: home.join("Videos"),
            home,
        }
    }

    #[test]
    fn aliases_and_relative_paths() {
        let f = folders();
        assert_eq!(f.resolve("Desktop/notes.txt"), f.desktop.join("notes.txt"));
        assert_eq!(f.resolve("دانلود"), f.downloads);
        assert_eq!(f.resolve("report.txt"), f.desktop.join("report.txt"));
        assert_eq!(f.resolve("Documents\\a\\..\\b.txt"), f.documents.join("b.txt"));
        assert_eq!(f.resolve(""), f.desktop);
    }

    #[test]
    fn traversal_cannot_escape_into_hidden_folders() {
        let f = folders();
        let sneaky = f.resolve("Desktop/../../.ssh/id_rsa");
        assert_eq!(f.blocked(&sneaky, true), Some("hidden configuration folders are off limits"));
        assert!(f.blocked(&f.home.join("AppData").join("x"), true).is_some());
    }

    #[test]
    #[cfg(windows)]
    fn letter_case_and_path_forms_do_not_open_blocked_places() {
        let f = folders();
        let upper = |rest: &str| PathBuf::from(format!(r"C:\USERS\ERMIA\{rest}"));
        assert_eq!(f.blocked(&upper(r"AppData\Roaming"), true), Some("hidden configuration folders are off limits"));
        assert_eq!(f.blocked(&upper(r".ssh\id_rsa"), true), Some("hidden configuration folders are off limits"));
        assert!(f.blocked(Path::new(r"\\?\C:\Users\ermia\AppData\Local"), true).is_some(), r"the \\?\ form too");
        assert!(f.blocked(Path::new(r"c:\windows\system32"), true).is_some());
        assert_eq!(f.blocked(Path::new("C:notes"), true), Some("a drive without a folder path is off limits"));
        assert!(f.blocked(&upper(r"Documents\a.txt"), false).is_none(), "ordinary files stay usable in any case");
        assert!(f.blocked(&PathBuf::from(r"C:\USERS\ERMIA\DOWNLOADS"), false).is_some(), "a whole user folder, whatever the case");
    }

    #[test]
    fn roots_are_protected_from_changes_but_readable() {
        let f = folders();
        assert!(f.blocked(&f.desktop, false).is_some());
        assert!(f.blocked(&f.desktop, true).is_none());
        assert!(f.blocked(&f.desktop.join("a.txt"), false).is_none());
    }

    #[test]
    #[cfg(windows)]
    fn system_folders_and_drive_roots_are_blocked() {
        let f = folders();
        assert!(f.blocked(Path::new("C:\\Windows\\System32\\x.dll"), true).is_some());
        assert!(f.blocked(Path::new("C:\\"), true).is_some());
    }
}
