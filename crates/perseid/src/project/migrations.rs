//! Forward-only, atomic schema upgrades. No migration exposes an intermediate
//! state: redb rolls back the entire transaction if validation or commit fails.

use redb::{Database, ReadableTable};

use super::{
    ProjectError, Result, codec,
    records::{HttpExchangeRecord, WsMessageRecord},
    schema::{self, ProjectMeta},
};

pub(super) fn migrate(db: &Database, meta: &mut ProjectMeta) -> Result<()> {
    match meta.schema_version {
        schema::SCHEMA_VERSION => return Ok(()),
        1 => {}
        version => {
            return Err(ProjectError::InvalidProject(format!(
                "no migration from schema {version}"
            )));
        }
    }
    let mut next = meta.clone();
    let mut tx = db.begin_write()?;
    tx.set_durability(redb::Durability::Immediate)?;
    // Early v1 files did not track aggregate retained-body bytes. Recompute
    // rather than applying a new retention policy to incomplete counters.
    next.http_count = 0;
    next.ws_count = 0;
    next.http_body_bytes = 0;
    next.ws_body_bytes = 0;
    for entry in tx.open_table(schema::HTTP_EXCHANGES)?.iter()? {
        let (_, value) = entry?;
        let record: HttpExchangeRecord = codec::decode(value.value())?;
        if record.record_version != schema::RECORD_VERSION {
            return Err(ProjectError::UnsupportedRecord {
                found: record.record_version,
                supported: schema::RECORD_VERSION,
            });
        }
        next.http_count += 1;
        next.http_body_bytes = next
            .http_body_bytes
            .saturating_add(record.request_body.stored_len)
            .saturating_add(record.response_body.stored_len);
    }
    for entry in tx.open_table(schema::WS_MESSAGES)?.iter()? {
        let (_, value) = entry?;
        let record: WsMessageRecord = codec::decode(value.value())?;
        if record.record_version != schema::RECORD_VERSION {
            return Err(ProjectError::UnsupportedRecord {
                found: record.record_version,
                supported: schema::RECORD_VERSION,
            });
        }
        next.ws_count += 1;
        next.ws_body_bytes = next.ws_body_bytes.saturating_add(record.payload.stored_len);
    }
    next.retention_records = 0;
    next.retention_bytes = 0;
    {
        let mut table = tx.open_table(schema::META)?;
        schema::write_counters(&mut table, &next)?;
        table.insert(
            schema::KEY_LAST_WRITER_VERSION,
            env!("CARGO_PKG_VERSION").as_bytes(),
        )?;
        table.insert(
            schema::KEY_SCHEMA_VERSION,
            schema::SCHEMA_VERSION.to_string().as_bytes(),
        )?;
    }
    tx.commit()?;
    next.schema_version = schema::SCHEMA_VERSION;
    *meta = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use redb::ReadableDatabase;

    fn fixture() -> (Database, ProjectMeta) {
        let mut meta = ProjectMeta::new("v1-fixture".to_string());
        meta.schema_version = 1;
        meta.next_http_id = 8;
        meta.next_http_number = 4;
        meta.next_ws_id = 3;
        meta.next_ws_number = 2;
        meta.next_ws_connection_id = 2;
        let db = super::super::backend::create_memory(&meta).unwrap();
        let tx = db.begin_write().unwrap();
        {
            let mut table = tx.open_table(schema::META).unwrap();
            table.remove(schema::KEY_RETENTION_RECORDS).unwrap();
            table.remove(schema::KEY_RETENTION_BYTES).unwrap();
            // Earliest v1 files did not store body accounting.
            table.remove(schema::KEY_HTTP_BODY_BYTES).unwrap();
        }
        tx.open_table(schema::HTTP_EXCHANGES)
            .unwrap()
            .insert(
                7,
                include_bytes!("../../tests/fixtures/project-v1.json").as_slice(),
            )
            .unwrap();
        tx.open_table(schema::HTTP_BODIES)
            .unwrap()
            .insert((7, 0), b"req".as_slice())
            .unwrap();
        tx.open_table(schema::HTTP_BODIES)
            .unwrap()
            .insert((7, 1), &[0, 1, 2, 255][..])
            .unwrap();
        tx.open_table(schema::HTTP_BY_TIME)
            .unwrap()
            .insert((1700000000000, 7), ())
            .unwrap();
        tx.open_table(schema::HTTP_BY_HOST)
            .unwrap()
            .insert(("example.test".to_string(), 7), ())
            .unwrap();
        tx.open_table(schema::HTTP_BY_STATUS)
            .unwrap()
            .insert((200, 7), ())
            .unwrap();
        tx.open_table(schema::WORKSPACE)
            .unwrap()
            .insert(
                schema::WORKSPACE_KEY,
                br#"{"inspector":{"notes":[{"id":7,"text":"legacy note"}]}}"#.as_slice(),
            )
            .unwrap();
        tx.open_table(schema::WS_MESSAGES)
            .unwrap()
            .insert(
                2,
                include_bytes!("../../tests/fixtures/project-v1-ws.json").as_slice(),
            )
            .unwrap();
        tx.open_table(schema::WS_PAYLOADS)
            .unwrap()
            .insert(2, &[255, 0, 42][..])
            .unwrap();
        tx.open_table(schema::WS_CONNECTIONS).unwrap().insert(1, br#"{"record_version":1,"id":1,"url":"wss://example.test/socket","host":"example.test","tls":true,"listener_port":8080,"protocol":null}"#.as_slice()).unwrap();
        tx.open_table(schema::WS_BY_TIME)
            .unwrap()
            .insert((1700000000001, 2), ())
            .unwrap();
        tx.open_table(schema::WS_BY_CONNECTION)
            .unwrap()
            .insert((1, 2), ())
            .unwrap();
        tx.commit().unwrap();
        (db, meta)
    }

    #[test]
    fn v1_fixture_migrates_without_losing_records_bodies_notes_or_identity() {
        let (db, mut meta) = fixture();
        migrate(&db, &mut meta).unwrap();
        let stored = schema::read_meta(&db.begin_read().unwrap()).unwrap();
        assert_eq!(stored.schema_version, 2);
        assert_eq!(stored.project_id, "v1-fixture");
        assert_eq!(stored.next_http_id, 8);
        assert_eq!(stored.next_http_number, 4);
        assert_eq!(stored.http_body_bytes, 7);
        assert_eq!(stored.http_count, 1);
        assert_eq!(stored.ws_count, 1);
        assert_eq!(stored.ws_body_bytes, 3);
        assert_eq!(
            super::super::queries::get_ws(&db, 2)
                .unwrap()
                .unwrap()
                .payload
                .data(),
            &[255, 0, 42]
        );
        assert_eq!(stored.retention_records, 0);
        let item = super::super::queries::get_http(&db, 7).unwrap().unwrap();
        assert_eq!(item.response_body().unwrap().data(), &[0, 1, 2, 255]);
        let read = db.begin_read().unwrap();
        let table = read.open_table(schema::WORKSPACE).unwrap();
        let workspace: super::super::records::WorkspaceState =
            codec::decode(table.get(schema::WORKSPACE_KEY).unwrap().unwrap().value()).unwrap();
        assert_eq!(workspace.inspector.notes[0].text, "legacy note");
        migrate(&db, &mut meta).unwrap(); // already-current is a no-op
    }

    #[test]
    fn malformed_record_rolls_back_the_entire_migration() {
        let (db, mut meta) = fixture();
        let tx = db.begin_write().unwrap();
        tx.open_table(schema::HTTP_EXCHANGES)
            .unwrap()
            .insert(8, b"invalid".as_slice())
            .unwrap();
        tx.commit().unwrap();
        assert!(migrate(&db, &mut meta).is_err());
        assert_eq!(meta.schema_version, 1);
        let read = db.begin_read().unwrap();
        let table = read.open_table(schema::META).unwrap();
        assert_eq!(
            table
                .get(schema::KEY_SCHEMA_VERSION)
                .unwrap()
                .unwrap()
                .value(),
            b"1"
        );
        assert!(table.get(schema::KEY_RETENTION_RECORDS).unwrap().is_none());
    }

    fn wait<T>(rx: std::sync::mpsc::Receiver<Result<T>>) -> Result<T> {
        rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap()
    }

    #[test]
    fn opening_a_v1_file_migrates_and_reopening_preserves_notes_and_binary_payloads() {
        let (db, meta) = fixture();
        let path =
            std::env::temp_dir().join(format!("perseid-migrate-{}.perseid", uuid::Uuid::new_v4()));
        drop(super::super::transfer::copy_to_file(&db, &path, &meta, false).unwrap());
        let project = super::super::ProjectHandle::temporary().unwrap();
        for _ in 0..2 {
            let opened = wait(project.open(path.clone(), 10, 1024)).unwrap();
            assert_eq!(opened.http[0].id, 7);
            assert_eq!(opened.http[0].number, 3);
            assert_eq!(opened.http[0].request_body.data(), b"req");
            assert_eq!(opened.ws[0].payload.data(), &[255, 0, 42]);
            assert_eq!(opened.workspace.inspector.notes[0].text, "legacy note");
            assert_eq!(project.status().retained_body_bytes, 10);
            wait(project.new_project(10, 1024)).unwrap();
        }
        let reopened = super::super::backend::open_file(&path).unwrap();
        let meta = schema::read_meta(&reopened.begin_read().unwrap()).unwrap();
        assert_eq!(meta.schema_version, 2);
        assert_eq!(meta.project_id, "v1-fixture");
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn invalid_legacy_and_future_files_are_rejected_without_upgrading_or_switching() {
        for case in [
            "future",
            "metadata",
            "missing-table",
            "initializing",
            "migrating",
            "record",
        ] {
            let (db, meta) = fixture();
            let path = std::env::temp_dir().join(format!(
                "perseid-migrate-{case}-{}.perseid",
                uuid::Uuid::new_v4()
            ));
            let file = super::super::transfer::copy_to_file(&db, &path, &meta, false).unwrap();
            let tx = file.begin_write().unwrap();
            match case {
                "future" => {
                    tx.open_table(schema::META)
                        .unwrap()
                        .insert(schema::KEY_SCHEMA_VERSION, b"999".as_slice())
                        .unwrap();
                }
                "metadata" => {
                    tx.open_table(schema::META)
                        .unwrap()
                        .insert(schema::KEY_NEXT_HTTP_ID, b"bad".as_slice())
                        .unwrap();
                }
                "missing-table" => {
                    tx.delete_table(schema::WS_MESSAGES).unwrap();
                }
                "initializing" | "migrating" => {
                    tx.open_table(schema::META)
                        .unwrap()
                        .insert(schema::KEY_STATE, case.as_bytes())
                        .unwrap();
                }
                _ => {
                    tx.open_table(schema::WS_MESSAGES)
                        .unwrap()
                        .insert(2, b"bad".as_slice())
                        .unwrap();
                }
            }
            tx.commit().unwrap();
            drop(file);
            let project = super::super::ProjectHandle::temporary().unwrap();
            let generation = project.status().generation;
            assert!(
                wait(project.open(path.clone(), 10, 1024)).is_err(),
                "{case}"
            );
            assert_eq!(project.status().generation, generation);
            let file = super::super::backend::open_file(&path).unwrap();
            let read = file.begin_read().unwrap();
            let table = read.open_table(schema::META).unwrap();
            assert_eq!(
                table
                    .get(schema::KEY_SCHEMA_VERSION)
                    .unwrap()
                    .unwrap()
                    .value(),
                if case == "future" {
                    b"999".as_slice()
                } else {
                    b"1".as_slice()
                }
            );
            drop(table);
            drop(read);
            drop(file);
            std::fs::remove_file(path).unwrap();
        }
    }
}
