use std::ops::Range;

use super::*;

fn plan(attack: AttackType, template: &str, sets: &[&[&str]]) -> IntruderPlan {
    IntruderPlan {
        attack,
        scheme: "http".to_string(),
        host: "127.0.0.1:1".to_string(),
        template: template.to_string(),
        payload_sets: sets
            .iter()
            .map(|set| set.iter().map(|value| value.to_string()).collect())
            .collect(),
        concurrency: 2,
        scope: Vec::new(),
    }
}

#[test]
fn marker_ranges_are_utf8_safe_and_reject_odd_delimiters() {
    let ranges = marker_ranges("GET /?q=$$héllo$$ HTTP/1.1\n\n").unwrap();
    let text = "GET /?q=$$héllo$$ HTTP/1.1\n\n";
    assert_eq!(&text[ranges[0].clone()], "héllo");
    assert!(matches!(
        marker_ranges("GET /?q=$$unclosed"),
        Err(IntruderError::UnbalancedMarker { .. })
    ));
    assert_eq!(
        marker_ranges("no markers").unwrap(),
        Vec::<Range<usize>>::new()
    );
}

#[test]
fn sniper_tests_each_position_without_replacing_the_others() {
    let plan = plan(
        AttackType::Sniper,
        "GET /?q=$$a$$&lang=$$en$$ HTTP/1.1\n\n",
        &[&["x", "y"]],
    );
    let prepared = plan.prepare().unwrap();
    assert_eq!(prepared.total(), 4);
    let rendered: Vec<_> = (1..=4)
        .map(|sequence| prepared.render(sequence).unwrap())
        .collect();
    assert_eq!(rendered[0].raw, "GET /?q=x&lang=en HTTP/1.1\n\n");
    assert_eq!(rendered[1].raw, "GET /?q=y&lang=en HTTP/1.1\n\n");
    assert_eq!(rendered[2].raw, "GET /?q=a&lang=x HTTP/1.1\n\n");
    assert_eq!(rendered[3].raw, "GET /?q=a&lang=y HTTP/1.1\n\n");
    assert_eq!(rendered[0].payloads, vec!["x"]);
}

#[test]
fn battering_ram_replaces_every_position_with_one_payload() {
    let plan = plan(
        AttackType::BatteringRam,
        "POST / HTTP/1.1\n\nuser=$$a$$&pass=$$b$$",
        &[&["same"]],
    );
    assert_eq!(
        plan.render_sequence(1).unwrap().raw,
        "POST / HTTP/1.1\n\nuser=same&pass=same"
    );
}

#[test]
fn pitchfork_stops_at_the_smallest_set() {
    let plan = plan(
        AttackType::Pitchfork,
        "GET /?u=$$a$$&p=$$b$$ HTTP/1.1\n\n",
        &[&["alice", "bob"], &["one", "two", "three"]],
    );
    assert_eq!(plan.request_count().unwrap(), 2);
    assert_eq!(
        plan.render_sequence(2).unwrap().raw,
        "GET /?u=bob&p=two HTTP/1.1\n\n"
    );
}

#[test]
fn cluster_bomb_advances_the_last_position_fastest() {
    let plan = plan(
        AttackType::ClusterBomb,
        "GET /?u=$$a$$&p=$$b$$ HTTP/1.1\n\n",
        &[&["alice", "bob"], &["one", "two"]],
    );
    assert_eq!(plan.request_count().unwrap(), 4);
    let rendered: Vec<_> = (1..=4)
        .map(|sequence| plan.render_sequence(sequence).unwrap().raw)
        .collect();
    assert_eq!(rendered[0], "GET /?u=alice&p=one HTTP/1.1\n\n");
    assert_eq!(rendered[1], "GET /?u=alice&p=two HTTP/1.1\n\n");
    assert_eq!(rendered[2], "GET /?u=bob&p=one HTTP/1.1\n\n");
    assert_eq!(rendered[3], "GET /?u=bob&p=two HTTP/1.1\n\n");
}

