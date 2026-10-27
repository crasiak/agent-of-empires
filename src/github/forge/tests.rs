use super::policy::{decode_async, AsyncResult};
use super::*;
use serde_json::json;
use std::sync::Mutex;

#[test]
fn rejects_unsafe_destinations_and_unknown_approval_fields() {
    for url in [
        "https://github.com/a/b?x",
        "https://token@github.com/a/b",
        "https://github.com:443/a/b",
        "https://github.com/a/../b",
        "https://github.com/a/b/c",
        "https://evil.test/a/b",
        "file:///tmp/repo",
        "ext::sh",
        "git@github.com:a/b",
        "https://github.com/a/%62",
        "https://github.com/a/b#x",
    ] {
        assert!(Destination::new(url, 1, "main").is_err(), "{url}");
    }
    assert_eq!(
        Destination::new("https://github.com/acme/skills.git", 1, "main")
            .unwrap()
            .remote(),
        "https://github.com/acme/skills.git"
    );
    assert!(serde_json::from_value::<PublishApproval>(
        json!({"preview_digest":"a","disclosure_confirmed":true,"ignore_policy":true})
    )
    .is_err());
}

#[test]
fn async_decoder_never_confuses_acceptance_or_queue_with_merge() {
    let pending = json!({"status":"pending","details":{"message":"queued","uuid":"4fdf0961-8919-43c8-9c39-6cfec9a094d7","merge_method":"squash","merge_action":"direct_merge","expected_head_sha":"1111111111111111111111111111111111111111","bypass_rules":false}});
    assert!(matches!(
        decode_async(&serde_json::to_vec(&pending).unwrap()).unwrap(),
        AsyncResult::Pending(_)
    ));
    for value in [
        json!({"status":"mystery","details":{"message":"x"}}),
        json!({"status":"merged","details":{"message":"x","sha":null}}),
        json!({"status":"merged","details":{"message":"x","sha":"1111111111111111111111111111111111111111","extra":true}}),
    ] {
        assert!(decode_async(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    assert!(matches!(
        decode_async(br#"{"status":"enqueued","details":{"message":"queued"}}"#).unwrap(),
        AsyncResult::Enqueued
    ));
}

#[path = "fixture_tests.rs"]
mod fixture_tests;
