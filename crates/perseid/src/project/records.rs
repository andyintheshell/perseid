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
///
/// This intentionally records user-controlled workbench content, not live
/// operations. Running attacks, pending sends, held interception decisions,
/// proxy lifecycle state, and the local certificate authority stay outside the
/// project.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceState {
    /// Bump when a workspace field or persisted enum gains a value that older
    /// readers cannot decode. `version` 2 added `TargetSectionRecord::Issues`.
    #[serde(default = "workspace_version")]
    pub version: u16,
    #[serde(default)]
    pub scope: ScopeWorkspace,
    #[serde(default)]
    pub match_replace: Vec<RuleWorkspace>,
    #[serde(default)]
    pub ui: UiWorkspace,
    #[serde(default)]
    pub inspector: InspectorWorkspace,
    #[serde(default)]
    pub repeater: Vec<RepeaterWorkspace>,
    #[serde(default)]
    pub intruder: IntruderWorkspace,
    #[serde(default)]
    pub decoder: Vec<DecoderPanelWorkspace>,
    #[serde(default)]
    pub comparer: Vec<Vec<u8>>,
    #[serde(default)]
    pub comparer_ui: ComparerWorkspace,
    #[serde(default)]
    pub search: SearchWorkspace,
    #[serde(default)]
    pub sequencer: SequencerWorkspace,
    #[serde(default)]
    pub tls: TlsWorkspace,
}

