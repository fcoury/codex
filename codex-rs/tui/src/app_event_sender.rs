use tokio::sync::mpsc::UnboundedSender;

use crate::app_event::AppEvent;
use crate::app_event::RuntimeEvent;
use crate::app_event::TuiAction;
use crate::session_log;

#[derive(Clone, Debug)]
pub(crate) struct AppEventSender {
    pub app_event_tx: UnboundedSender<RuntimeEvent>,
}

impl AppEventSender {
    pub(crate) fn new(app_event_tx: UnboundedSender<RuntimeEvent>) -> Self {
        Self { app_event_tx }
    }

    /// Send an event to the app event channel. If it fails, we swallow the
    /// error and log it.
    pub(crate) fn send(&self, event: AppEvent) {
        // Record inbound events for high-fidelity session replay.
        // Avoid double-logging Ops; those are logged at the point of submission.
        if !matches!(event, AppEvent::CodexOp(_)) {
            session_log::log_inbound_app_event(&event);
        }
        if let Err(e) = self.app_event_tx.send(RuntimeEvent::App(Box::new(event))) {
            tracing::error!("failed to send event: {e}");
        }
    }

    pub(crate) fn send_tui_action(&self, event: TuiAction) {
        if let Err(e) = self.app_event_tx.send(RuntimeEvent::Action(event)) {
            tracing::error!("failed to send TUI action: {e}");
        }
    }
}
