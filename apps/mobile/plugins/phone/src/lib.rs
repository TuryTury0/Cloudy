//! Phone access for the paired PC's assistant: battery and device, recent notifications, SMS inbox, launching
//! apps and links. The Android side is in android/ (Kotlin); elsewhere (desktop development) every call says it
//! only works on Android.
//!
//! Commands (Kotlin method names): status, notifications {limit}, messages {limit}, apps, openApp {name},
//! openUrl {url}, openNotificationAccess, requestPermissions, lastCrash, pickFile, saveFile {path, name},
//! openFile {uri}, ring, stopRing, listen {lang}, setAlarm {hour, minute, label}, setTimer {seconds, label},
//! dial {number}, composeSms {number, text}, navigate {destination}. Opening another app works only while AnyTask
//! is on the phone's screen (Android blocks it from the background).

use serde_json::Value;
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime};

pub struct Phone<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> Phone<R> {
    /// Runs one command on the phone; errors are the reason in plain words.
    pub async fn call(&self, command: &str, args: Value) -> Result<Value, String> {
        #[cfg(target_os = "android")]
        {
            self.handle.run_mobile_plugin_async::<Value>(command, args).await.map_err(|e| e.to_string())
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (command, args);
            Err("Phone access works only in the Android app.".into())
        }
    }
}

pub trait PhoneExt<R: Runtime> {
    fn phone(&self) -> &Phone<R>;
}

impl<R: Runtime, T: Manager<R>> PhoneExt<R> for T {
    fn phone(&self) -> &Phone<R> {
        self.state::<Phone<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("anytask-phone")
        .setup(|app, api| {
            #[cfg(target_os = "android")]
            let phone = Phone { handle: api.register_android_plugin("com.anytask.phone", "PhonePlugin")? };
            #[cfg(not(target_os = "android"))]
            let phone = {
                let _ = api;
                Phone::<R> { _runtime: std::marker::PhantomData }
            };
            app.manage(phone);
            Ok(())
        })
        .build()
}
