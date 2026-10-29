use std::time::SystemTime;

use crate::proxy::{
    CapturedBody, ExchangeEnd, ExchangeResult, RequestInfo, ResponseHead, WebSocketConnectionInfo,
    WebSocketDirection, WebSocketOpcode,
};

use super::*;

const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// A unique path under the system temporary directory, removed on drop.
struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("perseid-{label}-{}.perseid", uuid::Uuid::new_v4()));
        Self { path }
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn timestamp(offset_ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_000 + offset_ms)
}

fn started(wire_id: u64, method: &str) -> ProxyEvent {
    started_at(wire_id, wire_id, method)
}

fn started_at(wire_id: u64, offset_ms: u64, method: &str) -> ProxyEvent {
    ProxyEvent::Started {
        id: wire_id,
        timestamp: timestamp(offset_ms),
        request: RequestInfo {
            method: method.to_string(),
            uri: "http://example.com/api?x=1".to_string(),
            scheme: "http".to_string(),
            host: "example.com".to_string(),
            path: "/api?x=1".to_string(),
            version: "HTTP/1.1".to_string(),
            headers: vec![("host".to_string(), "example.com".to_string())],
        },
    }
}

fn response_head(wire_id: u64, status: u16) -> ProxyEvent {
    ProxyEvent::ResponseHead {
        id: wire_id,
        head: ResponseHead {
            version: "HTTP/1.1".to_string(),
            status,
            reason: String::new(),
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
        },
    }
}

fn finished(wire_id: u64, request: &[u8], response: &[u8]) -> ProxyEvent {
    finished_with(
        wire_id,
        CapturedBody::from_bytes(request.to_vec()),
        ExchangeResult::Response {
            body: CapturedBody::from_bytes(response.to_vec()),
        },
    )
}

fn finished_with(wire_id: u64, request_body: CapturedBody, result: ExchangeResult) -> ProxyEvent {
    ProxyEvent::Finished {
        id: wire_id,
        end: ExchangeEnd {
            duration: Duration::from_millis(7),
            request_body,
            result,
        },
    }
}

fn ws_message(wire_id: u64, wire_connection_id: u64, payload: &[u8]) -> ProxyEvent {
    ws_message_at(wire_id, wire_id, wire_connection_id, payload)
}

fn ws_message_at(
    wire_id: u64,
    offset_ms: u64,
    wire_connection_id: u64,
    payload: &[u8],
) -> ProxyEvent {
    ProxyEvent::WebSocketMessage {
        id: wire_id,
        timestamp: timestamp(offset_ms),
        connection: WebSocketConnectionInfo {
            id: wire_connection_id,
            url: "wss://example.com/socket".to_string(),
            host: "example.com".to_string(),
            tls: true,
            listener_port: 8080,
            protocol: Some("chat".to_string()),
        },
        direction: WebSocketDirection::ToClient,
        opcode: WebSocketOpcode::Text,
        payload: CapturedBody::from_bytes(payload.to_vec()),
    }
}

fn wait<T>(receiver: Receiver<Result<T>>) -> Result<T> {
    receiver
        .recv_timeout(REPLY_TIMEOUT)
        .expect("project writer dropped the reply channel")
}

fn capture_all(project: &ProjectHandle, events: &[ProxyEvent]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|event| project.capture(event).event)
        .map(|event| event.id())
        .collect()
}

/// A raw in-memory project for tests that drive the writer's batch application
/// directly, bypassing the service's ID normalization.
fn batch_project() -> (redb::Database, ProjectMeta) {
    let meta = ProjectMeta::new(new_project_id());
    let db = super::super::backend::create_memory(&meta).expect("memory project");
    (db, meta)
}

fn apply_batch(db: &redb::Database, meta: &mut ProjectMeta, events: &[ProxyEvent]) -> Result<()> {
    super::persistence::apply_capture_batch(db, meta, None, events)
}

fn insert_stale_ws_payload(db: &redb::Database, id: u64, payload: &[u8]) {
    let tx = db.begin_write().expect("begin write");
    tx.open_table(schema::WS_PAYLOADS)
        .expect("payloads")
        .insert(id, payload)
        .expect("stale payload");
    tx.commit().expect("commit");
}

#[test]
fn temporary_project_assigns_distinct_ids() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let event = started(1, "GET");
    let first = project.capture(&event).event.expect("normalized event");
    let second = project
        .capture(&event)
        .event
        .expect("duplicate normalized event");
    assert_ne!(first.id(), second.id());
    project.shutdown();
}

#[test]
fn capture_round_trip_persists_http_and_websocket() {
    let project = ProjectHandle::temporary().expect("temporary project");

    let ids = capture_all(
        &project,
        &[
            started(42, "POST"),
            response_head(42, 201),
            finished(42, b"request-body", b"response-body"),
        ],
    );
    assert_eq!(ids, vec![1, 1, 1]);
    let http_id = ids[0];

    let first_ws = project
        .capture(&ws_message(100, 7, b"hello"))
        .event
        .expect("normalized websocket message");
    let second_ws = project
        .capture(&ws_message(101, 7, b"again"))
        .event
        .expect("normalized websocket message");
    let ProxyEvent::WebSocketMessage {
        id: ws_id,
        connection,
        ..
    } = &first_ws
    else {
        panic!("expected a websocket event");
    };
    assert_eq!(connection.id, 1, "wire connection IDs are project-local");
    assert_eq!(second_ws.id(), ws_id + 1);
    let ProxyEvent::WebSocketMessage {
        connection: second, ..
    } = &second_ws
    else {
        panic!("expected a websocket event");
    };
    assert_eq!(second.id, connection.id);

    let receipt = wait(project.flush()).expect("flush");
    assert_eq!(receipt.http_count, 1);
    assert_eq!(receipt.ws_count, 2);

    let http = project
        .get_http(http_id)
        .expect("read HTTP record")
        .expect("stored HTTP record");
    assert_eq!(http.id, http_id);
    assert_eq!(http.number, 1);
    assert_eq!(http.request.method, "POST");
    assert_eq!(http.status(), Some(201));
    assert_eq!(http.request_body.data(), b"request-body");
    assert_eq!(
        http.response_body().expect("response body").data(),
        b"response-body"
    );
    assert_eq!(http.duration(), Some(Duration::from_millis(7)));
    assert!(!http.is_in_flight());

    let ws = project
        .get_ws(*ws_id)
        .expect("read websocket record")
        .expect("stored websocket record");
    assert_eq!(ws.id, *ws_id);
    assert_eq!(ws.payload.data(), b"hello");
    assert_eq!(ws.connection.id, connection.id);
    assert_eq!(ws.connection.url, "wss://example.com/socket");
    assert_eq!(ws.direction, WebSocketDirection::ToClient);
    assert_eq!(ws.opcode, WebSocketOpcode::Text);

    let status = project.status();
    assert_eq!(status.http_count, 1);
    assert_eq!(status.ws_count, 2);
    assert_eq!(status.mode, ProjectMode::Temporary);
}

