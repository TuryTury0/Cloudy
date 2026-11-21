//! Updates from the public GitHub repo (github.com/TuryTury0/AnyTask): each release holds the installer, its
//! signature and a latest.json that names them (scripts/release.mjs writes all three). A check runs a minute after
//! start and then every 6 hours. With "Update by itself" on (the default), a new version is downloaded at once and
//! installed as soon as no job is running: the installer shows a progress bar and opens AnyTask again. The download
//! is checked against the public key in tauri.conf.json, so only a release signed with our private key installs.
//! A check that fails straight out (GitHub blocked or slow) is tried once more through the proxy, when web pages may
//! use it.

use crate::brain::Brain;
use crate::commands::SettingsStore;
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Url};
use tauri_plugin_updater::{Update, UpdaterExt};

const FIRST_CHECK: Duration = Duration::from_secs(60);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How often a ready update looks again whether the assistant is free.
const IDLE_POLL: Duration = Duration::from_secs(30);
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// What the About page shows. `state` is one of: idle, checking, latest, downloading, ready, installing, failed.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub state: String,
    pub current: String,
    /// The new version, once one is found.
    pub version: Option<String>,
    pub notes: Option<String>,
    /// Download progress, 0-100, when the size is known.
    pub progress: Option<u8>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<Option<UpdateStatus>>,
    /// A downloaded update waiting to be installed.
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    /// One check or download at a time (the timer and the button share it).
    running: tokio::sync::Mutex<()>,
}

fn status(app: &AppHandle) -> UpdateStatus {
    let current = app.package_info().version.to_string();
    let updates = app.state::<Updates>();
    let known = updates.status.lock().ok().and_then(|s| s.clone());
    known.unwrap_or(UpdateStatus { state: "idle".into(), current, version: None, notes: None, progress: None, error: None })
}

fn set(app: &AppHandle, change: impl FnOnce(&mut UpdateStatus)) -> UpdateStatus {
    let mut next = status(app);
    change(&mut next);
    if let Ok(mut s) = app.state::<Updates>().status.lock() {
        *s = Some(next.clone());
    }
    let _ = app.emit("update-status", &next);
    next
}

fn log(app: &AppHandle, text: String) {
    if let Some(brain) = app.try_state::<Brain>() {
        brain.agent.events().emit(anytask_core::CoreEvent::Log { turn_id: None, source: "update".into(), text });
    }
}

fn auto(app: &AppHandle) -> bool {
    app.try_state::<SettingsStore>().and_then(|s| s.settings.lock().ok().map(|s| s.auto_update)).unwrap_or(true)
}

/// The proxy a second try may use: only when web pages are allowed to go through it.
fn proxy(app: &AppHandle) -> Option<Url> {
    let store = app.try_state::<SettingsStore>()?;
    let settings = store.settings.lock().ok()?;
    settings.proxy.url().filter(|_| settings.proxy.web).and_then(|u| Url::parse(&u).ok())
}

async fn find(app: &AppHandle, through: Option<Url>) -> Result<Option<Update>, String> {
    let mut builder = app.updater_builder().timeout(CHECK_TIMEOUT);
    if let Some(url) = through {
        builder = builder.proxy(url);
    }
    let updater = builder.build().map_err(|e| e.to_string())?;
    updater.check().await.map_err(|e| e.to_string())
}

async fn download(app: &AppHandle, update: &Update) -> Result<Vec<u8>, String> {
    let mut got = 0usize;
    let mut last = None;
    let progress = |chunk: usize, total: Option<u64>| {
        got += chunk;
        let pct = total.filter(|t| *t > 0).map(|t| ((got as u64 * 100) / t).min(100) as u8);
        if pct != last {
            last = pct;
            set(app, |s| s.progress = pct);
        }
    };
    update.download(progress, || {}).await.map_err(|e| e.to_string())
}

