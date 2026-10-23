//! Versioned records stored inside a project.
//!
//! These types are the file format, not the runtime UI or proxy types. Keep
//! field names and enum tags stable; add new fields with `serde(default)` or a
//! new record version when the meaning changes.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::history::{HistoryItem, Phase};
use crate::proxy::{
    CapturedBody, ExchangeResult, ProxyEvent, RequestInfo, ResponseHead, WebSocketConnectionInfo,
    WebSocketDirection, WebSocketOpcode,
};
use crate::websocket_history::WebSocketMessageItem as WsHistoryItem;

use super::schema::RECORD_VERSION;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BodyCaptureRecord {
    pub observed_len: u64,
    pub stored_len: u64,
    pub truncated: bool,
    pub complete: bool,
}

impl BodyCaptureRecord {
    pub fn empty() -> Self {
        Self {
            observed_len: 0,
            stored_len: 0,
            truncated: false,
            complete: false,
        }
    }

    pub fn from_body(body: &CapturedBody) -> Self {
        Self {
            observed_len: body.total_len(),
            stored_len: body.data().len() as u64,
            truncated: body.is_truncated(),
            complete: body.is_complete(),
        }
    }

    pub fn to_body(&self, data: Vec<u8>) -> CapturedBody {
        CapturedBody::from_stored_parts(data, self.observed_len, self.truncated, self.complete)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum HttpPhaseRecord {
    InFlight,
    Completed { duration_ms: u64 },
    Failed { duration_ms: u64, error: String },
    Dropped { duration_ms: u64 },
    Interrupted { duration_ms: u64, reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpExchangeRecord {
    pub record_version: u16,
    pub id: u64,
    pub display_number: u64,
    pub started_at_ms: u64,
    pub duration_ms: Option<u64>,
    pub request: RequestInfo,
    pub response: Option<ResponseHead>,
    pub phase: HttpPhaseRecord,
    pub request_body: BodyCaptureRecord,
    pub response_body: BodyCaptureRecord,
}

impl HttpExchangeRecord {
    pub fn started(
        id: u64,
        display_number: u64,
        timestamp: SystemTime,
        request: RequestInfo,
    ) -> Self {
        Self {
            record_version: RECORD_VERSION,
            id,
            display_number,
            started_at_ms: system_time_ms(timestamp),
            duration_ms: None,
            request,
            response: None,
            phase: HttpPhaseRecord::InFlight,
            request_body: BodyCaptureRecord::empty(),
            response_body: BodyCaptureRecord::empty(),
        }
    }

    pub fn to_history_item(&self, request_body: Vec<u8>, response_body: Vec<u8>) -> HistoryItem {
        let request_body = self.request_body.to_body(request_body);
        let response_body = self.response_body.to_body(response_body);
        let phase = match &self.phase {
            HttpPhaseRecord::InFlight => Phase::InFlight,
            HttpPhaseRecord::Completed { duration_ms } => Phase::Finished {
                duration: Duration::from_millis(*duration_ms),
                result: ExchangeResult::Response {
                    body: response_body,
                },
            },
            HttpPhaseRecord::Failed { duration_ms, error } => Phase::Finished {
                duration: Duration::from_millis(*duration_ms),
                result: ExchangeResult::Failed {
                    error: error.clone(),
                },
            },
            HttpPhaseRecord::Dropped { duration_ms } => Phase::Finished {
                duration: Duration::from_millis(*duration_ms),
                result: ExchangeResult::Dropped,
            },
            HttpPhaseRecord::Interrupted {
                duration_ms,
                reason,
            } => Phase::Finished {
                duration: Duration::from_millis(*duration_ms),
                result: ExchangeResult::Failed {
                    error: reason.clone(),
                },
            },
        };
        HistoryItem {
            id: self.id,
            number: self.display_number,
            timestamp: system_time_from_ms(self.started_at_ms),
            request: self.request.clone(),
            request_body,
            response: self.response.clone(),
            phase,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WsDirectionRecord {
    ToServer,
    ToClient,
}

impl From<WebSocketDirection> for WsDirectionRecord {
    fn from(value: WebSocketDirection) -> Self {
        match value {
            WebSocketDirection::ToServer => Self::ToServer,
            WebSocketDirection::ToClient => Self::ToClient,
        }
    }
}

impl From<WsDirectionRecord> for WebSocketDirection {
    fn from(value: WsDirectionRecord) -> Self {
        match value {
            WsDirectionRecord::ToServer => Self::ToServer,
            WsDirectionRecord::ToClient => Self::ToClient,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WsOpcodeRecord {
    Text,
    Binary,
}

impl From<WebSocketOpcode> for WsOpcodeRecord {
    fn from(value: WebSocketOpcode) -> Self {
        match value {
            WebSocketOpcode::Text => Self::Text,
            WebSocketOpcode::Binary => Self::Binary,
        }
    }
}

impl From<WsOpcodeRecord> for WebSocketOpcode {
    fn from(value: WsOpcodeRecord) -> Self {
        match value {
            WsOpcodeRecord::Text => Self::Text,
            WsOpcodeRecord::Binary => Self::Binary,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WsMessageRecord {
    pub record_version: u16,
    pub id: u64,
    pub display_number: u64,
    pub timestamp_ms: u64,
    pub connection_id: u64,
    pub direction: WsDirectionRecord,
    pub opcode: WsOpcodeRecord,
    pub payload: BodyCaptureRecord,
}

impl WsMessageRecord {
    pub fn from_event(
        id: u64,
        display_number: u64,
        timestamp: SystemTime,
        connection_id: u64,
        direction: WebSocketDirection,
        opcode: WebSocketOpcode,
        payload: &CapturedBody,
    ) -> Self {
        Self {
            record_version: RECORD_VERSION,
            id,
            display_number,
            timestamp_ms: system_time_ms(timestamp),
            connection_id,
            direction: direction.into(),
            opcode: opcode.into(),
            payload: BodyCaptureRecord::from_body(payload),
        }
    }

    pub fn to_history_item(
        &self,
        connection: WebSocketConnectionInfo,
        payload: Vec<u8>,
    ) -> WsHistoryItem {
        WsHistoryItem {
            id: self.id,
            number: self.display_number,
            timestamp: system_time_from_ms(self.timestamp_ms),
            connection,
            direction: self.direction.into(),
            opcode: self.opcode.into(),
            payload: self.payload.to_body(payload),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WsConnectionRecord {
    pub record_version: u16,
    pub id: u64,
    pub url: String,
    pub host: String,
    pub tls: bool,
    pub listener_port: u16,
    pub protocol: Option<String>,
}

impl WsConnectionRecord {
    pub fn from_info(connection: &WebSocketConnectionInfo, id: u64) -> Self {
        Self {
            record_version: RECORD_VERSION,
            id,
            url: connection.url.clone(),
            host: connection.host.clone(),
            tls: connection.tls,
            listener_port: connection.listener_port,
            protocol: connection.protocol.clone(),
        }
    }

    pub fn to_info(&self) -> WebSocketConnectionInfo {
        WebSocketConnectionInfo {
            id: self.id,
            url: self.url.clone(),
            host: self.host.clone(),
            tls: self.tls,
            listener_port: self.listener_port,
            protocol: self.protocol.clone(),
        }
    }
}

/// Workbench state that belongs to a project rather than the machine.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceState {
    #[serde(default = "workspace_version")]
    pub version: u16,
    #[serde(default)]
    pub scope: ScopeWorkspace,
    #[serde(default)]
    pub match_replace: Vec<RuleWorkspace>,
    #[serde(default)]
    pub repeater: Vec<RepeaterWorkspace>,
    #[serde(default)]
    pub intruder: IntruderWorkspace,
    #[serde(default)]
    pub decoder: Vec<DecoderPanelWorkspace>,
    #[serde(default)]
    pub comparer: Vec<Vec<u8>>,
}

impl WorkspaceState {
    pub const CURRENT_VERSION: u16 = 1;
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            scope: ScopeWorkspace::default(),
            match_replace: Vec::new(),
            repeater: Vec::new(),
            intruder: IntruderWorkspace::default(),
            decoder: Vec::new(),
            comparer: Vec::new(),
        }
    }
}

fn workspace_version() -> u16 {
    WorkspaceState::CURRENT_VERSION
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeWorkspace {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub hosts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleDirectionRecord {
    #[default]
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RulePartRecord {
    #[default]
    Head,
    Body,
    All,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuleWorkspace {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub direction: RuleDirectionRecord,
    #[serde(default)]
    pub part: RulePartRecord,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub replacement: String,
    #[serde(default)]
    pub regex: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeaterWorkspace {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub scheme: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub request: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntruderAttackRecord {
    #[default]
    Sniper,
    BatteringRam,
    Pitchfork,
    ClusterBomb,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntruderWorkspace {
    #[serde(default)]
    pub scheme: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub request: String,
    #[serde(default)]
    pub attack: IntruderAttackRecord,
    #[serde(default)]
    pub payload_sets: Vec<String>,
    #[serde(default)]
    pub active_set: usize,
    #[serde(default)]
    pub concurrency: usize,
    #[serde(default)]
    pub update_host: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecoderPanelWorkspace {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub data: Vec<u8>,
}

pub fn system_time_ms(timestamp: SystemTime) -> u64 {
    timestamp
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub fn system_time_from_ms(milliseconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(milliseconds)
}

/// A normalized capture event, after the project has assigned stable IDs.
pub fn normalized_event(event: &ProxyEvent, id: u64) -> ProxyEvent {
    match event {
        ProxyEvent::Started {
            timestamp, request, ..
        } => ProxyEvent::Started {
            id,
            timestamp: *timestamp,
            request: request.clone(),
        },
        ProxyEvent::ResponseHead { head, .. } => ProxyEvent::ResponseHead {
            id,
            head: head.clone(),
        },
        ProxyEvent::Finished { end, .. } => ProxyEvent::Finished {
            id,
            end: end.clone(),
        },
        ProxyEvent::WebSocketMessage {
            timestamp,
            connection,
            direction,
            opcode,
            payload,
            ..
        } => ProxyEvent::WebSocketMessage {
            id,
            timestamp: *timestamp,
            connection: connection.clone(),
            direction: *direction,
            opcode: *opcode,
            payload: payload.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_capture_round_trips_metadata() {
        let body = CapturedBody::from_bytes(vec![1, 2, 3]);
        let record = BodyCaptureRecord::from_body(&body);
        let restored = record.to_body(body.data().to_vec());
        assert_eq!(restored.data(), body.data());
        assert_eq!(restored.total_len(), body.total_len());
        assert!(restored.is_complete());
        assert!(!restored.is_truncated());
    }

    #[test]
    fn workspace_defaults_are_stable() {
        let state = WorkspaceState::default();
        let json = serde_json::to_string(&state).expect("workspace should encode");
        let decoded: WorkspaceState = serde_json::from_str(&json).expect("workspace should decode");
        assert_eq!(state, decoded);
    }
}