#[test]
fn save_as_switches_to_a_persistent_project_that_reopens() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(
        &project,
        &[
            started(5, "GET"),
            response_head(5, 200),
            finished(5, b"", b"ok"),
            ws_message(6, 3, b"message"),
        ],
    );
    let http_id = ids[0];

    let mut workspace = WorkspaceState::default();
    workspace.scope.enabled = true;
    workspace.scope.enforce = true;
    workspace.scope.hosts = vec!["*.example.com".to_string()];
    assert!(project.set_workspace(workspace.clone()));
    wait(project.flush()).expect("flush");

    let file = TempProject::new("save-as");
    wait(project.save_as(file.path.clone(), false)).expect("save as");
    let status = project.status();
    assert_eq!(status.mode.path(), Some(file.path.as_path()));
    assert_eq!(status.http_count, 1);
    assert_eq!(status.ws_count, 1);
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
    assert_eq!(opened.http_total, 1);
    assert_eq!(opened.ws_total, 1);
    assert_eq!(opened.http.len(), 1);
    assert_eq!(opened.ws.len(), 1);
    assert_eq!(opened.http[0].id, http_id);
    assert_eq!(opened.http[0].status(), Some(200));
    assert_eq!(opened.workspace, workspace);
    assert_eq!(
        reopened.status().mode.path(),
        Some(file.path.as_path()),
        "opening switches the writer to the project file"
    );

    let persisted = reopened
        .get_http(http_id)
        .expect("read persisted HTTP record")
        .expect("persisted HTTP record");
    assert_eq!(
        persisted.response_body().expect("response body").data(),
        b"ok"
    );
}

#[test]
fn save_copy_leaves_the_source_open_and_creates_a_reopenable_file() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(&project, &[started(9, "GET"), finished(9, b"", b"copy")]);
    let http_id = ids[0];
    wait(project.flush()).expect("flush");

    let file = TempProject::new("save-copy");
    wait(project.save_copy(file.path.clone(), false)).expect("save copy");

    assert_eq!(project.status().mode, ProjectMode::Temporary);
    assert!(project.status().mode.path().is_none());
    let source = project
        .get_http(http_id)
        .expect("read source record")
        .expect("source record");
    assert_eq!(
        source.response_body().expect("response body").data(),
        b"copy"
    );
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open copy");
    assert_eq!(opened.http_total, 1);
    assert_eq!(opened.http[0].id, http_id);
}

#[test]
fn save_as_refuses_to_replace_an_existing_file() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let file = TempProject::new("existing");
    std::fs::write(&file.path, b"do not replace me").expect("write existing file");

    let error = wait(project.save_as(file.path.clone(), false)).expect_err("save must be refused");
    assert!(matches!(
        error,
        ProjectError::DestinationExists(ref path) if path == &file.path
    ));
    assert_eq!(
        std::fs::read(&file.path).expect("read existing file"),
        b"do not replace me"
    );
    assert_eq!(project.status().mode, ProjectMode::Temporary);
    assert!(project.get_http(1).expect("read project").is_none());
}

#[test]
fn save_as_replaces_an_existing_file_when_overwrite_is_allowed() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(
        &project,
        &[started(31, "GET"), finished(31, b"", b"replaced")],
    );
    let http_id = ids[0];
    wait(project.flush()).expect("flush");

    let file = TempProject::new("overwrite");
    std::fs::write(&file.path, b"old project bytes").expect("write existing file");

    wait(project.save_as(file.path.clone(), true)).expect("overwrite save");
    assert_eq!(project.status().mode.path(), Some(file.path.as_path()));
    assert_eq!(project.status().http_count, 1);
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
    assert_eq!(opened.http_total, 1);
    assert_eq!(opened.http[0].id, http_id);
    assert_eq!(
        opened.http[0]
            .response_body()
            .expect("response body")
            .data(),
        b"replaced"
    );
}

#[test]
fn save_as_can_write_to_the_active_project_path() {
    let project = ProjectHandle::temporary().expect("temporary project");
    capture_all(
        &project,
        &[started(32, "GET"), finished(32, b"", b"same path")],
    );
    let file = TempProject::new("same-path");
    wait(project.save_as(file.path.clone(), false)).expect("save as");

    wait(project.save_as(file.path.clone(), true)).expect("overwrite the open project");
    assert_eq!(project.status().mode.path(), Some(file.path.as_path()));
    assert_eq!(project.status().http_count, 1);
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
    assert_eq!(opened.http_total, 1);
    assert_eq!(
        opened.http[0]
            .response_body()
            .expect("response body")
            .data(),
        b"same path"
    );
}

#[test]
fn save_copy_can_replace_an_existing_file_without_switching() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(
        &project,
        &[started(33, "GET"), finished(33, b"", b"copy over")],
    );
    let http_id = ids[0];
    wait(project.flush()).expect("flush");

    let file = TempProject::new("copy-overwrite");
    std::fs::write(&file.path, b"old copy bytes").expect("write existing file");
    wait(project.save_copy(file.path.clone(), true)).expect("overwrite copy");

    assert_eq!(project.status().mode, ProjectMode::Temporary);
    assert!(project.status().mode.path().is_none());
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open copy");
    assert_eq!(opened.http_total, 1);
    assert_eq!(opened.http[0].id, http_id);
}

#[test]
fn open_rejects_invalid_files_without_switching_projects() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(&project, &[started(11, "GET"), finished(11, b"", b"kept")]);
    let http_id = ids[0];
    wait(project.flush()).expect("flush");

    let file = TempProject::new("junk");
    std::fs::write(&file.path, b"this is not a redb database").expect("write junk");
    wait(project.open(file.path.clone(), 100, 1024 * 1024)).expect_err("opening junk must fail");

    let status = project.status();
    assert_eq!(status.mode, ProjectMode::Temporary);
    assert_eq!(status.http_count, 1);
    let kept = project
        .get_http(http_id)
        .expect("read current project")
        .expect("current project still open");
    assert_eq!(kept.response_body().expect("response body").data(), b"kept");
}