#[test]
fn empty_positions_and_unbalanced_markers_are_rejected() {
    assert!(matches!(
        plan(AttackType::Sniper, "GET / HTTP/1.1\n\n", &[&["x"]]).prepare(),
        Err(IntruderError::NoPositions)
    ));
    assert!(matches!(
        plan(AttackType::Sniper, "GET /?q=$$a$$ HTTP/1.1\n\n", &[&[]]).prepare(),
        Err(IntruderError::EmptyPayloadSet { .. })
    ));
    assert!(matches!(
        plan(
            AttackType::Pitchfork,
            "GET /?q=$$a$$&p=$$b$$ HTTP/1.1\n\n",
            &[&["x"]]
        )
        .prepare(),
        Err(IntruderError::MissingPayloadSet { .. })
    ));
}

#[test]
fn toggle_marker_wraps_unwraps_and_handles_empty_positions() {
    let text = "GET /?q=value HTTP/1.1\n\n";
    let start = text.find("value").unwrap();
    let (marked, range) = toggle_marker(text, start..start + 5);
    assert_eq!(marked, "GET /?q=$$value$$ HTTP/1.1\n\n");
    assert_eq!(&marked[range.clone()], "value");

    let (unmarked, _) = toggle_marker(&marked, range);
    assert_eq!(unmarked, text);

    let (empty, range) = toggle_marker(text, start..start);
    assert_eq!(empty, "GET /?q=$$$$value HTTP/1.1\n\n");
    assert_eq!(range, start + 2..start + 2);
    let (restored, _) = toggle_marker(&empty, range);
    assert_eq!(restored, text);
}

#[test]
fn toggle_marker_handles_multibyte_boundaries_and_existing_markers() {
    for character in ["é", "中", "😀"] {
        let text = format!("{character}value");
        let start = character.len();
        for selection in [start..start, start..text.len()] {
            let (marked, range) = toggle_marker(&text, selection.clone());
            assert_eq!(range, start + 2..selection.end + 2);
            assert_eq!(&marked[range.clone()], &text[selection.clone()]);
            let (restored, restored_range) = toggle_marker(&marked, range);
            assert_eq!(restored, text);
            assert_eq!(restored_range, selection);
        }
        let marked = format!("{character}$${character}$$");
        let start = character.len();
        let (restored, range) = toggle_marker(&marked, start..marked.len());
        assert_eq!(restored, format!("{character}{character}"));
        assert_eq!(range, start..start + character.len());
    }
}

#[test]
fn auto_mark_covers_query_form_and_cookie_values() {
    let request = "POST /search?q=term&page=2 HTTP/1.1\nHost: example.test\nCookie: session=abc; theme=dark\nContent-Type: application/x-www-form-urlencoded\n\nuser=alice&pass=secret";
    let (marked, count) = auto_mark(request);
    assert_eq!(count, 6);
    assert!(marked.contains("q=$$term$$"));
    assert!(marked.contains("page=$$2$$"));
    assert!(marked.contains("session=$$abc$$"));
    assert!(marked.contains("theme=$$dark$$"));
    assert!(marked.contains("user=$$alice$$"));
    assert!(marked.contains("pass=$$secret$$"));

    let (again, count) = auto_mark(&marked);
    assert_eq!(count, 0);
    assert_eq!(again, marked);
}

#[test]
fn auto_mark_skips_values_already_inside_a_position() {
    let request = "GET /?q=$$term$$&page=2 HTTP/1.1\nHost: example.test\n\n";
    let (marked, count) = auto_mark(request);
    assert_eq!(count, 1);
    assert!(marked.contains("q=$$term$$"));
    assert!(marked.contains("page=$$2$$"));
}

