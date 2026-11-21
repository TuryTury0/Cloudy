//! The connected phone as tools for the assistant: its notifications and SMS, battery, and opening apps or links
//! there. Each call goes to the phone over the paired channel and waits for its answer. The tools exist only while
//! a phone is connected.

use crate::phone::Phone;
use anytask_core::{Tool, ToolCtx};
use anytask_llm::ToolSpec;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

/// What the step is called on the island: (English, Persian).
type Label = (&'static str, &'static str);

/// Sends a file from this PC to the phone (it lands in the phone's Downloads/AnyTask).
struct SendFile {
    phone: Arc<Phone>,
}

#[async_trait]
impl Tool for SendFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "send_file_to_phone".into(),
            description: "Sends a file from this PC to the user's phone; it is saved in the phone's Downloads/AnyTask folder. Give the full path (find it first with find_files if needed).".into(),
            parameters: json!({ "type": "object", "properties": { "path": { "type": "string", "description": "Full path of the file on this PC." } }, "required": ["path"] }),
        }
    }

    fn label(&self, _args: &Value) -> String {
        anytask_core::lang::tr("Sending a file to the phone", "فرستادن فایل به گوشی").into()
    }

    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        self.phone.offer_file(args["path"].as_str().unwrap_or_default().trim()).await
    }
}

struct PhoneTool {
    phone: Arc<Phone>,
    name: &'static str,
    /// The command on the phone (Kotlin method name).
    command: &'static str,
    description: &'static str,
    parameters: Value,
    label: Label,
}

#[async_trait]
impl Tool for PhoneTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: self.name.into(), description: self.description.into(), parameters: self.parameters.clone() }
    }

    fn label(&self, _args: &Value) -> String {
        anytask_core::lang::tr(self.label.0, self.label.1).into()
    }

    async fn run(&self, args: Value, _ctx: &ToolCtx) -> Result<Value, String> {
        self.phone.call_device(self.command, args).await
    }
}

pub fn tools(phone: Arc<Phone>) -> Vec<Arc<dyn Tool>> {
    let none = || json!({ "type": "object", "properties": {} });
    let limit = || json!({ "type": "object", "properties": { "limit": { "type": "integer", "description": "How many, newest first (default 20)." } } });
    let list: [(&str, &str, &str, Value, Label); 13] = [
        ("phone_status", "status", "The user's phone: battery and charging, model, and which access it allows (notifications, SMS).", none(), ("Phone status", "وضعیت گوشی")),
        (
            "phone_notifications",
            "notifications",
            "Recent notifications on the user's phone, newest first: app, title, text, time (ms). Messages from Telegram, WhatsApp and other apps arrive here.",
            limit(),
            ("Reading the phone's notifications", "خواندن اعلان‌های گوشی"),
        ),
        ("phone_messages", "messages", "Recent SMS in the phone's inbox, newest first: from, text, time (ms), read.", limit(), ("Reading the phone's messages", "خواندن پیامک‌های گوشی")),
        ("phone_apps", "apps", "Names of the apps installed on the phone.", none(), ("Apps on the phone", "برنامه‌های گوشی")),
        (
            "phone_open_app",
            "openApp",
            "Opens an app on the phone by its name.",
            json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }),
            ("Opening an app on the phone", "باز کردن برنامه روی گوشی"),
        ),
        (
            "phone_ring",
            "ring",
            "Rings the user's phone loudly for 30 seconds (alarm sound, even when silenced) so they can find it.",
            none(),
            ("Ringing the phone", "زنگ زدن گوشی"),
        ),
        ("phone_stop_ring", "stopRing", "Stops the phone ringing (after phone_ring).", none(), ("Stopping the ring", "قطع زنگ گوشی")),
        (
            "phone_set_alarm",
            "setAlarm",
            "Sets an alarm on the user's phone (its clock app). Ask morning or evening if the hour is unclear.",
            json!({ "type": "object", "properties": { "hour": { "type": "integer", "description": "0-23" }, "minute": { "type": "integer" }, "label": { "type": "string" } }, "required": ["hour", "minute"] }),
            ("Setting an alarm on the phone", "تنظیم زنگ هشدار روی گوشی"),
        ),
        (
            "phone_set_timer",
            "setTimer",
            "Starts a timer on the user's phone (its clock app).",
            json!({ "type": "object", "properties": { "seconds": { "type": "integer" }, "label": { "type": "string" } }, "required": ["seconds"] }),
            ("Starting a timer on the phone", "شروع تایمر روی گوشی"),
        ),
        (
            "phone_dial",
            "dial",
            "Opens the phone's dialer with a number filled in; the user presses call themselves (nothing is called automatically).",
            json!({ "type": "object", "properties": { "number": { "type": "string" } }, "required": ["number"] }),
            ("Opening the dialer on the phone", "باز کردن شماره‌گیر گوشی"),
        ),
        (
            "phone_compose_sms",
            "composeSms",
            "Opens a new SMS on the phone with the number and text filled in; the user presses send themselves (nothing is sent automatically).",
            json!({ "type": "object", "properties": { "number": { "type": "string" }, "text": { "type": "string" } }, "required": ["text"] }),
            ("Writing an SMS on the phone", "نوشتن پیامک روی گوشی"),
        ),
        (
            "phone_navigate",
            "navigate",
            "Opens the phone's maps app at a place (a search for it; the user starts directions there).",
            json!({ "type": "object", "properties": { "destination": { "type": "string" } }, "required": ["destination"] }),
            ("Opening maps on the phone", "باز کردن نقشه روی گوشی"),
        ),
        (
            "phone_open_url",
            "openUrl",
            "Opens an http(s) link on the phone.",
            json!({ "type": "object", "properties": { "url": { "type": "string" } }, "required": ["url"] }),
            ("Opening a link on the phone", "باز کردن لینک روی گوشی"),
        ),
    ];
    let mut tools: Vec<Arc<dyn Tool>> = list
        .into_iter()
        .map(|(name, command, description, parameters, label)| Arc::new(PhoneTool { phone: phone.clone(), name, command, description, parameters, label }) as Arc<dyn Tool>)
        .collect();
    tools.push(Arc::new(SendFile { phone }));
    tools
}