#[test]
fn open_rejects_an_interrupted_creation() {
    let file = TempProject::new("incomplete");
    let meta = ProjectMeta::new(new_project_id());
    let db = super::super::backend::create_initializing(&file.path, &meta)
        .expect("create initializing project");
    drop(db);

    let project = ProjectHandle::temporary().expect("temporary project");
    let error = wait(project.open(file.path.clone(), 100, 1024 * 1024))
        .expect_err("incomplete projects must be rejected");
    assert!(matches!(
        error,
        ProjectError::IncompleteProject(ref path) if path == &file.path
    ));
    assert_eq!(project.status().mode, ProjectMode::Temporary);
}

#[test]
fn open_rejects_a_newer_schema_version() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let file = TempProject::new("future-schema");
    wait(project.save_as(file.path.clone(), false)).expect("save as");
    drop(project);

    // Bump the schema version behind the service's back.
    {
        let db = super::super::backend::open_file(&file.path).expect("open raw project");
        let tx = db.begin_write().expect("begin write");
        {
            let mut table = tx.open_table(schema::META).expect("meta table");
            table
                .insert(
                    schema::KEY_SCHEMA_VERSION,
                    (schema::SCHEMA_VERSION + 1).to_string().as_bytes(),
                )
                .expect("bump schema version");
        }
        tx.commit().expect("commit");
    }

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let error = wait(reopened.open(file.path.clone(), 100, 1024 * 1024))
        .expect_err("a newer schema must be rejected");
    assert!(matches!(
        error,
        ProjectError::FutureSchema { found, supported }
            if found == schema::SCHEMA_VERSION + 1 && supported == schema::SCHEMA_VERSION
    ));
    assert_eq!(reopened.status().mode, ProjectMode::Temporary);
}

#[test]
fn open_repairs_exchanges_left_in_flight() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(&project, &[started(21, "GET")]);
    let http_id = ids[0];
    wait(project.flush()).expect("flush");

    let file = TempProject::new("in-flight");
    wait(project.save_as(file.path.clone(), false)).expect("save as");
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
    assert_eq!(opened.http_total, 1);
    let item = &opened.http[0];
    assert_eq!(item.id, http_id);
    assert!(!item.is_in_flight());
    assert_eq!(
        item.error(),
        Some("application terminated before exchange completed")
    );
    assert_eq!(reopened.status().event_loss_count, 1);
}

#[test]
fn clearing_history_is_persisted() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(
        &project,
        &[
            started(31, "GET"),
            finished(31, b"", b"http"),
            ws_message(32, 2, b"ws"),
        ],
    );
    let http_id = ids[0];
    let ws_id = ids[2];
    wait(project.flush()).expect("flush");

    let http_receipt = wait(project.clear_http()).expect("clear HTTP history");
    assert_eq!(http_receipt.http_count, 0);
    assert!(project.get_http(http_id).expect("read HTTP").is_none());
    assert_eq!(project.status().http_count, 0);
    let meta = persisted_meta(&project);
    assert_eq!(meta.http_body_bytes, 0);
    assert_eq!(meta.ws_body_bytes, b"ws".len() as u64);

    let ws_receipt = wait(project.clear_ws()).expect("clear WebSocket history");
    assert_eq!(ws_receipt.ws_count, 0);
    assert!(project.get_ws(ws_id).expect("read WebSocket").is_none());
    assert_eq!(project.status().ws_count, 0);
    assert_eq!(persisted_meta(&project).ws_body_bytes, 0);

    let file = TempProject::new("cleared");
    wait(project.save_as(file.path.clone(), false)).expect("save as");
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open project");
    assert_eq!(opened.http_total, 0);
    assert_eq!(opened.ws_total, 0);
    assert!(opened.http.is_empty());
    assert!(opened.ws.is_empty());
}

#[test]
fn cleared_ids_are_not_reused() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let ids = capture_all(&project, &[started(41, "GET"), finished(41, b"", b"one")]);
    let first_id = ids[0];
    wait(project.flush()).expect("flush");
    wait(project.clear_http()).expect("clear HTTP history");

    let new_id = capture_all(&project, &[started(42, "GET"), finished(42, b"", b"two")])[0];
    assert!(
        new_id > first_id,
        "clearing a project must not reuse durable IDs"
    );
    wait(project.flush()).expect("flush");
    assert!(project.get_http(first_id).expect("read old ID").is_none());
    assert_eq!(
        project
            .get_http(new_id)
            .expect("read new HTTP record")
            .expect("new record")
            .response_body()
            .expect("response body")
            .data(),
        b"two"
    );
    assert_eq!(project.status().http_count, 1);
}

#[test]
fn workspace_is_persisted_without_switching_projects() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let mut workspace = WorkspaceState::default();
    workspace.scope.enabled = true;
    workspace.scope.hosts = vec!["example.com".to_string()];
    assert!(project.set_workspace(workspace.clone()));
    wait(project.flush()).expect("flush");

    let file = TempProject::new("workspace");
    wait(project.save_copy(file.path.clone(), false)).expect("save copy");
    drop(project);

    let reopened = ProjectHandle::temporary().expect("temporary project");
    let opened = wait(reopened.open(file.path.clone(), 100, 1024 * 1024)).expect("open copy");
    assert_eq!(opened.workspace, workspace);
}

#[test]
fn connect_requests_are_not_captured() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let outcome = project.capture(&started(70, "CONNECT"));
    assert!(outcome.event.is_none());

    wait(project.flush()).expect("flush");
    assert_eq!(project.status().http_count, 0);
}

#[test]
fn unmapped_response_events_are_returned_without_storage() {
    let project = ProjectHandle::temporary().expect("temporary project");

    let head = response_head(999, 200);
    let returned = project.capture(&head).event.expect("returned head");
    assert_eq!(returned.id(), 999);

    let finished = finished(999, b"", b"");
    let returned = project.capture(&finished).event.expect("returned finish");
    assert_eq!(returned.id(), 999);

    wait(project.flush()).expect("flush");
    let status = project.status();
    assert_eq!(status.http_count, 0);
    assert_eq!(status.event_loss_count, 0);
}

#[test]
fn a_stopped_writer_rejects_new_workspace_snapshots() {
    let project = ProjectHandle::temporary().expect("temporary project");
    project.shutdown();
    assert!(!project.set_workspace(WorkspaceState::default()));
}

#[test]
fn project_mode_reports_its_file_path() {
    assert_eq!(ProjectMode::Temporary.path(), None);
    let path = PathBuf::from("/tmp/example.perseid");
    assert_eq!(
        ProjectMode::Persistent { path: path.clone() }.path(),
        Some(path.as_path())
    );
}

