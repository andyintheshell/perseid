//! The per-frame proxy-event pump: capture, interception, and shutdown.

use std::time::Duration;

use super::*;

/// Bound on events applied per frame so a flood cannot stall rendering.
const MAX_EVENTS_PER_FRAME: usize = 1024;

impl PerseidApp {
    pub(super) fn drain_events(&mut self, ctx: &egui::Context) {
        // Repeater and Intruder are independent of the proxy, so drain them
        // before the proxy early return. Results may arrive while the proxy is
        // stopped.
        self.drain_repeater_results();
        self.drain_intruder_results();
        if self
            .repeater_tabs
            .iter()
            .any(repeater::RepeaterTab::is_pending)
            || self.intruder.is_running()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let mut selected_changed = false;
        let mut received_intercept = false;
        let mut queue_drained = false;
        let stopped;
        {
            let Some(proxy) = self.proxy.as_mut() else {
                return;
            };
            // Only discard a stopped runtime after draining a queue sampled
            // after it stopped. Shutdown itself may enqueue final
            // body-completion events.
            let was_stopped = !proxy.is_running();
            let dropped = proxy.dropped_events();
            let selected = self.selected;
            let websocket_selected = self.websocket_selected;
            for _ in 0..MAX_EVENTS_PER_FRAME {
                let Some(event) = proxy.try_recv() else {
                    queue_drained = true;
                    break;
                };
                // Give the project first refusal: it assigns stable IDs and
                // queues the normalized event for the writer. The UI applies
                // the returned event so its rows and the file agree.
                let event = match self.project.handle() {
                    Some(handle) => {
                        let outcome = handle.capture(&event);
                        let Some(event) = outcome.event else {
                            continue;
                        };
                        event
                    }
                    None => event,
                };
                let id = event.id();
                selected_changed |= selected == Some(id) || websocket_selected == Some(id);
                match event {
                    ProxyEvent::WebSocketMessage { .. } => {
                        self.websocket_history.apply(event);
                    }
                    event => {
                        self.history.apply(event);
                    }
                }
            }
            if queue_drained && dropped > self.dropped_seen {
                self.dropped_seen = dropped;
                let interrupted = self.history.interrupt_in_flight(
                    "capture events were dropped; the exchange result is unavailable",
                );
                selected_changed |= interrupted > 0;
                if interrupted > 0
                    && let Some(handle) = self.project.handle()
                {
                    handle.interrupt_in_flight(
                        "capture events were dropped; the exchange result is unavailable",
                    );
                }
            }
            if !queue_drained || proxy.dropped_events() > self.dropped_seen {
                ctx.request_repaint();
            }
            // Interception has its own bounded, backpressured channel.
            // Draining it here keeps held requests moving even when the
            // history event queue is full or capture is paused.
            for _ in 0..MAX_EVENTS_PER_FRAME {
                let Some(item) = proxy.try_recv_intercept() else {
                    break;
                };
                self.intercept_queue.push_back(InterceptEntry::new(item));
                received_intercept = true;
            }
            // Drain what the proxy queued before declaring it stopped, then
            // resolve any exchange whose terminal event never made it through.
            if !was_stopped && !proxy.is_running() {
                ctx.request_repaint();
            }
            stopped = was_stopped && queue_drained;
        }
        if stopped {
            self.proxy = None;
            self.proxy_error = Some("the proxy stopped unexpectedly".to_string());
            self.history
                .interrupt_in_flight("proxy stopped before the exchange finished");
            if let Some(handle) = self.project.handle() {
                handle.interrupt_in_flight("proxy stopped before the exchange finished");
            }
            selected_changed = true;
            self.clear_intercept_queue();
        }
        if selected_changed {
            self.detail = None;
            self.websocket_detail = None;
        }
        if self
            .selected
            .is_some_and(|id| self.history.get(id).is_none())
        {
            self.selected = None;
            self.detail = None;
        }
        if self
            .websocket_selected
            .is_some_and(|id| self.websocket_history.get(id).is_none())
        {
            self.websocket_selected = None;
            self.websocket_detail = None;
        }
        if received_intercept {
            self.intercept_scroll_to_active = true;
        }
        if received_intercept || !self.intercept_queue.is_empty() {
            self.sync_intercept_selection();
        }
        // Keep durations and in-flight rows ticking while anything is open.
        if self.history.in_flight() > 0 {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
