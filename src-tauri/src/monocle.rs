//! Mirrors generated tokens onto the monocle's panel.
//!
//! llama.cpp emits a token every few milliseconds; the BLE connection interval
//! is 30 ms and a full panel redraw is ~25 ms. Writing per token would
//! saturate the link and outrun the display, so tokens are collected and sent
//! as one `append` on a timer.
//!
//! The batching is deliberately separated from the BLE write behind a sink, in
//! the same spirit as `llama::stream_completion` being separated from the
//! sidecar lifecycle: the interesting logic is then testable without hardware.

use std::future::Future;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::ble::{self, BleState, DISPLAY_OP_APPEND, DISPLAY_OP_SET, DISPLAY_TEXT_MAX};

/// How often pending tokens are pushed to the panel. Long enough to batch a
/// handful of tokens into one write, short enough to read as streaming.
const FLUSH_INTERVAL: Duration = Duration::from_millis(150);

/// Shown while the model is thinking. Inference takes seconds, and a panel
/// still showing the previous answer is indistinguishable from a frozen one.
///
/// ASCII on purpose: the firmware's font covers 0x20..0x7E and draws anything
/// else as '?', so a typographic ellipsis would render as junk.
const THINKING: &str = "...";

enum Message {
    Token(String),
}

/// Handle held for the duration of one generation.
///
/// Dropping it ends the stream: the task flushes whatever is pending and
/// exits, so an error path that returns early still leaves the last words on
/// the panel.
pub struct Mirror {
    tx: Option<mpsc::UnboundedSender<Message>>,
}

impl Mirror {
    /// Queues a token. Cheap and non-blocking — the write happens on the
    /// mirror task, so the caller's SSE loop is never held up by BLE.
    pub fn push(&self, token: &str) {
        if let Some(tx) = &self.tx {
            // Send only fails once the task has stopped, which is what happens
            // after a write error. Dropping tokens then is the intent.
            let _ = tx.send(Message::Token(token.to_string()));
        }
    }

    /// A mirror that goes nowhere, for when no monocle is connected.
    fn disabled() -> Self {
        Mirror { tx: None }
    }
}

/// Starts mirroring, if there is a monocle to mirror to.
///
/// Returns a disabled handle when nothing is connected, so chatting with no
/// device attached costs nothing and logs nothing.
pub fn start(app: &AppHandle) -> Mirror {
    let Some(state) = app.try_state::<BleState>() else {
        return Mirror::disabled();
    };
    if !state.is_connected() {
        return Mirror::disabled();
    }

    let (tx, rx) = mpsc::unbounded_channel();
    let task_app = app.clone();

    tauri::async_runtime::spawn(async move {
        run(rx, FLUSH_INTERVAL, move |op, text| {
            let app = task_app.clone();
            async move {
                let Some(state) = app.try_state::<BleState>() else {
                    return Err("no BLE state".to_string());
                };
                ble::write_display(&app, &state, op, &text).await
            }
        })
        .await;
    });

    Mirror { tx: Some(tx) }
}

/// Splits text into pieces that each fit a single ATT write.
///
/// Splitting on a character boundary matters: cutting a multi-byte character
/// in half would put replacement junk in front of the wearer, and the panel is
/// the one surface they cannot scroll back on.
fn split_for_writes(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let mut end = (start + DISPLAY_TEXT_MAX).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&text[start..end]);
        start = end;
    }
    chunks
}

/// Collects tokens and hands them to `sink` in batches.
///
/// The first batch replaces the screen — clearing the thinking indicator —
/// and every batch after it appends. Returns when the sender is dropped or a
/// write fails.
async fn run<F, Fut>(
    mut rx: mpsc::UnboundedReceiver<Message>,
    interval: Duration,
    mut sink: F,
) where
    F: FnMut(u8, String) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if sink(DISPLAY_OP_SET, THINKING.to_string()).await.is_err() {
        return;
    }

    let mut ticker = tokio::time::interval(interval);
    // Fixed cadence rather than a timer restarted per token, which a steady
    // stream would keep resetting forever. Delay rather than Burst so a slow
    // write cannot be followed by a flurry of catch-up flushes.
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await; // the first tick resolves immediately

    let mut pending = String::new();
    let mut replaced = false;

    loop {
        tokio::select! {
            message = rx.recv() => match message {
                Some(Message::Token(token)) => pending.push_str(&token),
                // Sender dropped: the generation is over, so flush the tail
                // rather than stranding the last words.
                None => {
                    flush(&mut pending, &mut replaced, &mut sink).await;
                    return;
                }
            },
            _ = ticker.tick() => {
                if !flush(&mut pending, &mut replaced, &mut sink).await {
                    return;
                }
            }
        }
    }
}

/// Writes whatever has accumulated. Returns false if the sink failed, which
/// ends the mirror: the panel is advisory, and a Bluetooth problem must not
/// interrupt a reply the user is reading on screen.
async fn flush<F, Fut>(pending: &mut String, replaced: &mut bool, sink: &mut F) -> bool
where
    F: FnMut(u8, String) -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if pending.is_empty() {
        return true;
    }

    for chunk in split_for_writes(pending) {
        let op = if *replaced {
            DISPLAY_OP_APPEND
        } else {
            DISPLAY_OP_SET
        };
        if let Err(err) = sink(op, chunk.to_string()).await {
            eprintln!("monocle: display write failed, stopped mirroring: {err}");
            pending.clear();
            return false;
        }
        *replaced = true;
    }

    pending.clear();
    true
}

#[cfg(test)]
#[path = "../tests/monocle_test.rs"]
mod tests;