#[test]
fn open_rejects_migrating_and_unknown_states() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let file = TempProject::new("bad-state");
    wait(project.save_as(file.path.clone(), false)).expect("save as");
    drop(project);

    let set_state = |state: &str| {
        let db = super::super::backend::open_file(&file.path).expect("open raw project");
        let tx = db.begin_write().expect("begin write");
        {
            let mut table = tx.open_table(schema::META).expect("meta table");
            table
                .insert(schema::KEY_STATE, state.as_bytes())
                .expect("set project state");
        }
        tx.commit().expect("commit");
    };

    set_state(schema::STATE_MIGRATING);
    let reopened = ProjectHandle::temporary().expect("temporary project");
    let error = wait(reopened.open(file.path.clone(), 100, 1024 * 1024))
        .expect_err("a migrating project must be rejected");
    assert!(
        matches!(error, ProjectError::InvalidProject(ref message) if message.contains("migrating"))
    );
    assert_eq!(reopened.status().mode, ProjectMode::Temporary);

    set_state("future-state");
    let error = wait(reopened.open(file.path.clone(), 100, 1024 * 1024))
        .expect_err("an unknown state must be rejected");
    assert!(
        matches!(error, ProjectError::InvalidProject(ref message) if message.contains("unknown project state"))
    );
    assert_eq!(reopened.status().mode, ProjectMode::Temporary);
}

fn persisted_meta(project: &ProjectHandle) -> ProjectMeta {
    let db = project.current_db().expect("current database");
    let read = db.begin_read().expect("begin read");
    super::super::schema::read_meta(&read).expect("read project metadata")
}

fn corrupt_project(file: &TempProject, mutate: impl FnOnce(&redb::WriteTransaction)) {
    let db = super::super::backend::open_file(&file.path).expect("open raw project");
    let tx = db.begin_write().expect("begin write");
    mutate(&tx);
    tx.commit().expect("commit corruption");
}

fn assert_snapshot_error(error: ProjectError, component: &str) {
    match error {
        ProjectError::Snapshot {
            component: actual,
            source,
        } => {
            assert_eq!(actual, component);
            assert!(
                matches!(*source, ProjectError::Encoding(_)),
                "unexpected snapshot source: {source}"
            );
        }
        other => panic!("expected a snapshot error, got {other}"),
    }
}

#[test]
fn failed_capture_batches_roll_back_records_and_metadata() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let first = capture_all(&project, &[started(1, "GET")])[0];
    wait(project.flush()).expect("flush");
    let before = persisted_meta(&project);

    // Corrupt the committed record so the next batch fails while applying a
    // response head, after adding another exchange in the same transaction.
    {
        let db = project.current_db().expect("current database");
        let tx = db.begin_write().expect("begin write");
        {
            tx.open_table(schema::HTTP_EXCHANGES)
                .expect("exchanges table")
                .insert(first, b"not json".as_slice())
                .expect("corrupt record");
        }
        tx.commit().expect("commit corruption");
    }

    let second = capture_all(&project, &[started(2, "GET"), response_head(1, 200)]);
    assert_eq!(second[1], first);
    let new_id = second[0];
    wait(project.flush()).expect("flush after the failed batch");

    let status = project.status();
    assert_eq!(status.http_count, before.http_count);
    assert_eq!(
        status.last_committed_sequence,
        before.last_committed_sequence
    );
    assert!(
        status.last_error.is_some(),
        "the failed batch must stay visible in the status bar"
    );

    let after = persisted_meta(&project);
    assert_eq!(after.http_count, before.http_count);
    assert_eq!(
        after.last_committed_sequence,
        before.last_committed_sequence
    );
    assert_eq!(after.next_http_id, before.next_http_id);
    let db = project.current_db().expect("current database");
    assert_eq!(
        super::super::queries::count_http(&db).expect("count HTTP records"),
        before.http_count,
        "the successful part of the batch must roll back with the transaction"
    );
    assert!(
        project
            .get_http(new_id)
            .expect("read rolled-back ID")
            .is_none(),
        "a record inserted before the failure must not remain visible"
    );
}

#[test]
fn temporary_retention_evicts_the_oldest_records_across_http_and_websocket() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(2, 0))
        .expect("bounded temporary project");
    let ids = capture_all(
        &project,
        &[
            started(1, "GET"),
            finished(1, b"old-req", b"old-resp"),
            started(2, "GET"),
            finished(2, b"new-req", b"new-resp"),
            ws_message(3, 1, b"ws"),
        ],
    );
    let old_http = ids[0];
    let new_http = ids[2];
    let ws_id = ids[4];
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.http_count, 1);
    assert_eq!(status.ws_count, 1);
    assert_eq!(status.retention_evictions, 1);
    assert!(
        project
            .get_http(old_http)
            .expect("read evicted HTTP record")
            .is_none()
    );
    assert!(
        project
            .get_http(new_http)
            .expect("read retained HTTP record")
            .is_some()
    );
    assert!(
        project
            .get_ws(ws_id)
            .expect("read retained WebSocket message")
            .is_some()
    );

    let meta = persisted_meta(&project);
    assert_eq!(meta.http_count, 1);
    assert_eq!(meta.ws_count, 1);
    assert_eq!(meta.retention_evictions, 1);
    assert_eq!(
        meta.http_body_bytes,
        (b"new-req".len() + b"new-resp".len()) as u64,
        "evicting a record must also release its retained body bytes"
    );
    assert_eq!(meta.ws_body_bytes, b"ws".len() as u64);
    assert_eq!(project.list_http(None, 10).expect("list HTTP").len(), 1);
    assert_eq!(project.list_ws(None, 10).expect("list WebSocket").len(), 1);
}

#[test]
fn temporary_retention_enforces_body_bytes_and_keeps_the_newest_record() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 8))
        .expect("byte-bounded temporary project");
    let ids = capture_all(
        &project,
        &[
            started(1, "GET"),
            finished(1, b"aaaa", b""),
            started(2, "GET"),
            finished(2, b"bbbb", b""),
            ws_message(3, 1, b"cccc"),
        ],
    );
    let old_http = ids[0];
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.http_count, 1);
    assert_eq!(status.ws_count, 1);
    assert_eq!(status.retention_evictions, 1);
    assert!(
        project
            .get_http(old_http)
            .expect("read evicted HTTP record")
            .is_none()
    );
    let meta = persisted_meta(&project);
    assert_eq!(meta.http_body_bytes, 4);
    assert_eq!(meta.ws_body_bytes, 4);

    // A single record larger than the whole budget is still retained.
    let oversized = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 2))
        .expect("tiny byte budget");
    let oversized_ids = capture_all(&oversized, &[started(1, "GET"), finished(1, b"12345", b"")]);
    wait(oversized.flush()).expect("flush oversized record");
    let status = oversized.status();
    assert_eq!(status.http_count, 1);
    assert_eq!(status.retention_evictions, 0);
    assert!(
        oversized
            .get_http(oversized_ids[0])
            .expect("read newest record")
            .is_some()
    );
    assert_eq!(persisted_meta(&oversized).http_body_bytes, 5);
}

