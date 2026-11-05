//! Body-free history queries shared by temporary and disk-backed projects.
//!
//! Pages contain metadata only. Reads use short transactions, and filtering
//! precedes pagination. Legacy records without summaries are decoded one at a
//! time; newly captured records never need body reads to populate the table.

use std::ops::Bound;
use std::sync::atomic::{AtomicBool, Ordering};

use redb::ReadableDatabase;

use super::records::{HttpExchangeRecord, WsMessageRecord};
use super::{ProjectSnapshot, Result, codec, queries, schema};
use crate::history::HistoryItem;
use crate::websocket_history::WebSocketMessageItem;

#[cfg(test)]
mod summary_tests {
    use super::*;
    use crate::proxy::{CapturedBody, RequestInfo, ResponseHead};
    use std::io::Write;

    #[test]
    fn borrowed_summary_counts_forms_and_decodes_html_without_changing_capture_bytes() {
        let request = RequestInfo {
            method: "POST".into(),
            uri: "http://example.test/?q=1".into(),
            scheme: "http".into(),
            host: "example.test".into(),
            path: "/?q=1".into(),
            version: "HTTP/1.1".into(),
            headers: vec![(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
        };
        let request_body = CapturedBody::from_bytes(b"a=1&b=2".to_vec());
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(b"<title>Borrowed &amp; decoded</title>")
            .unwrap();
        let compressed = encoder.finish().unwrap();
        let response_body = CapturedBody::from_bytes(compressed.clone());
        let response = ResponseHead {
            version: "HTTP/1.1".into(),
            status: 200,
            reason: "OK".into(),
            headers: vec![
                ("content-type".into(), "text/html".into()),
                ("content-encoding".into(), "gzip".into()),
            ],
        };
        let summary = HttpSummary::from_parts(
            &request,
            &request_body,
            Some(&response),
            Some(&response_body),
        );
        assert_eq!(summary.parameters, 3);
        assert_eq!(summary.title, "Borrowed & decoded");
        assert_eq!(response_body.data(), compressed);
        assert!(
            HttpSummary::from_parts(&request, &request_body, None, None)
                .title
                .is_empty()
        );
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct HttpSummary {
    pub parameters: usize,
    pub title: String,
}

impl HttpSummary {
    pub(crate) fn from_item(item: &HistoryItem) -> Self {
        Self::from_parts(
            &item.request,
            &item.request_body,
            item.response.as_ref(),
            item.response_body(),
        )
    }

    pub(crate) fn from_parts(
        request: &crate::proxy::RequestInfo,
        request_body: &crate::proxy::CapturedBody,
        response: Option<&crate::proxy::ResponseHead>,
        response_body: Option<&crate::proxy::CapturedBody>,
    ) -> Self {
        let parameters = crate::site_map::request_parameter_count(request, request_body).count;
        let html = response.is_some_and(|head| {
            head.headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                .is_some_and(|(_, value)| {
                    crate::site_map::mime_label(value.split(';').next().unwrap_or(value).trim())
                        == "HTML"
                })
        });
        let title = if html
            && response_body.is_some_and(|body| body.is_complete() || body.is_truncated())
        {
            crate::site_map::html_title(&crate::view::response_body_for_search_from_parts(
                response,
                response_body,
            ))
            .unwrap_or_default()
        } else {
            String::new()
        };
        Self { parameters, title }
    }
}

#[derive(Clone)]
pub(crate) struct HttpHistoryRow {
    pub item: HistoryItem,
    pub summary: HttpSummary,
}

pub(crate) struct HistoryPage<T> {
    pub offset: usize,
    pub rows: Vec<T>,
    pub total: usize,
    pub selected_index: Option<usize>,
    pub selected_id: Option<u64>,
}

impl<T> HistoryPage<T> {
    fn new(offset: usize) -> Self {
        Self {
            offset,
            rows: Vec::new(),
            total: 0,
            selected_index: None,
            selected_id: None,
        }
    }

    fn push(&mut self, row: T, id: u64, selected: Option<u64>, limit: usize) {
        if selected == Some(id) {
            self.selected_index = Some(self.total);
        }
        if self.total >= self.offset && self.rows.len() < limit {
            self.rows.push(row);
        }
        self.total += 1;
    }
}

impl ProjectSnapshot {
    pub(crate) fn http_history_page(
        &self,
        offset: usize,
        limit: usize,
        selected: Option<u64>,
        cancel: &AtomicBool,
        mut matches: impl FnMut(&HttpHistoryRow) -> bool,
    ) -> Result<HistoryPage<HttpHistoryRow>> {
        let mut page = HistoryPage::new(offset);
        page.selected_id = selected;
        let upper = {
            let tx = self.db.begin_read()?;
            let index = tx.open_table(schema::HTTP_BY_TIME)?;
            index
                .range::<(u64, u64)>(..)?
                .next_back()
                .transpose()?
                .map(|(key, _)| key.value())
        };
        let Some(upper) = upper else { return Ok(page) };
        let mut after = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let tx = self.db.begin_read()?;
            let index = tx.open_table(schema::HTTP_BY_TIME)?;
            let records = tx.open_table(schema::HTTP_EXCHANGES)?;
            let start = after.map_or(Bound::Unbounded, Bound::Excluded);
            let keys: Vec<_> = index
                .range((start, Bound::Included(upper)))?
                .take(256)
                .map(|entry| entry.map(|(key, _)| key.value()))
                .collect::<std::result::Result<_, _>>()?;
            for (_, id) in &keys {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let Some(value) = records.get(*id)? else {
                    continue;
                };
                let record: HttpExchangeRecord = codec::decode(value.value())?;
                queries::ensure_valid_record_version(record.record_version)?;
                let summary = match &record.summary {
                    Some(summary) => summary.clone(),
                    None => {
                        let (request, response) = queries::read_http_bodies(&tx, *id)?;
                        HttpSummary::from_item(&record.to_history_item(request, response))
                    }
                };
                let row = HttpHistoryRow {
                    item: record.to_history_item(Vec::new(), Vec::new()),
                    summary,
                };
                if matches(&row) {
                    page.push(row, *id, selected, limit.min(1000));
                }
            }
            after = keys.last().copied();
            if keys.len() < 256 {
                break;
            }
        }
        Ok(page)
    }

    pub(crate) fn ws_history_page(
        &self,
        offset: usize,
        limit: usize,
        selected: Option<u64>,
        cancel: &AtomicBool,
        mut matches: impl FnMut(&WebSocketMessageItem) -> bool,
    ) -> Result<HistoryPage<WebSocketMessageItem>> {
        let mut page = HistoryPage::new(offset);
        page.selected_id = selected;
        let upper = {
            let tx = self.db.begin_read()?;
            let index = tx.open_table(schema::WS_BY_TIME)?;
            index
                .range::<(u64, u64)>(..)?
                .next_back()
                .transpose()?
                .map(|(key, _)| key.value())
        };
        let Some(upper) = upper else { return Ok(page) };
        let mut after = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let tx = self.db.begin_read()?;
            let index = tx.open_table(schema::WS_BY_TIME)?;
            let records = tx.open_table(schema::WS_MESSAGES)?;
            let connections = tx.open_table(schema::WS_CONNECTIONS)?;
            let start = after.map_or(Bound::Unbounded, Bound::Excluded);
            let keys: Vec<_> = index
                .range((start, Bound::Included(upper)))?
                .take(256)
                .map(|entry| entry.map(|(key, _)| key.value()))
                .collect::<std::result::Result<_, _>>()?;
            for (_, id) in &keys {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let Some(value) = records.get(*id)? else {
                    continue;
                };
                let record: WsMessageRecord = codec::decode(value.value())?;
                queries::ensure_valid_record_version(record.record_version)?;
                let connection =
                    queries::read_ws_connection_from(&connections, record.connection_id)?
                        .unwrap_or_else(|| queries::placeholder_connection(record.connection_id));
                let row = record.to_history_item(connection, Vec::new());
                if matches(&row) {
                    page.push(row, *id, selected, limit.min(1000));
                }
            }
            after = keys.last().copied();
            if keys.len() < 256 {
                break;
            }
        }
        Ok(page)
    }
}