impl WorkspaceState {
    pub const CURRENT_VERSION: u16 = 2;
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            scope: ScopeWorkspace::default(),
            match_replace: Vec::new(),
            ui: UiWorkspace::default(),
            inspector: InspectorWorkspace::default(),
            repeater: Vec::new(),
            intruder: IntruderWorkspace::default(),
            decoder: Vec::new(),
            comparer: Vec::new(),
            comparer_ui: ComparerWorkspace::default(),
            search: SearchWorkspace::default(),
            sequencer: SequencerWorkspace::default(),
            tls: TlsWorkspace::default(),
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
    /// Block out-of-scope traffic instead of only filtering the views.
    #[serde(default)]
    pub enforce: bool,
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

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MainTabRecord {
    Target,
    #[default]
    Proxy,
    Intruder,
    Repeater,
    Sequencer,
    Decoder,
    Comparer,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxySectionRecord {
    Intercept,
    #[default]
    HttpHistory,
    WebSocketsHistory,
    MatchReplace,
    Tls,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetSectionRecord {
    #[default]
    SiteMap,
    Scope,
    Issues,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessagePaneRecord {
    #[default]
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageViewRecord {
    #[default]
    Pretty,
    Raw,
    Hex,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SideTabRecord {
    #[default]
    Inspector,
    Notes,
}

/// Burp hides these static asset extensions by default.
const BURP_HIDDEN_EXTENSIONS: &str = "js,gif,jpg,png,ico,css,woff,woff2";

/// Burp-style HTTP history display filter, persisted with the project.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryFilterWorkspace {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub in_scope_only: bool,
    #[serde(default)]
    pub hide_without_response: bool,
    #[serde(default)]
    pub parameterized_only: bool,
    #[serde(default = "default_true")]
    pub show_html: bool,
    #[serde(default = "default_true")]
    pub show_script: bool,
    #[serde(default = "default_true")]
    pub show_xml: bool,
    #[serde(default)]
    pub show_css: bool,
    #[serde(default = "default_true")]
    pub show_other_text: bool,
    #[serde(default)]
    pub show_images: bool,
    #[serde(default = "default_true")]
    pub show_flash: bool,
    #[serde(default = "default_true")]
    pub show_other_binary: bool,
    #[serde(default = "default_true")]
    pub show_2xx: bool,
    #[serde(default = "default_true")]
    pub show_3xx: bool,
    #[serde(default = "default_true")]
    pub show_4xx: bool,
    #[serde(default = "default_true")]
    pub show_5xx: bool,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub search_regex: bool,
    #[serde(default)]
    pub search_case_sensitive: bool,
    #[serde(default)]
    pub search_negative: bool,
    #[serde(default)]
    pub extension_show_only_enabled: bool,
    #[serde(default)]
    pub extension_show_only: String,
    #[serde(default = "default_true")]
    pub extension_hide_enabled: bool,
    #[serde(default = "default_hidden_extensions")]
    pub extension_hide: String,
}

fn default_hidden_extensions() -> String {
    BURP_HIDDEN_EXTENSIONS.to_string()
}

impl Default for HistoryFilterWorkspace {
    fn default() -> Self {
        Self {
            enabled: true,
            in_scope_only: false,
            hide_without_response: false,
            parameterized_only: false,
            show_html: true,
            show_script: true,
            show_xml: true,
            show_css: false,
            show_other_text: true,
            show_images: false,
            show_flash: true,
            show_other_binary: true,
            show_2xx: true,
            show_3xx: true,
            show_4xx: true,
            show_5xx: true,
            search: String::new(),
            search_regex: false,
            search_case_sensitive: false,
            search_negative: false,
            extension_show_only_enabled: false,
            extension_show_only: String::new(),
            extension_hide_enabled: true,
            extension_hide: default_hidden_extensions(),
        }
    }
}

/// Active tool, section, filters, and selection for the main workspace.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiWorkspace {
    #[serde(default)]
    pub main_tab: MainTabRecord,
    #[serde(default)]
    pub proxy_section: ProxySectionRecord,
    #[serde(default)]
    pub target_section: TargetSectionRecord,
    #[serde(default)]
    pub request_view: MessageViewRecord,
    #[serde(default)]
    pub response_view: MessageViewRecord,
    #[serde(default)]
    pub websocket_view: MessageViewRecord,
    #[serde(default)]
    pub http_filter: String,
    #[serde(default)]
    pub history_filter: HistoryFilterWorkspace,
    #[serde(default)]
    pub websocket_filter: String,
    #[serde(default)]
    pub selected_http_id: Option<u64>,
    #[serde(default)]
    pub selected_websocket_id: Option<u64>,
    /// Index into the Repeater tab list.
    #[serde(default)]
    pub repeater_active: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InspectorNoteWorkspace {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InspectorWorkspace {
    #[serde(default = "default_true")]
    pub open: bool,
    #[serde(default)]
    pub tab: SideTabRecord,
    #[serde(default)]
    pub expanded: Vec<String>,
    #[serde(default)]
    pub notes: Vec<InspectorNoteWorkspace>,
}

impl Default for InspectorWorkspace {
    fn default() -> Self {
        Self {
            open: true,
            tab: SideTabRecord::default(),
            expanded: Vec::new(),
            notes: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageViewsWorkspace {
    #[serde(default)]
    pub pretty: String,
    #[serde(default)]
    pub raw: String,
    #[serde(default)]
    pub hex: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepeaterResponseWorkspace {
    #[serde(default)]
    pub views: MessageViewsWorkspace,
    #[serde(default)]
    pub status: u16,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub body_complete: bool,
    #[serde(default)]
    pub body_error: Option<String>,
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
    /// Last completed response, if one was retained when the project was saved.
    #[serde(default)]
    pub response: Option<RepeaterResponseWorkspace>,
    /// Last direct-send failure shown in the tab.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub response_view: MessageViewRecord,
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

/// A bounded byte body stored as base64 inside the workspace JSON.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BodyWorkspace {
    #[serde(default, with = "base64_bytes")]
    pub data: Vec<u8>,
    #[serde(default)]
    pub total_len: u64,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub complete: bool,
}

impl BodyWorkspace {
    pub fn from_body(body: &CapturedBody) -> Self {
        Self {
            data: body.data().to_vec(),
            total_len: body.total_len(),
            truncated: body.is_truncated(),
            complete: body.is_complete(),
        }
    }

    pub fn to_body(&self) -> CapturedBody {
        CapturedBody::from_stored_parts(
            self.data.clone(),
            self.total_len,
            self.truncated,
            self.complete,
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum IntruderResultOutcomeWorkspace {
    Response {
        #[serde(default)]
        version: String,
        #[serde(default)]
        status: u16,
        #[serde(default)]
        reason: String,
        #[serde(default)]
        headers: Vec<(String, String)>,
        #[serde(default)]
        body: BodyWorkspace,
        #[serde(default)]
        body_error: Option<String>,
    },
    Failed {
        #[serde(default)]
        error: String,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntruderResultWorkspace {
    #[serde(default)]
    pub attack_id: u64,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub payloads: Vec<String>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub outcome: IntruderResultOutcomeWorkspace,
}

impl Default for IntruderResultOutcomeWorkspace {
    fn default() -> Self {
        Self::Failed {
            error: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntruderPlanWorkspace {
    #[serde(default)]
    pub attack: IntruderAttackRecord,
    #[serde(default)]
    pub scheme: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub payload_sets: Vec<Vec<String>>,
    #[serde(default)]
    pub concurrency: usize,
    #[serde(default)]
    pub scope: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextSelectionWorkspace {
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub end: usize,
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
    #[serde(default)]
    pub selection: Option<TextSelectionWorkspace>,
    #[serde(default)]
    pub cursor: usize,
    #[serde(default)]
    pub results: Vec<IntruderResultWorkspace>,
    #[serde(default)]
    pub selected: Option<u64>,
    #[serde(default)]
    pub detail_pane: MessagePaneRecord,
    #[serde(default)]
    pub detail_view: MessageViewRecord,
    #[serde(default)]
    pub attack_id: Option<u64>,
    #[serde(default)]
    pub last_plan: Option<IntruderPlanWorkspace>,
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub received: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub finished_note: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecoderCodecRecord {
    #[default]
    Url,
    Html,
    Base64,
    AsciiHex,
    Hex,
    Octal,
    Binary,
    Gzip,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecoderHashRecord {
    Blake2b160,
    Blake2b256,
    Blake2b384,
    Blake2b512,
    Blake2s128,
    Blake2s160,
    Blake2s224,
    Blake2s256,
    Blake3,
    Keccak224,
    Keccak256,
    Keccak384,
    Keccak512,
    Md2,
    Md4,
    Md5,
    #[default]
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Sha512_224,
    Sha512_256,
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
    Sm3,
    Tiger,
    Whirlpool,
    Ripemd160,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecoderColorTagRecord {
    Codec(DecoderCodecRecord),
    Hash,
    #[default]
    Plain,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecoderSpanWorkspace {
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub end: usize,
    #[serde(default)]
    pub tag: DecoderColorTagRecord,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecoderOperationRecord {
    #[default]
    Plain,
    Decode(DecoderCodecRecord),
    Encode(DecoderCodecRecord),
    Hash(DecoderHashRecord),
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecoderViewRecord {
    #[default]
    Text,
    Hex,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecoderPanelWorkspace {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub data: Vec<u8>,
    #[serde(default)]
    pub spans: Vec<DecoderSpanWorkspace>,
    #[serde(default)]
    pub operation: Option<DecoderOperationRecord>,
    #[serde(default)]
    pub view: DecoderViewRecord,
    #[serde(default)]
    pub selection: Option<TextSelectionWorkspace>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompareModeRecord {
    #[default]
    Words,
    Bytes,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultViewRecord {
    #[default]
    Text,
    Hex,
}

/// Selections, result window state, and view modes for the Comparer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComparerWorkspace {
    #[serde(default)]
    pub selected: [Option<usize>; 2],
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub result_open: bool,
    #[serde(default)]
    pub result_mode: Option<CompareModeRecord>,
    /// Items compared by the retained result, which can differ from the
    /// current table selections while the result window stays open.
    #[serde(default)]
    pub result_items: Option<[usize; 2]>,
    #[serde(default)]
    pub result_views: [ResultViewRecord; 2],
    #[serde(default)]
    pub sync_views: bool,
    #[serde(default)]
    pub diff_cursor: usize,
}

impl Default for ComparerWorkspace {
    fn default() -> Self {
        Self {
            selected: [None, None],
            active: 0,
            result_open: false,
            result_mode: None,
            result_items: None,
            result_views: [ResultViewRecord::Text, ResultViewRecord::Text],
            sync_views: false,
            diff_cursor: 0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchOptionsWorkspace {
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub negative: bool,
    #[serde(default)]
    pub in_scope_only: bool,
    #[serde(default)]
    pub dynamic_update: bool,
    #[serde(default = "default_true")]
    pub proxy_history: bool,
    #[serde(default = "default_true")]
    pub request_headers: bool,
    #[serde(default = "default_true")]
    pub request_body: bool,
    #[serde(default = "default_true")]
    pub response_headers: bool,
    #[serde(default = "default_true")]
    pub response_body: bool,
}

impl Default for SearchOptionsWorkspace {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            regex: false,
            negative: false,
            in_scope_only: false,
            dynamic_update: false,
            proxy_history: true,
            request_headers: true,
            request_body: true,
            response_headers: true,
            response_body: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchWorkspace {
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub options: SearchOptionsWorkspace,
    #[serde(default)]
    pub ran: bool,
    #[serde(default)]
    pub dirty: bool,
    /// Result row IDs. They refer to project history IDs and are filtered to
    /// rows present in the loaded snapshot when the workspace is applied.
    #[serde(default)]
    pub results: Vec<u64>,
    #[serde(default)]
    pub selected: Option<u64>,
    #[serde(default)]
    pub pane: MessagePaneRecord,
    #[serde(default)]
    pub request_view: MessageViewRecord,
    #[serde(default)]
    pub response_view: MessageViewRecord,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SequencerSectionRecord {
    LiveCapture,
    #[default]
    ManualLoad,
    Settings,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapturePartRecord {
    #[default]
    Body,
    Headers,
    Both,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SequencerResultTabRecord {
    #[default]
    Summary,
    Character,
    Bit,
    Settings,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SequencerTokenFormatRecord {
    #[default]
    Auto,
    Hex,
    Base64,
    Text,
}

#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SequencerSignificanceRecord {
    TenPercent,
    #[default]
    OnePercent,
    PointOnePercent,
    PointZeroOnePercent,
    PointZeroZeroOnePercent,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SequencerCharacterAnalysisRecord {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SequencerAnalysisSettingsWorkspace {
    #[serde(default)]
    pub format: SequencerTokenFormatRecord,
    #[serde(default)]
    pub significance: SequencerSignificanceRecord,
    #[serde(default)]
    pub character_analysis: SequencerCharacterAnalysisRecord,
    #[serde(default = "default_true")]
    pub bit_analysis: bool,
}

impl Default for SequencerAnalysisSettingsWorkspace {
    fn default() -> Self {
        Self {
            format: SequencerTokenFormatRecord::Auto,
            significance: SequencerSignificanceRecord::OnePercent,
            character_analysis: SequencerCharacterAnalysisRecord::Auto,
            bit_analysis: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SequencerSettingsWorkspace {
    #[serde(default)]
    pub analysis: SequencerAnalysisSettingsWorkspace,
    #[serde(default)]
    pub capture_regex: String,
    #[serde(default)]
    pub capture_part: CapturePartRecord,
    #[serde(default = "default_true")]
    pub deduplicate: bool,
}

impl Default for SequencerSettingsWorkspace {
    fn default() -> Self {
        Self {
            analysis: SequencerAnalysisSettingsWorkspace::default(),
            capture_regex: String::new(),
            capture_part: CapturePartRecord::Body,
            deduplicate: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SequencerWorkspace {
    #[serde(default)]
    pub section: SequencerSectionRecord,
    #[serde(default)]
    pub tokens: Vec<String>,
    #[serde(default)]
    pub settings: SequencerSettingsWorkspace,
    #[serde(default)]
    pub result_open: bool,
    #[serde(default)]
    pub result_tab: SequencerResultTabRecord,
    #[serde(default)]
    pub result_title: String,
    #[serde(default)]
    pub analysis_count: u64,
    /// Source label for the retained analysis. The analysis itself is
    /// recomputed from the saved token sample and settings on open.
    #[serde(default)]
    pub result_source: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TlsWorkspace {
    #[serde(default)]
    pub intercept: bool,
    #[serde(default)]
    pub passthrough: Vec<String>,
}

/// Base64 keeps arbitrary response bodies valid inside the JSON workspace
/// record while staying substantially smaller than a JSON array of bytes.
mod base64_bytes {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(text)
            .map_err(serde::de::Error::custom)
    }
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

    #[test]
    fn older_workspace_json_gets_defaults_for_new_fields() {
        // The shape written before Inspector, UI, Search, Sequencer, Comparer
        // view state, and TLS settings were added.
        let json = r#"{
            "version": 1,
            "scope": {"enabled": true, "enforce": false, "hosts": ["example.test"]},
            "match_replace": [],
            "repeater": [],
            "intruder": {
                "scheme": "https",
                "host": "example.test",
                "request": "GET / HTTP/1.1\r\nHost: example.test\r\n\r\n",
                "attack": "sniper",
                "payload_sets": ["one"],
                "active_set": 0,
                "concurrency": 4,
                "update_host": true,
                "note": null
            },
            "decoder": [],
            "comparer": [[1, 2, 3]]
        }"#;
        let decoded: WorkspaceState = serde_json::from_str(json).expect("legacy workspace");

        assert_eq!(decoded.scope.hosts, ["example.test"]);
        assert_eq!(decoded.intruder.concurrency, 4);
        assert_eq!(decoded.comparer, [vec![1, 2, 3]]);
        assert_eq!(decoded.ui, UiWorkspace::default());
        assert!(!decoded.ui.history_filter.show_css);
        assert!(!decoded.ui.history_filter.show_images);
        assert!(decoded.ui.history_filter.extension_hide_enabled);
        assert_eq!(
            decoded.ui.history_filter.extension_hide,
            "js,gif,jpg,png,ico,css,woff,woff2"
        );
        assert_eq!(decoded.inspector, InspectorWorkspace::default());
        assert_eq!(decoded.comparer_ui, ComparerWorkspace::default());
        assert_eq!(decoded.search, SearchWorkspace::default());
        assert_eq!(decoded.sequencer, SequencerWorkspace::default());
        assert_eq!(decoded.tls, TlsWorkspace::default());
    }

    #[test]
    fn workspace_bodies_and_view_state_round_trip() {
        let body = CapturedBody::from_stored_parts(vec![0, 1, 2, 255], 9, true, false);
        let workspace = WorkspaceState {
            ui: UiWorkspace {
                main_tab: MainTabRecord::Repeater,
                proxy_section: ProxySectionRecord::Tls,
                target_section: TargetSectionRecord::Scope,
                request_view: MessageViewRecord::Raw,
                response_view: MessageViewRecord::Hex,
                websocket_view: MessageViewRecord::Raw,
                http_filter: "status:200".to_string(),
                history_filter: HistoryFilterWorkspace::default(),
                websocket_filter: "chat".to_string(),
                selected_http_id: Some(7),
                selected_websocket_id: Some(8),
                repeater_active: 1,
            },
            inspector: InspectorWorkspace {
                open: false,
                tab: SideTabRecord::Notes,
                expanded: vec!["request.headers".to_string()],
                notes: vec![InspectorNoteWorkspace {
                    id: 7,
                    text: "check the auth header".to_string(),
                }],
            },
            repeater: vec![RepeaterWorkspace {
                id: 3,
                title: "Login".to_string(),
                scheme: "https".to_string(),
                host: "example.test".to_string(),
                request: "GET / HTTP/1.1\r\nHost: example.test\r\n\r\n".to_string(),
                note: Some("seeded".to_string()),
                response: Some(RepeaterResponseWorkspace {
                    views: MessageViewsWorkspace {
                        pretty: "pretty".to_string(),
                        raw: "raw".to_string(),
                        hex: "00".to_string(),
                    },
                    status: 200,
                    reason: "OK".to_string(),
                    duration_ms: 12,
                    size: 4,
                    truncated: false,
                    body_complete: true,
                    body_error: None,
                }),
                error: None,
                response_view: MessageViewRecord::Raw,
            }],
            intruder: IntruderWorkspace {
                results: vec![IntruderResultWorkspace {
                    attack_id: 1,
                    sequence: 2,
                    payloads: vec!["admin".to_string()],
                    duration_ms: 9,
                    outcome: IntruderResultOutcomeWorkspace::Response {
                        version: "HTTP/1.1".to_string(),
                        status: 200,
                        reason: "OK".to_string(),
                        headers: vec![("Content-Length".to_string(), "4".to_string())],
                        body: BodyWorkspace::from_body(&body),
                        body_error: None,
                    },
                }],
                ..Default::default()
            },
            decoder: vec![DecoderPanelWorkspace {
                id: 1,
                data: b"abc".to_vec(),
                spans: vec![DecoderSpanWorkspace {
                    start: 0,
                    end: 3,
                    tag: DecoderColorTagRecord::Codec(DecoderCodecRecord::Url),
                }],
                operation: Some(DecoderOperationRecord::Decode(DecoderCodecRecord::Url)),
                view: DecoderViewRecord::Hex,
                selection: Some(TextSelectionWorkspace { start: 0, end: 3 }),
            }],
            comparer: vec![b"one".to_vec(), b"two".to_vec()],
            comparer_ui: ComparerWorkspace {
                selected: [Some(0), Some(1)],
                active: 1,
                result_open: true,
                result_mode: Some(CompareModeRecord::Words),
                result_items: Some([0, 1]),
                result_views: [ResultViewRecord::Text, ResultViewRecord::Hex],
                sync_views: true,
                diff_cursor: 0,
            },
            search: SearchWorkspace {
                open: true,
                query: "needle".to_string(),
                options: SearchOptionsWorkspace {
                    regex: true,
                    negative: true,
                    in_scope_only: true,
                    ..Default::default()
                },
                ran: true,
                dirty: false,
                results: vec![7],
                selected: Some(7),
                pane: MessagePaneRecord::Request,
                request_view: MessageViewRecord::Hex,
                response_view: MessageViewRecord::Raw,
                error: None,
            },
            sequencer: SequencerWorkspace {
                section: SequencerSectionRecord::ManualLoad,
                tokens: vec!["deadbeef".to_string(), "cafebabe".to_string()],
                settings: SequencerSettingsWorkspace {
                    analysis: SequencerAnalysisSettingsWorkspace {
                        format: SequencerTokenFormatRecord::Hex,
                        significance: SequencerSignificanceRecord::TenPercent,
                        character_analysis: SequencerCharacterAnalysisRecord::Always,
                        bit_analysis: false,
                    },
                    capture_regex: "token=(\\w+)".to_string(),
                    capture_part: CapturePartRecord::Both,
                    deduplicate: false,
                },
                result_open: true,
                result_tab: SequencerResultTabRecord::Character,
                result_title: "Perseid Sequencer [manual load analysis #2]".to_string(),
                analysis_count: 2,
                result_source: Some("manual load".to_string()),
            },
            tls: TlsWorkspace {
                intercept: true,
                passthrough: vec!["pinned.example.test".to_string()],
            },
            ..Default::default()
        };

        let json = serde_json::to_string(&workspace).expect("encode workspace");
        let decoded: WorkspaceState = serde_json::from_str(&json).expect("decode workspace");
        assert_eq!(decoded, workspace);
        assert_eq!(
            decoded.intruder.results[0].outcome,
            IntruderResultOutcomeWorkspace::Response {
                version: "HTTP/1.1".to_string(),
                status: 200,
                reason: "OK".to_string(),
                headers: vec![("Content-Length".to_string(), "4".to_string())],
                body: BodyWorkspace::from_body(&body),
                body_error: None,
            }
        );
        assert_eq!(decoded.comparer[1], b"two");
    }
}