#[test]
fn saved_projects_never_apply_temporary_retention_limits() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(1, 0))
        .expect("bounded temporary project");
    let file = TempProject::new("saved-retention");
    wait(project.save_as(file.path.clone(), false)).expect("save as");

    let ids = capture_all(
        &project,
        &[
            started(1, "GET"),
            finished(1, b"", b"one"),
            started(2, "GET"),
            finished(2, b"", b"two"),
            ws_message(3, 1, b"ws"),
        ],
    );
    wait(project.flush()).expect("flush");
    let status = project.status();
    assert_eq!(status.http_count, 2);
    assert_eq!(status.ws_count, 1);
    assert_eq!(status.retention_evictions, 0);
    assert!(
        project
            .get_http(ids[0])
            .expect("read oldest saved HTTP record")
            .is_some()
    );
    let meta = persisted_meta(&project);
    assert_eq!(meta.http_count, 2);
    assert_eq!(meta.ws_count, 1);
    assert_eq!(meta.retention_evictions, 0);
}

#[test]
fn saved_retention_is_applied_immediately_and_survives_reopen_and_copy() {
    let project = ProjectHandle::temporary().unwrap();
    let file = TempProject::new("persistent-limits");
    let copy = TempProject::new("persistent-limits-copy");
    wait(project.save_as(file.path.clone(), false)).unwrap();
    let ids = capture_all(
        &project,
        &[
            started(1, "GET"),
            finished(1, b"", b"one"),
            ws_message(2, 1, b"ws"),
            started(3, "GET"),
            finished(3, b"", b"three"),
            started(4, "GET"),
            finished(4, b"", b"four"),
        ],
    );
    wait(project.flush()).unwrap();
    wait(project.set_retention(RetentionLimits::new(2, 0))).unwrap();
    assert_eq!(project.status().http_count, 1);
    assert_eq!(project.status().ws_count, 1);
    assert!(project.get_http(ids[0]).unwrap().is_none());
    assert_eq!(project.status().retention_evictions, 2);
    wait(project.save_copy(copy.path.clone(), false)).unwrap();
    wait(project.new_project(10, 1024)).unwrap();
    wait(project.open(copy.path.clone(), 10, 1024)).unwrap();
    assert_eq!(project.status().retention, RetentionLimits::new(2, 0));
    capture_all(&project, &[started(10, "POST"), finished(10, b"", b"new")]);
    wait(project.flush()).unwrap();
    assert_eq!(project.status().http_count, 1);
    assert_eq!(project.status().ws_count, 1);
    assert!(project.get_http(ids[6]).unwrap().is_none());
}

#[test]
fn saved_body_retention_honours_zero_and_in_flight_exceptions() {
    let project = ProjectHandle::temporary().unwrap();
    let file = TempProject::new("persistent-byte-limits");
    wait(project.save_as(file.path.clone(), false)).unwrap();
    wait(project.set_retention(RetentionLimits::new(0, 4))).unwrap();
    let ids = capture_all(
        &project,
        &[
            started(1, "GET"),
            started(2, "GET"),
            finished(2, b"a", b"bb"),
            started(3, "GET"),
            finished(3, b"a", b"bbb"),
        ],
    );
    wait(project.flush()).unwrap();
    assert!(project.get_http(ids[0]).unwrap().is_some()); // in flight
    assert!(project.get_http(ids[1]).unwrap().is_none());
    assert_eq!(project.status().retained_body_bytes, 4);
    capture_all(&project, &[finished(1, b"", b"older")]);
    wait(project.flush()).unwrap();
    assert!(project.get_http(ids[0]).unwrap().is_none());
    wait(project.set_retention(RetentionLimits::new(0, 0))).unwrap();
    capture_all(
        &project,
        &[started(4, "GET"), finished(4, b"", b"unbounded")],
    );
    wait(project.flush()).unwrap();
    assert_eq!(project.status().http_count, 2);
}

#[test]
fn failed_retention_rolls_back_policy_counters_and_already_pruned_rows() {
    let (db, mut meta) = batch_project();
    apply_batch(
        &db,
        &mut meta,
        &[
            started(1, "GET"),
            finished(1, b"a", b"b"),
            started(2, "GET"),
            finished(2, b"a", b"b"),
            started(3, "GET"),
            finished(3, b"a", b"b"),
        ],
    )
    .unwrap();
    let tx = db.begin_write().unwrap();
    tx.open_table(schema::HTTP_EXCHANGES)
        .unwrap()
        .insert(2, b"bad".as_slice())
        .unwrap();
    tx.commit().unwrap();
    assert!(super::persistence::set_retention(&db, &mut meta, RetentionLimits::new(1, 0)).is_err());
    assert_eq!(meta.http_count, 3);
    assert_eq!(meta.retention_evictions, 0);
    assert_eq!(meta.retention_records, 0);
    let stored = schema::read_meta(&db.begin_read().unwrap()).unwrap();
    assert_eq!(stored.http_count, 3);
    assert_eq!(stored.http_body_bytes, 6);
    assert_eq!(stored.retention_records, 0);
    assert!(queries::get_http(&db, 1).unwrap().is_some());
    // Queued, not-yet-committed IDs must never be classified as evicted.
    assert_eq!(
        queries::pruned_ids(&db, vec![1, 4], vec![1]).unwrap(),
        (vec![], vec![])
    );
}

#[test]
fn failed_file_opens_preserve_identity_and_accept_further_captures() {
    let project = ProjectHandle::temporary().unwrap();
    let saved = ProjectHandle::temporary().unwrap();
    let locked = TempProject::new("locked-error");
    wait(saved.save_as(locked.path.clone(), false)).unwrap();
    let missing = TempProject::new("missing-error");
    let corrupt = TempProject::new("corrupt-error");
    std::fs::write(&corrupt.path, b"not a database").unwrap();
    let generation = project.status().generation;
    for path in [&missing.path, &locked.path, &corrupt.path] {
        let error = wait(project.open(path.clone(), 10, 1024)).unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(project.status().generation, generation);
        capture_all(
            &project,
            &[started(100 + project.status().http_count, "GET")],
        );
        wait(project.flush()).unwrap();
    }
    assert_eq!(project.status().http_count, 3);
    assert!(!missing.path.exists());
}

