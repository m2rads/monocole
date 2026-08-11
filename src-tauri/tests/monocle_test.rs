//! Unit tests for token mirroring.
//!
//! The BLE write is injected as a sink, so everything interesting — batching,
//! the set-then-append sequence, splitting, failure handling — is exercised
//! with no hardware and no Bluetooth. What is left for a human is whether
//! 150 ms *looks* right on a real panel.
//!
//! Time is paused: `start_paused` makes the runtime jump to the next timer
//! once everything is idle, so the flush interval is deterministic instead of
//! a race against the test host.

use std::sync::{Arc, Mutex};

use super::*;

type Log = Arc<Mutex<Vec<(u8, String)>>>;

/// A sink that records what it was asked to write.
fn recording_sink(log: &Log) -> impl FnMut(u8, String) -> std::future::Ready<Result<(), String>> {
    let log = log.clone();
    move |op, text| {
        log.lock().unwrap().push((op, text));
        std::future::ready(Ok(()))
    }
}

/// A sink that fails on every call.
fn failing_sink() -> impl FnMut(u8, String) -> std::future::Ready<Result<(), String>> {
    move |_op, _text| std::future::ready(Err("device went away".to_string()))
}

fn entries(log: &Log) -> Vec<(u8, String)> {
    log.lock().unwrap().clone()
}

#[test]
fn split_leaves_short_text_in_one_piece() {
    assert_eq!(split_for_writes("hello"), vec!["hello"]);
}

#[test]
fn split_is_empty_for_empty_text() {
    assert!(split_for_writes("").is_empty());
}

#[test]
fn split_fills_each_write_to_the_maximum() {
    let text = "x".repeat(DISPLAY_TEXT_MAX * 2 + 5);
    let chunks = split_for_writes(&text);
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0].len(), DISPLAY_TEXT_MAX);
    assert_eq!(chunks[1].len(), DISPLAY_TEXT_MAX);
    assert_eq!(chunks[2].len(), 5);
}

#[test]
fn split_never_cuts_a_character_in_half() {
    // "é" is two bytes, so a byte-based split would land mid-character every
    // other character and put junk on the panel.
    let text = "é".repeat(DISPLAY_TEXT_MAX);
    let chunks = split_for_writes(&text);

    assert!(chunks.len() > 1, "expected the text to need several writes");
    for chunk in &chunks {
        assert!(chunk.len() <= DISPLAY_TEXT_MAX);
        // Reassembling must reproduce the original exactly.
        assert!(chunk.chars().all(|c| c == 'é'));
    }
    assert_eq!(chunks.concat(), text);
}

#[tokio::test(start_paused = true)]
async fn thinking_indicator_goes_up_before_any_token() {
    let log: Log = Arc::default();
    let (tx, rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(run(rx, Duration::from_millis(150), recording_sink(&log)));
    drop(tx);
    task.await.unwrap();

    assert_eq!(entries(&log)[0], (DISPLAY_OP_SET, "...".to_string()));
}

#[tokio::test(start_paused = true)]
async fn tokens_within_one_interval_become_a_single_write() {
    let log: Log = Arc::default();
    let (tx, rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(run(rx, Duration::from_millis(150), recording_sink(&log)));
    for token in ["Hel", "lo ", "there"] {
        tx.send(Message::Token(token.to_string())).unwrap();
    }
    drop(tx);
    task.await.unwrap();

    // The placeholder, then one batch — not one write per token, which would
    // outrun both the link and the panel.
    assert_eq!(
        entries(&log),
        vec![
            (DISPLAY_OP_SET, "...".to_string()),
            (DISPLAY_OP_SET, "Hello there".to_string()),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn the_first_batch_replaces_and_later_ones_append() {
    let log: Log = Arc::default();
    let (tx, rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(run(rx, Duration::from_millis(150), recording_sink(&log)));

    tx.send(Message::Token("first".to_string())).unwrap();
    // Long enough for the ticker to fire between the two batches.
    tokio::time::sleep(Duration::from_millis(400)).await;
    tx.send(Message::Token(" second".to_string())).unwrap();
    drop(tx);
    task.await.unwrap();

    let entries = entries(&log);
    assert_eq!(entries[0], (DISPLAY_OP_SET, "...".to_string()));
    // Replaces the placeholder rather than appending to it.
    assert_eq!(entries[1], (DISPLAY_OP_SET, "first".to_string()));
    // Everything after adds to what is on screen.
    assert_eq!(entries[2], (DISPLAY_OP_APPEND, " second".to_string()));
}

#[tokio::test(start_paused = true)]
async fn the_tail_is_flushed_when_the_stream_ends() {
    let log: Log = Arc::default();
    let (tx, rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(run(rx, Duration::from_millis(150), recording_sink(&log)));
    // Arrives and the sender drops immediately, well inside one interval.
    tx.send(Message::Token("last words".to_string())).unwrap();
    drop(tx);
    task.await.unwrap();

    assert_eq!(
        entries(&log).last().unwrap(),
        &(DISPLAY_OP_SET, "last words".to_string()),
        "tokens arriving just before the end must not be stranded"
    );
}

#[tokio::test(start_paused = true)]
async fn a_batch_larger_than_one_write_is_split() {
    let log: Log = Arc::default();
    let (tx, rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(run(rx, Duration::from_millis(150), recording_sink(&log)));
    tx.send(Message::Token("y".repeat(DISPLAY_TEXT_MAX + 10)))
        .unwrap();
    drop(tx);
    task.await.unwrap();

    let entries = entries(&log);
    assert_eq!(entries.len(), 3, "placeholder plus two writes");
    assert_eq!(entries[1].0, DISPLAY_OP_SET);
    assert_eq!(entries[1].1.len(), DISPLAY_TEXT_MAX);
    // The overflow continues the same screen rather than replacing it.
    assert_eq!(entries[2], (DISPLAY_OP_APPEND, "y".repeat(10)));
}

#[tokio::test(start_paused = true)]
async fn a_failed_write_stops_mirroring_without_hanging() {
    let (tx, rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(run(rx, Duration::from_millis(150), failing_sink()));

    // The task gives up on the first failure; sending afterwards must not
    // block or panic, because generation carries on regardless.
    tx.send(Message::Token("ignored".to_string())).ok();
    task.await.unwrap();
    tx.send(Message::Token("also ignored".to_string())).ok();
}