/// Looks for a newer version and downloads it. Returns the status after it.
pub async fn check(app: &AppHandle) -> UpdateStatus {
    let updates = app.state::<Updates>();
    let Ok(_one) = updates.running.try_lock() else {
        return status(app);
    };
    if updates.ready.lock().map(|r| r.is_some()).unwrap_or(false) {
        return status(app);
    }
    set(app, |s| {
        s.state = "checking".into();
        s.error = None;
        s.progress = None;
    });
    let found = match find(app, None).await {
        Ok(found) => Ok(found),
        Err(first) => match proxy(app) {
            Some(url) => find(app, Some(url)).await.map_err(|second| format!("{first}; through the proxy: {second}")),
            None => Err(first),
        },
    };
    let update = match found {
        Ok(Some(update)) => update,
        Ok(None) => return set(app, |s| s.state = "latest".into()),
        Err(e) => {
            log(app, format!("update check failed: {e}"));
            return set(app, |s| {
                s.state = "failed".into();
                s.error = Some(e);
            });
        }
    };
    log(app, format!("version {} found, downloading", update.version));
    set(app, |s| {
        s.state = "downloading".into();
        s.version = Some(update.version.clone());
        s.notes = update.body.clone();
    });
    match download(app, &update).await {
        Ok(bytes) => {
            log(app, format!("version {} downloaded ({} KB)", update.version, bytes.len() / 1024));
            if let Ok(mut ready) = updates.ready.lock() {
                *ready = Some((update, bytes));
            }
            set(app, |s| {
                s.state = "ready".into();
                s.progress = Some(100);
            })
        }
        Err(e) => {
            log(app, format!("update download failed: {e}"));
            set(app, |s| {
                s.state = "failed".into();
                s.error = Some(e);
            })
        }
    }
}

/// Runs the downloaded installer. On Windows the app closes here and the installer opens it again when done.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let ready = app.state::<Updates>().ready.lock().ok().and_then(|mut r| r.take());
    let Some((update, bytes)) = ready else {
        return Err("No update is downloaded yet.".into());
    };
    log(app, format!("installing version {}", update.version));
    set(app, |s| s.state = "installing".into());
    // Give the windows a moment to show "installing" before the app closes.
    tokio::time::sleep(Duration::from_millis(400)).await;
    update.install(&bytes).map_err(|e| {
        let e = e.to_string();
        set(app, |s| {
            s.state = "failed".into();
            s.error = Some(e.clone());
        });
        e
    })
}

/// Installs a ready update once the assistant is free (no job running), while "Update by itself" stays on.
async fn install_when_free(app: &AppHandle) {
    loop {
        let has = app.state::<Updates>().ready.lock().map(|r| r.is_some()).unwrap_or(false);
        if !has || !auto(app) {
            return;
        }
        let busy = app.try_state::<Brain>().map(|b| b.agent.is_busy()).unwrap_or(true);
        if !busy {
            let _ = install(app).await;
            return;
        }
        tokio::time::sleep(IDLE_POLL).await;
    }
}

/// True for a program run from a build folder (target\app, target\release): the test app, not an installed one.
fn built_here(exe: &std::path::Path) -> bool {
    exe.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target"))
}

/// The background timer. Not in debug builds or the test app in target\: a developer's run must never install the
/// released app by itself (the button in Settings > About still can).
pub fn start(app: &AppHandle) {
    app.manage(Updates::default());
    let test_app = std::env::current_exe().map(|exe| built_here(&exe)).unwrap_or(true);
    if cfg!(debug_assertions) || test_app {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if auto(&app) {
                check(&app).await;
                install_when_free(&app).await;
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> UpdateStatus {
    status(&app)
}

/// "Check for updates": finds and downloads a newer version; with "Update by itself" on it is installed when free.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> UpdateStatus {
    let after = check(&app).await;
    if after.state == "ready" && auto(&app) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { install_when_free(&app).await });
    }
    after
}

/// "Install now": the user asked, so it does not wait for a running job.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    install(&app).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn only_an_installed_app_updates_by_itself() {
        assert!(built_here(Path::new(r"E:\all2\AnyTask - Copy\anytask-next\target\app\anytask-desktop.exe")));
        assert!(built_here(Path::new(r"E:\code\Target\release\anytask-desktop.exe")), "whatever the case");
        assert!(!built_here(Path::new(r"C:\Users\x\AppData\Local\AnyTask\anytask-desktop.exe")));
        assert!(!built_here(Path::new(r"C:\Users\x\targets\AnyTask\anytask-desktop.exe")), "a whole folder name only");
    }
}