#[cfg(unix)]
#[test]
fn read_only_project_reports_actionable_error_without_switching() {
    use std::os::unix::fs::PermissionsExt;
    let file = TempProject::new("read-only");
    let project = ProjectHandle::temporary().unwrap();
    wait(project.save_copy(file.path.clone(), false)).unwrap();
    std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o400)).unwrap();
    let error = wait(project.open(file.path.clone(), 10, 1024)).unwrap_err();
    assert!(error.to_string().contains("writable"));
    assert_eq!(project.status().mode, ProjectMode::Temporary);
    std::fs::set_permissions(&file.path, std::fs::Permissions::from_mode(0o600)).unwrap();
    wait(project.open(file.path.clone(), 10, 1024)).unwrap();
}

#[test]
fn open_surfaces_corrupt_http_snapshots_and_preserves_the_current_project() {
    let source = ProjectHandle::temporary().expect("source project");
    let saved_id = capture_all(
        &source,
        &[started(1, "GET"), finished(1, b"", b"persisted")],
    )[0];
    wait(source.flush()).expect("flush");
    let file = TempProject::new("corrupt-http-snapshot");
    wait(source.save_as(file.path.clone(), false)).expect("save as");
    drop(source);
    corrupt_project(&file, |tx| {
        tx.open_table(schema::HTTP_EXCHANGES)
            .expect("exchanges table")
            .insert(saved_id, b"not json".as_slice())
            .expect("corrupt HTTP record");
    });

    let project = ProjectHandle::temporary().expect("current project");
    let kept = capture_all(
        &project,
        &[started(101, "GET"), finished(101, b"", b"kept")],
    )[0];
    wait(project.flush()).expect("flush current project");

    let error = wait(project.open(file.path.clone(), 100, 1024 * 1024))
        .expect_err("a corrupt HTTP snapshot must fail the open");
    assert_snapshot_error(error, "HTTP history");
    assert_eq!(project.status().mode, ProjectMode::Temporary);
    let current = project
        .get_http(kept)
        .expect("read current HTTP record")
        .expect("current project still present");
    assert_eq!(
        current.response_body().expect("response body").data(),
        b"kept"
    );
}

#[test]
fn open_surfaces_corrupt_websocket_snapshots_and_preserves_the_current_project() {
    let source = ProjectHandle::temporary().expect("source project");
    let saved_id = capture_all(&source, &[ws_message(1, 1, b"persisted")])[0];
    wait(source.flush()).expect("flush");
    let file = TempProject::new("corrupt-ws-snapshot");
    wait(source.save_as(file.path.clone(), false)).expect("save as");
    drop(source);
    corrupt_project(&file, |tx| {
        tx.open_table(schema::WS_MESSAGES)
            .expect("messages table")
            .insert(saved_id, b"not json".as_slice())
            .expect("corrupt WebSocket record");
    });

    let project = ProjectHandle::temporary().expect("current project");
    let kept = capture_all(&project, &[ws_message(101, 2, b"kept")])[0];
    wait(project.flush()).expect("flush current project");

    let error = wait(project.open(file.path.clone(), 100, 1024 * 1024))
        .expect_err("a corrupt WebSocket snapshot must fail the open");
    assert_snapshot_error(error, "WebSocket history");
    assert_eq!(project.status().mode, ProjectMode::Temporary);
    let current = project
        .get_ws(kept)
        .expect("read current WebSocket record")
        .expect("current project still present");
    assert_eq!(current.payload.data(), b"kept");
}

#[test]
fn open_surfaces_corrupt_workspace_state_and_preserves_the_current_project() {
    let source = ProjectHandle::temporary().expect("source project");
    let mut workspace = WorkspaceState::default();
    workspace.ui.http_filter = "saved-filter".to_string();
    assert!(source.set_workspace(workspace));
    wait(source.flush()).expect("flush");
    let file = TempProject::new("corrupt-workspace");
    wait(source.save_as(file.path.clone(), false)).expect("save as");
    drop(source);
    corrupt_project(&file, |tx| {
        tx.open_table(schema::WORKSPACE)
            .expect("workspace table")
            .insert(schema::WORKSPACE_KEY, b"not json".as_slice())
            .expect("corrupt workspace");
    });

    let project = ProjectHandle::temporary().expect("current project");
    let kept = capture_all(
        &project,
        &[started(101, "GET"), finished(101, b"", b"kept")],
    )[0];
    wait(project.flush()).expect("flush current project");

    let error = wait(project.open(file.path.clone(), 100, 1024 * 1024))
        .expect_err("corrupt workspace state must fail the open");
    assert_snapshot_error(error, "workspace state");
    assert_eq!(project.status().mode, ProjectMode::Temporary);
    assert!(
        project
            .get_http(kept)
            .expect("read current HTTP record")
            .is_some()
    );
}

#[test]
fn persistent_writers_start_without_temporary_retention() {
    let meta = ProjectMeta::new(new_project_id());
    let db = super::super::backend::create_memory(&meta).expect("memory project");
    let path = PathBuf::from("/tmp/persistent-writer.perseid");
    let project = ProjectHandle::from_database(
        db,
        ProjectMode::Persistent { path: path.clone() },
        RetentionLimits::new(1, 0),
    )
    .expect("persistent writer");

    let ids = capture_all(
        &project,
        &[
            started(1, "GET"),
            finished(1, b"", b"one"),
            started(2, "GET"),
            finished(2, b"", b"two"),
        ],
    );
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.mode, ProjectMode::Persistent { path });
    assert_eq!(
        status.http_count, 2,
        "a persistent writer never applies the temporary retention limits"
    );
    assert!(project.get_http(ids[0]).expect("read oldest").is_some());
}

