//! Unit tests for the BLE module. Live scanning/connecting needs hardware
//! and stays manual; these cover the event payload shapes, which are the
//! contract with src/hooks/use-ble.ts — camelCase keys, absent (not null)
//! optional fields.

use super::*;

#[test]
fn status_event_serializes_camel_case_and_skips_none() {
    let value = serde_json::to_value(StatusEvent {
        kind: "connected",
        device_id: Some("aa-bb".into()),
        name: None,
        reason: None,
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "kind": "connected", "deviceId": "aa-bb" })
    );
}

#[test]
fn scan_timeout_event_carries_reason() {
    let value = serde_json::to_value(StatusEvent {
        kind: "scanStopped",
        device_id: None,
        name: None,
        reason: Some("timeout"),
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "kind": "scanStopped", "reason": "timeout" })
    );
}

#[test]
fn discovered_device_serializes_camel_case() {
    let value = serde_json::to_value(DiscoveredDevice {
        id: "x".into(),
        name: Some("minicole-monocle".into()),
        rssi: Some(-40),
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "id": "x", "name": "minicole-monocle", "rssi": -40 })
    );
}

// --- Wi-Fi provisioning -----------------------------------------------------
//
// The encode/parse pair is the wire contract with the firmware's
// gatt_svr_chr_access_wifi() and gatt_svr_notify_wifi_state(). Both sides
// reject malformed input rather than trusting a length byte.

#[test]
fn wifi_creds_encode_length_prefixed() {
    assert_eq!(
        encode_wifi_creds("net", "pw").unwrap(),
        vec![3, b'n', b'e', b't', 2, b'p', b'w']
    );
}

#[test]
fn wifi_creds_allow_empty_password_for_open_networks() {
    assert_eq!(
        encode_wifi_creds("cafe", "").unwrap(),
        vec![4, b'c', b'a', b'f', b'e', 0]
    );
}

#[test]
fn wifi_creds_reject_empty_ssid() {
    assert!(encode_wifi_creds("", "pw").is_err());
}

#[test]
fn wifi_creds_reject_oversized_fields() {
    assert!(encode_wifi_creds(&"a".repeat(SSID_MAX_LEN + 1), "pw").is_err());
    assert!(encode_wifi_creds("net", &"a".repeat(PASS_MAX_LEN + 1)).is_err());
}

#[test]
fn wifi_creds_error_never_quotes_the_password() {
    let secret = "a".repeat(PASS_MAX_LEN + 1);
    let err = encode_wifi_creds("net", &secret).unwrap_err();
    assert!(!err.contains(&secret));
}

#[test]
fn wifi_creds_length_prefixes_count_bytes_not_chars() {
    // "é" is two UTF-8 bytes; the firmware indexes bytes.
    let encoded = encode_wifi_creds("é", "").unwrap();
    assert_eq!(encoded[0], 2);
    assert_eq!(encoded.len(), 1 + 2 + 1);
}

#[test]
fn wifi_state_parses_connected_with_address() {
    let event = parse_wifi_state(&[2, 192, 168, 4, 22]).unwrap();
    assert_eq!(event.kind, "connected");
    assert_eq!(event.ip.as_deref(), Some("192.168.4.22"));
}

#[test]
fn wifi_state_parses_failure_reason() {
    let event = parse_wifi_state(&[3, 15]).unwrap();
    assert_eq!(event.kind, "failed");
    assert_eq!(event.reason, Some(15));
}

#[test]
fn wifi_state_parses_progress_states() {
    assert_eq!(parse_wifi_state(&[0]).unwrap().kind, "idle");
    assert_eq!(parse_wifi_state(&[1]).unwrap().kind, "connecting");
}

#[test]
fn wifi_state_rejects_truncated_and_unknown_payloads() {
    assert!(parse_wifi_state(&[]).is_none());
    assert!(parse_wifi_state(&[2, 192, 168]).is_none(), "short address");
    assert!(parse_wifi_state(&[3]).is_none(), "missing reason");
    assert!(parse_wifi_state(&[9]).is_none(), "unknown state");
}

#[test]
fn display_payload_leads_with_the_op_byte() {
    assert_eq!(encode_display(DISPLAY_OP_SET, "hi").unwrap(), b"\x01hi");
}

#[test]
fn display_payload_allows_empty_text() {
    // How `clear` is sent: an op and nothing else.
    assert_eq!(encode_display(0, "").unwrap(), vec![0]);
}

#[test]
fn display_payload_counts_bytes_not_characters() {
    // A multi-byte character must not be measured as one, or the payload
    // would overrun what a single ATT write can carry.
    let text = "é".repeat(DISPLAY_TEXT_MAX / 2);
    assert_eq!(
        encode_display(DISPLAY_OP_SET, &text).unwrap().len(),
        1 + DISPLAY_TEXT_MAX / 2 * 2
    );
}

#[test]
fn display_payload_at_the_maximum_fits_one_att_write() {
    // An ATT write request carries MTU-3 = 253 bytes of value at the MTU of
    // 256 macOS negotiates, and the op byte is one of them. A limit set one
    // byte higher builds a payload that cannot be written at all.
    let text = "x".repeat(DISPLAY_TEXT_MAX);
    let payload = encode_display(DISPLAY_OP_SET, &text).unwrap();
    assert_eq!(payload.len(), 253);
}

#[test]
fn display_payload_rejects_text_too_long_for_one_write() {
    // Truncating would cut UTF-8 mid-character; splitting is the caller's job.
    let text = "x".repeat(DISPLAY_TEXT_MAX + 1);
    assert!(encode_display(DISPLAY_OP_SET, &text).is_err());
}

#[test]
fn wifi_event_serializes_camel_case_and_skips_none() {
    let value = serde_json::to_value(WifiEvent {
        kind: "connected",
        ip: Some("10.0.0.5".into()),
        reason: None,
    })
    .unwrap();
    assert_eq!(
        value,
        serde_json::json!({ "kind": "connected", "ip": "10.0.0.5" })
    );
}
