//! "Take a screenshot (and send it to my phone)": the whole screen at full size, saved in Pictures\Screenshots, and
//! when asked sent to the paired phone at once. There was no tool for it: the model had nothing to take a picture with.

use crate::island::{ISLAND, IslandState};
use crate::phone::Phone;
use anytask_core::lang::tr;
use anytask_core::{Tool, ToolCtx};
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager};

const QUALITY: u8 = 90;

pub struct Screenshot {
    pub app: AppHandle,
    pub pictures: PathBuf,
}

impl Screenshot {
    /// Leaves the island out of the picture (it floats over every app and would cover what the user wants to show),
    /// or puts it back the way the screen agent left it.
    fn hide_island(&self, hide: bool) {
        let working = self.app.try_state::<IslandState>().is_some_and(|s| s.agent_working.load(Ordering::Relaxed));
        if let Some(window) = self.app.get_webview_window(ISLAND)
            && let Ok(hwnd) = window.hwnd()
        {
            let _ = anytask_control::exclude_from_capture(hwnd.0 as isize, hide || working);
        }
    }
}

#[async_trait]
impl Tool for Screenshot {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "screenshot".into(),
            description: "Takes a screenshot of the whole screen now (the assistant's island left out) and saves it as a picture in \
                          Pictures/Screenshots. With send_to_phone it also sends it to the user's phone (Downloads/AnyTask). Returns the \
                          file's path. Use it for \"aks az safe begir\", \"screenshot begir\", \"befrest baraye gooshim\"."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": { "send_to_phone": { "type": "boolean", "description": "Also send it to the user's phone." } }
            }),
        }
    }

    fn label(&self, args: &Value) -> String {
        if args["send_to_phone"].as_bool() == Some(true) {
            tr("Screenshot to the phone", "عکس صفحه برای گوشی").into()
        } else {
            tr("Taking a screenshot", "گرفتن عکس از صفحه").into()
        }
    }

    async fn run(&self, args: Value, ctx: &ToolCtx) -> Result<Value, String> {
        self.hide_island(true);
        // The window manager draws the island's new state on its next frame.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        let shot = tokio::task::spawn_blocking(|| {
            let (image, _) = anytask_control::screen::capture_primary(u32::MAX)?;
            anytask_control::marks::encode_jpeg(&image, QUALITY)
        })
        .await
        .map_err(|e| format!("screenshot worker stopped: {e}"));
        self.hide_island(false);
        let jpeg = shot??;
        let folder = self.pictures.join("Screenshots");
        std::fs::create_dir_all(&folder).map_err(|e| format!("Could not make {}: {e}", folder.display()))?;
        let path = folder.join(format!("AnyTask {}.jpg", chrono::Local::now().format("%Y-%m-%d %H-%M-%S")));
        std::fs::write(&path, &jpeg).map_err(|e| format!("Could not save {}: {e}", path.display()))?;
        let shown = path.display().to_string();
        (ctx.log)(format!("screenshot: {} KB -> {shown}", jpeg.len() / 1024));
        let mut out = json!({ "saved": shown });
        if args["send_to_phone"].as_bool() == Some(true) {
            match self.app.try_state::<Arc<Phone>>() {
                Some(phone) => match phone.offer_file(&shown).await {
                    Ok(sent) => out["phone"] = sent,
                    Err(why) => out["phone_error"] = json!(format!("{why} The picture is still saved on the PC.")),
                },
                None => out["phone_error"] = json!("No phone is paired (Settings > Phone). The picture is saved on the PC."),
            }
        }
        Ok(out)
    }
}