#[test]
fn capture_batch_ignores_duplicates_and_counts_unmatched_events() {
    let (db, mut meta) = batch_project();
    apply_batch(
        &db,
        &mut meta,
        &[
            started(1, "GET"),
            started(1, "GET"),       // a replayed Started must not add a second row
            response_head(999, 200), // no stored exchange: counted as an event loss
            finished(999, b"", b""), // no stored exchange: counted as an event loss
            ws_message(2, 7, b"payload"),
            ws_message(2, 7, b"payload"), // a replayed message must not be stored twice
        ],
    )
    .expect("apply batch");

    assert_eq!(meta.http_count, 1);
    assert_eq!(meta.ws_count, 1);
    assert_eq!(meta.event_loss_count, 2);
    assert_eq!(meta.http_body_bytes, 0);
    assert_eq!(meta.ws_body_bytes, b"payload".len() as u64);

    let read = db.begin_read().expect("begin read");
    let http = read.open_table(schema::HTTP_EXCHANGES).expect("exchanges");
    assert!(http.get(1u64).expect("read exchange").is_some());
    let ws = read.open_table(schema::WS_MESSAGES).expect("messages");
    assert!(ws.get(2u64).expect("read message").is_some());
}

#[test]
fn capture_batch_replaces_response_heads_and_accounts_for_body_records() {
    let (db, mut meta) = batch_project();
    apply_batch(&db, &mut meta, &[started(1, "GET"), response_head(1, 200)]).expect("first batch");
    apply_batch(&db, &mut meta, &[response_head(1, 201)]).expect("replacement head");

    // Another exchange keeps 2 counted body bytes while a stale, uncounted
    // body row appears for exchange 1. Replacing the stale row must not
    // subtract from exchange 2's bytes.
    apply_batch(
        &db,
        &mut meta,
        &[started(2, "GET"), finished(2, b"22", b"")],
    )
    .expect("second exchange");
    assert_eq!(meta.http_body_bytes, 2);
    {
        let tx = db.begin_write().expect("begin write");
        tx.open_table(schema::HTTP_BODIES)
            .expect("bodies")
            .insert((1u64, 0u8), b"stale".as_slice())
            .expect("stale body");
        tx.commit().expect("commit");
    }

    apply_batch(&db, &mut meta, &[finished(1, b"one", b"one")]).expect("finished");
    assert_eq!(meta.http_body_bytes, 8);
    apply_batch(&db, &mut meta, &[finished(1, b"22", b"22")]).expect("replaced bodies");
    assert_eq!(meta.http_body_bytes, 6);
    apply_batch(&db, &mut meta, &[finished(1, b"", b"")]).expect("released bodies");
    assert_eq!(
        meta.http_body_bytes, 2,
        "only exchange 2's request body stays counted"
    );

    let read = db.begin_read().expect("begin read");
    let statuses = read.open_table(schema::HTTP_BY_STATUS).expect("statuses");
    assert!(statuses.get((200u16, 1u64)).expect("old status").is_none());
    assert!(statuses.get((201u16, 1u64)).expect("new status").is_some());
    let bodies = read.open_table(schema::HTTP_BODIES).expect("bodies");
    assert!(bodies.get((1u64, 0u8)).expect("request body").is_none());
    assert!(bodies.get((1u64, 1u8)).expect("response body").is_none());
    assert_eq!(
        bodies
            .get((2u64, 0u8))
            .expect("second request body")
            .expect("stored body")
            .value(),
        b"22"
    );
}

#[test]
fn retention_releases_only_the_counted_bytes_of_evicted_records() {
    let (db, mut meta) = batch_project();
    let limits = Some(RetentionLimits::new(1, 0));

    super::persistence::apply_capture_batch(
        &db,
        &mut meta,
        limits,
        &[started(1, "GET"), finished(1, b"one", b"")],
    )
    .expect("first exchange");
    // Exchange 1 never had a response body, so this row is stale and outside
    // the retained-byte total. Evicting the exchange must not subtract it.
    {
        let tx = db.begin_write().expect("begin write");
        tx.open_table(schema::HTTP_BODIES)
            .expect("bodies")
            .insert((1u64, 1u8), b"stale".as_slice())
            .expect("stale body");
        tx.commit().expect("commit");
    }

    super::persistence::apply_capture_batch(
        &db,
        &mut meta,
        limits,
        &[started(2, "GET"), finished(2, b"22", b"")],
    )
    .expect("second exchange");

    assert_eq!(meta.http_count, 1);
    assert_eq!(meta.retention_evictions, 1);
    assert_eq!(
        meta.http_body_bytes, 2,
        "eviction releases the counted request body of exchange 1 only"
    );
    assert_eq!(meta.ws_body_bytes, 0);
}

#[test]
fn capture_batch_accounts_for_websocket_payloads() {
    let (db, mut meta) = batch_project();
    // A payload row without a message is stale; capturing the message replaces
    // it and only the new payload is counted.
    insert_stale_ws_payload(&db, 2, b"stale");
    apply_batch(&db, &mut meta, &[ws_message(2, 7, b"xy")]).expect("first message");
    assert_eq!(meta.ws_body_bytes, 2);

    // Capturing a second message drops its stale row without releasing the
    // unrelated payload of message 2 from the retained-byte total.
    insert_stale_ws_payload(&db, 3, b"stale");
    apply_batch(&db, &mut meta, &[ws_message(3, 7, b"")]).expect("empty payload");
    assert_eq!(meta.ws_body_bytes, 2);

    let read = db.begin_read().expect("begin read");
    let payloads = read.open_table(schema::WS_PAYLOADS).expect("payloads");
    assert_eq!(
        payloads
            .get(2u64)
            .expect("payload 2")
            .expect("stored payload")
            .value(),
        b"xy"
    );
    assert!(payloads.get(3u64).expect("payload 3").is_none());
}

#[test]
fn finished_outcomes_are_persisted_with_their_terminal_phase() {
    let project = ProjectHandle::temporary().expect("temporary project");
    let cases = [
        (
            11u64,
            ExchangeResult::Failed {
                error: "boom".to_string(),
            },
        ),
        (12, ExchangeResult::Dropped),
        (
            13,
            ExchangeResult::Tunnel {
                client_to_server: 3,
                server_to_client: 4,
            },
        ),
        (14, ExchangeResult::InterceptedTls { requests: 1 }),
    ];

    let mut ids = Vec::new();
    for (wire, result) in cases {
        let id = project
            .capture(&started(wire, "GET"))
            .event
            .expect("normalized start")
            .id();
        project.capture(&finished_with(
            wire,
            CapturedBody::from_bytes(Vec::new()),
            result,
        ));
        ids.push(id);
    }
    wait(project.flush()).expect("flush");

    let failed = project
        .get_http(ids[0])
        .expect("read failed exchange")
        .expect("stored failed exchange");
    assert_eq!(failed.error(), Some("boom"));
    assert!(!failed.is_in_flight());
    for id in &ids[1..] {
        let item = project
            .get_http(*id)
            .expect("read exchange")
            .expect("stored exchange");
        assert!(!item.is_in_flight());
        assert_eq!(item.error(), None);
        assert_eq!(item.duration(), Some(Duration::from_millis(7)));
    }
    let meta = persisted_meta(&project);
    assert_eq!(meta.http_count, 4);
    assert_eq!(meta.event_loss_count, 0);
}

