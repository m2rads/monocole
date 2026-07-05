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
