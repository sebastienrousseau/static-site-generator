// Copyright © 2023 - 2026 Static Site Generator (SSG). All rights reserved.
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Integration tests for `ssg::hmr` — protocol stability (AC5) plus
//! broadcaster fanout semantics.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use serde_json::Value;
use ssg::hmr::{HmrBroadcaster, HmrMessage, HmrType};

// ---------------------------------------------------------------------------
// AC5 — wire-protocol shape is stable.
// ---------------------------------------------------------------------------

#[test]
fn ac5_protocol_hmr_css_shape() {
    let m = HmrMessage::css(vec!["assets/style.css".into()]).with_sha("abc");
    let v: Value = serde_json::from_str(&m.to_json()).unwrap();
    assert_eq!(v["type"], "hmr-css");
    assert_eq!(v["paths"][0], "assets/style.css");
    assert_eq!(v["sha"], "abc");
}

#[test]
fn ac5_protocol_hmr_html_shape() {
    let m = HmrMessage::html(vec!["/blog/foo/".into(), "/blog/bar/".into()])
        .with_sha("def");
    let v: Value = serde_json::from_str(&m.to_json()).unwrap();
    assert_eq!(v["type"], "hmr-html");
    assert_eq!(v["paths"][0], "/blog/foo/");
    assert_eq!(v["paths"][1], "/blog/bar/");
    assert_eq!(v["sha"], "def");
}

#[test]
fn ac5_protocol_reload_shape() {
    let m = HmrMessage::reload();
    let v: Value = serde_json::from_str(&m.to_json()).unwrap();
    assert_eq!(v["type"], "reload");
    assert!(
        v["paths"].as_array().unwrap().is_empty(),
        "{:?}",
        v["paths"].as_array().unwrap()
    );
}

#[test]
fn ac5_protocol_three_types_distinct() {
    assert_eq!(HmrType::HmrCss.wire(), "hmr-css");
    assert_eq!(HmrType::HmrHtml.wire(), "hmr-html");
    assert_eq!(HmrType::Reload.wire(), "reload");
    // The three wire strings are mutually distinct.
    assert_ne!(HmrType::HmrCss.wire(), HmrType::HmrHtml.wire());
    assert_ne!(HmrType::HmrCss.wire(), HmrType::Reload.wire());
    assert_ne!(HmrType::HmrHtml.wire(), HmrType::Reload.wire());
}

#[test]
fn ac5_protocol_round_trips_through_json() {
    let m = HmrMessage::html(vec!["/a/".into()]).with_sha("ff");
    let json = m.to_json();
    let back: HmrMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m);
}

#[test]
fn ac5_protocol_sha_field_is_optional_on_wire() {
    // A frame without "sha" should still parse — the JS client
    // tolerates older servers and tests must too.
    let json = r#"{"type":"hmr-html","paths":["/x/"]}"#;
    let m: HmrMessage = serde_json::from_str(json).unwrap();
    assert_eq!(m.kind, HmrType::HmrHtml);
    assert_eq!(m.sha, "");
}

// ---------------------------------------------------------------------------
// Broadcaster — fanout, eviction.
// ---------------------------------------------------------------------------

#[test]
fn broadcaster_starts_with_no_subscribers() {
    let b = HmrBroadcaster::new();
    assert_eq!(b.subscriber_count(), 0);
}

#[test]
fn broadcaster_fans_one_message_to_many_tabs() {
    let b = HmrBroadcaster::new();
    let received = Arc::new(AtomicUsize::new(0));
    for _ in 0..5 {
        let r = Arc::clone(&received);
        b.subscribe(Box::new(move |_| {
            let _ = r.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
    }
    let delivered = b.broadcast(&HmrMessage::reload());
    assert_eq!(delivered, 5);
    assert_eq!(received.load(Ordering::SeqCst), 5);
}

#[test]
fn broadcaster_evicts_disconnected_tabs() {
    let b = HmrBroadcaster::new();
    b.subscribe(Box::new(|_| Err(())));
    b.subscribe(Box::new(|_| Ok(())));
    b.subscribe(Box::new(|_| Err(())));
    assert_eq!(b.subscriber_count(), 3);

    let delivered = b.broadcast(&HmrMessage::reload());
    assert_eq!(delivered, 1, "only one healthy tab");
    assert_eq!(
        b.subscriber_count(),
        1,
        "two erroring tabs should be evicted"
    );
}

#[test]
fn broadcaster_propagates_full_json_to_subscribers() {
    let b = HmrBroadcaster::new();
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let c = Arc::clone(&captured);
    b.subscribe(Box::new(move |payload| {
        if let Ok(mut g) = c.lock() {
            *g = Some(payload.to_string());
        }
        Ok(())
    }));

    let msg = HmrMessage::html(vec!["/p/".into()]).with_sha("zz");
    let _ = b.broadcast(&msg);
    let seen = captured.lock().unwrap().clone().unwrap();
    let v: Value = serde_json::from_str(&seen).unwrap();
    assert_eq!(v["type"], "hmr-html");
    assert_eq!(v["paths"][0], "/p/");
    assert_eq!(v["sha"], "zz");
}

#[test]
fn broadcaster_default_constructor() {
    let b = HmrBroadcaster::default();
    assert_eq!(b.subscriber_count(), 0);
}

#[test]
fn broadcaster_handles_burst_of_broadcasts() {
    // 1000 frames should not allocate unboundedly or hang.
    let b = HmrBroadcaster::new();
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    b.subscribe(Box::new(move |_| {
        let _ = c.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    for _ in 0..1000 {
        let _ = b.broadcast(&HmrMessage::reload());
    }
    assert_eq!(count.load(Ordering::SeqCst), 1000);
}

#[test]
fn broadcaster_no_subscribers_returns_zero() {
    let b = HmrBroadcaster::new();
    assert_eq!(b.broadcast(&HmrMessage::reload()), 0);
}

// ---------------------------------------------------------------------------
// HmrMessage convenience constructors.
// ---------------------------------------------------------------------------

#[test]
fn hmr_message_constructors_set_kind_correctly() {
    assert_eq!(HmrMessage::css(vec![]).kind, HmrType::HmrCss);
    assert_eq!(HmrMessage::html(vec![]).kind, HmrType::HmrHtml);
    assert_eq!(HmrMessage::reload().kind, HmrType::Reload);
}

#[test]
fn hmr_message_with_sha_returns_self_for_chaining() {
    let m = HmrMessage::css(vec!["x.css".into()]).with_sha("aa");
    assert_eq!(m.sha, "aa");
}