#[test]
fn zero_retention_limits_disable_eviction() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(0, 0))
        .expect("unbounded project");
    let ids = capture_all(
        &project,
        &[
            started(1, "GET"),
            finished(1, b"body", b"body"),
            ws_message(2, 7, b"ws"),
        ],
    );
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.http_count, 1);
    assert_eq!(status.ws_count, 1);
    assert_eq!(status.retention_evictions, 0);
    assert!(project.get_http(ids[0]).expect("read HTTP").is_some());
    assert!(project.get_ws(ids[2]).expect("read WebSocket").is_some());
}

#[test]
fn retention_does_not_evict_a_newest_record_to_break_a_timestamp_tie() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(1, 0))
        .expect("bounded project");
    // The WebSocket message and the HTTP exchange start in the same
    // millisecond, and the HTTP exchange arrived later. With a record limit of
    // one, neither stream's newest record may be evicted, so the recent HTTP
    // exchange is not dropped in favour of the older WebSocket message.
    let ws = project
        .capture(&ws_message_at(1, 10, 7, b"first"))
        .event
        .expect("normalized WebSocket message");
    let http = capture_all(
        &project,
        &[started_at(2, 10, "GET"), finished(2, b"", b"done")],
    )[0];
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.retention_evictions, 0);
    assert_eq!(status.event_loss_count, 0);
    assert_eq!(status.http_count, 1);
    assert_eq!(status.ws_count, 1);
    assert!(project.get_http(http).expect("read HTTP").is_some());
    assert!(project.get_ws(ws.id()).expect("read WebSocket").is_some());
}

#[test]
fn retention_skips_in_flight_exchanges_instead_of_losing_their_completion() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(2, 0))
        .expect("bounded project");
    let ids = capture_all(
        &project,
        &[
            started_at(1, 10, "GET"),
            started_at(2, 20, "GET"),
            finished(2, b"", b"two"),
            started_at(3, 30, "GET"),
            finished(3, b"", b"three"),
        ],
    );
    let in_flight = ids[0];
    let evicted = ids[1];
    let newest = ids[3];
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.retention_evictions, 1);
    assert_eq!(status.event_loss_count, 0);
    assert!(project.get_http(in_flight).expect("read").is_some());
    assert!(
        project.get_http(evicted).expect("read evicted").is_none(),
        "the older in-flight exchange is kept, so the next-oldest one is evicted instead"
    );
    assert!(project.get_http(newest).expect("read newest").is_some());

    // The completion still lands on the retained exchange instead of being
    // counted as a lost capture.
    project.capture(&finished(1, b"", b"done"));
    wait(project.flush()).expect("flush completion");
    let item = project
        .get_http(in_flight)
        .expect("read completed")
        .expect("stored completed exchange");
    assert!(!item.is_in_flight());
    assert_eq!(project.status().event_loss_count, 0);
}

#[test]
fn retention_evicts_websocket_messages_and_prunes_unused_connections() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(2, 0))
        .expect("bounded project");
    let ids = capture_all(
        &project,
        &[
            ws_message_at(1, 10, 1, b"first"),
            ws_message_at(2, 20, 2, b"second"),
            ws_message_at(3, 30, 2, b"third"),
        ],
    );
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.ws_count, 2);
    assert_eq!(status.retention_evictions, 1);
    assert!(project.get_ws(ids[0]).expect("read evicted").is_none());
    assert!(project.get_ws(ids[1]).expect("read retained").is_some());
    assert!(project.get_ws(ids[2]).expect("read newest").is_some());

    let db = project.current_db().expect("current database");
    let read = db.begin_read().expect("begin read");
    let connections = read
        .open_table(schema::WS_CONNECTIONS)
        .expect("connections");
    assert!(
        connections.get(1u64).expect("read pruned").is_none(),
        "a connection without messages is pruned with its last message"
    );
    assert!(connections.get(2u64).expect("read used").is_some());
}

#[test]
fn retention_discards_dangling_time_index_entries() {
    let project = ProjectHandle::temporary_with_retention(RetentionLimits::new(1, 0))
        .expect("bounded project");
    let first = capture_all(
        &project,
        &[
            started_at(1, 10, "GET"),
            response_head(1, 200),
            finished(1, b"", b"one"),
        ],
    )[0];
    let second = capture_all(
        &project,
        &[
            started_at(2, 20, "GET"),
            response_head(2, 201),
            finished(2, b"", b"two"),
        ],
    )[0];
    wait(project.flush()).expect("flush");
    assert!(project.get_http(first).expect("read").is_none());
    assert!(project.get_http(second).expect("read").is_some());

    // A crashed index update can leave an entry whose record is gone; it must
    // be dropped without counting as a record or an eviction.
    {
        let db = project.current_db().expect("current database");
        let tx = db.begin_write().expect("begin write");
        tx.open_table(schema::HTTP_BY_TIME)
            .expect("by time")
            .insert((5u64, 999u64), ())
            .expect("dangling entry");
        tx.commit().expect("commit");
    }

    let third = capture_all(
        &project,
        &[
            started_at(3, 30, "GET"),
            response_head(3, 202),
            finished(3, b"", b"three"),
        ],
    )[0];
    wait(project.flush()).expect("flush");

    let status = project.status();
    assert_eq!(status.http_count, 1);
    assert_eq!(status.retention_evictions, 2);
    assert!(project.get_http(second).expect("read evicted").is_none());
    assert!(project.get_http(third).expect("read newest").is_some());
    assert_eq!(persisted_meta(&project).http_count, 1);

    let db = project.current_db().expect("current database");
    let read = db.begin_read().expect("begin read");
    let by_time = read.open_table(schema::HTTP_BY_TIME).expect("by time");
    assert!(
        by_time
            .get((5u64, 999u64))
            .expect("read dangling")
            .is_none()
    );
    let by_host = read.open_table(schema::HTTP_BY_HOST).expect("by host");
    assert!(
        by_host
            .get(("example.com".to_string(), second))
            .expect("host index")
            .is_none(),
        "the evicted exchange is removed from the host index"
    );
    let by_status = read.open_table(schema::HTTP_BY_STATUS).expect("by status");
    assert!(
        by_status
            .get((201u16, second))
            .expect("status index")
            .is_none(),
        "the evicted exchange is removed from the status index"
    );
}