#[test]
fn update_host_header_replaces_or_inserts_a_header() {
    let request = "GET / HTTP/1.1\r\nHost: old.test\r\nAccept: */*\r\n\r\n";
    assert_eq!(
        update_host_header(request, "new.test"),
        "GET / HTTP/1.1\r\nHost: new.test\r\nAccept: */*\r\n\r\n"
    );
    let no_host = "GET / HTTP/1.1\nAccept: */*\n\n";
    assert_eq!(
        update_host_header(no_host, "new.test"),
        "GET / HTTP/1.1\nHost: new.test\nAccept: */*\n\n"
    );
    assert_eq!(update_host_header(request, ""), request);
}

#[test]
fn request_counts_fail_instead_of_overflowing() {
    let plan = IntruderPlan {
        attack: AttackType::ClusterBomb,
        scheme: "http".to_string(),
        host: "127.0.0.1".to_string(),
        template: "GET /?a=$$a$$&b=$$b$$ HTTP/1.1\n\n".to_string(),
        payload_sets: vec![vec!["x".to_string(); 2], vec!["y".to_string(); 2]],
        concurrency: 1,
        scope: Vec::new(),
    };
    assert_eq!(plan.request_count().unwrap(), 4);

    // 65,536 is 2^16; four non-empty sets make the product exceed u64
    // without allocating billions of payload strings.
    let huge = IntruderPlan {
        attack: AttackType::ClusterBomb,
        template: "GET /?a=$$a$$&b=$$b$$&c=$$c$$&d=$$d$$ HTTP/1.1\n\n".to_string(),
        payload_sets: (0..4).map(|_| vec![String::new(); 65_536]).collect(),
        ..plan
    };
    assert!(matches!(
        huge.request_count(),
        Err(IntruderError::RequestCountOverflow)
    ));
}

#[test]
fn split_raw_request_handles_crlf_lf_and_missing_separators() {
    assert_eq!(
        split_raw_request("GET / HTTP/1.1\r\n\r\nbody"),
        ("GET / HTTP/1.1\r\n\r\n", "body")
    );
    assert_eq!(
        split_raw_request("GET / HTTP/1.1\n\nbody"),
        ("GET / HTTP/1.1\n\n", "body")
    );
    assert_eq!(split_raw_request("GET / HTTP/1.1"), ("GET / HTTP/1.1", ""));
}

#[test]
fn update_host_header_preserves_line_endings_and_handles_unterminated_requests() {
    let crlf = "GET / HTTP/1.1\r\nAccept: */*\r\n\r\n";
    assert_eq!(
        update_host_header(crlf, "new.test"),
        "GET / HTTP/1.1\r\nHost: new.test\r\nAccept: */*\r\n\r\n"
    );

    let one_line = "GET / HTTP/1.1";
    assert_eq!(
        update_host_header(one_line, "new.test"),
        "GET / HTTP/1.1\nHost: new.test\n"
    );
    assert_eq!(update_host_header(one_line, "   "), one_line);
}

#[test]
fn prepare_reports_missing_and_empty_sets_and_bad_sequences() {
    let mut pitchfork = plan(
        AttackType::Pitchfork,
        "GET /?a=$$1$$&b=$$2$$ HTTP/1.1\n\n",
        &[&["x"]],
    );
    assert!(matches!(
        pitchfork.request_count(),
        Err(IntruderError::MissingPayloadSet { position: 2 })
    ));

    pitchfork.payload_sets = vec![vec!["x".to_string()], Vec::new()];
    assert!(matches!(
        pitchfork.request_count(),
        Err(IntruderError::EmptyPayloadSet { position: 2 })
    ));

    let sniper = plan(
        AttackType::Sniper,
        "GET /?a=$$1$$ HTTP/1.1\n\n",
        &[&["x", "y"]],
    );
    assert!(matches!(
        sniper.render_sequence(0),
        Err(IntruderError::SequenceOutOfRange {
            sequence: 0,
            total: 2
        })
    ));
    assert!(matches!(
        sniper.render_sequence(3),
        Err(IntruderError::SequenceOutOfRange {
            sequence: 3,
            total: 2
        })
    ));
}
