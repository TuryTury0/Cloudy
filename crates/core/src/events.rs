//! One event stream for everything the assistant does. The island, chat window, task panel, log file and (later)
//! the phone all read the same events.

use crate::state::StateUpdate;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CoreEvent {
    /// The visible state changed.
    State(StateUpdate),
    /// A request started.
    #[serde(rename_all = "camelCase")]
    TurnStarted { turn_id: u64, user: String },
    /// A tool is running; `label` is a short human line ("Searching: gold price").
    #[serde(rename_all = "camelCase")]
    ToolStarted { turn_id: u64, call_id: String, name: String, label: String, args: Value },
    /// A detail line for the log (timings, decisions); not shown on the island.
    #[serde(rename_all = "camelCase")]
    Log { turn_id: Option<u64>, source: String, text: String },
    /// A live sub-step of a running tool, e.g. one desktop action.
    #[serde(rename_all = "camelCase")]
    ToolProgress { turn_id: u64, call_id: String, text: String },
    #[serde(rename_all = "camelCase")]
    ToolFinished { turn_id: u64, call_id: String, name: String, ok: bool, summary: String },
    /// The final written answer.
    #[serde(rename_all = "camelCase")]
    Answer { turn_id: u64, text: String },
    /// Tokens one model call used.
    #[serde(rename_all = "camelCase")]
    Usage { turn_id: u64, label: String, input: u64, output: u64, thinking: u64, cached: u64 },
    /// Tokens a whole request used: every model call in it, screen steps included.
    #[serde(rename_all = "camelCase")]
    TurnUsage { turn_id: u64, calls: u32, input: u64, output: u64, thinking: u64 },
    /// Which model answered, or why it failed.
    #[serde(rename_all = "camelCase")]
    ModelUsed { label: String, ok: bool, #[serde(skip_serializing_if = "Option::is_none")] reason: Option<String> },
    #[serde(rename_all = "camelCase")]
    Error { turn_id: Option<u64>, message: String },
    #[serde(rename_all = "camelCase")]
    Cancelled { turn_id: u64 },
}

/// Fan-out channel; slow listeners miss old events rather than block the agent.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<CoreEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(512)
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        Self { sender: broadcast::channel(capacity).0 }
    }

    pub fn emit(&self, event: CoreEvent) {
        let _ = self.sender.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.sender.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AssistantState;

    #[test]
    fn events_serialize_with_kind() {
        let json = serde_json::to_value(CoreEvent::ToolStarted {
            turn_id: 1,
            call_id: "c".into(),
            name: "web_search".into(),
            label: "Searching".into(),
            args: serde_json::json!({}),
        })
        .unwrap();
        assert_eq!(json["kind"], "toolStarted");
        assert_eq!(json["turnId"], 1);
        let state = serde_json::to_value(CoreEvent::State(StateUpdate::new(AssistantState::Answer))).unwrap();
        assert_eq!(state["kind"], "state");
        assert_eq!(state["state"], "answer");
    }

    #[tokio::test]
    async fn subscribers_receive_events() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        bus.emit(CoreEvent::Cancelled { turn_id: 7 });
        assert_eq!(rx.recv().await.unwrap(), CoreEvent::Cancelled { turn_id: 7 });
    }
}
