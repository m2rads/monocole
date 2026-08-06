use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::Duration;

use btleplug::api::{
    Central, CentralEvent, CentralState, Characteristic, Manager as _, Peripheral as _, ScanFilter,
    WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use uuid::{uuid, Uuid};

pub const DEVICE_EVENT: &str = "ble-device";
pub const STATUS_EVENT: &str = "ble-status";
pub const WIFI_EVENT: &str = "ble-wifi";
const SCAN_TTL: Duration = Duration::from_secs(30);

// The monocle's GATT contract. These must match the BLE_UUID128_INIT values in
// the firmware's gatt_svr.c — see docs/protocol.md.
const WIFI_CREDS_CHR_UUID: Uuid = uuid!("2c9b4a45-d3a5-4bf9-ac60-1f5f2e98db3c");
const WIFI_STATE_CHR_UUID: Uuid = uuid!("1ad1e743-dcae-422d-a7a8-68b4d695ac8b");

// Mirrors the 802.11 limits the firmware enforces. Checked here too so a bad
// value is reported in the UI rather than as an opaque ATT error.
const SSID_MAX_LEN: usize = 32;
const PASS_MAX_LEN: usize = 63;

// The monocle is currently a XIAO ESP32-S3 Sense, but the board may change,
// so connection management stays generic BLE: scan for any peripheral and
// let the user pick. Once the firmware's GATT service stabilizes, add its
// service UUID to the ScanFilter and to a "looks like a monocle" check.
// TODO(auto-reconnect): persist the chosen device id in settings.json and
// reconnect to it automatically on launch / signal loss.
// TODO(monocle-protocol): after connect, subscribe to the voice and status
// characteristics and route them into a session, and write control/tokens.
// `discover_services()` below currently runs only as a connectivity check —
// its result is dropped. Stills do NOT arrive here: they come over a TCP
// socket on the Wi-Fi data plane, bootstrapped by writing credentials to the
// wifi_creds characteristic. See docs/protocol.md.

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedDevice {
    id: String,
    name: Option<String>,
}

#[derive(Default)]
struct BleInner {
    adapter: Option<Adapter>,
    events_task_started: bool,
}

#[derive(Default)]
pub struct BleState {
    inner: Mutex<BleInner>,
    /// Shared with the event-listener task so unexpected disconnects clear it.
    connected: Arc<StdMutex<Option<ConnectedDevice>>>,
    /// Shared with the TTL and event tasks so any of them can end a scan.
    scanning: Arc<AtomicBool>,
    /// Bumped whenever a scan starts or stops, so a stale TTL task can tell
    /// it no longer owns the current scan.
    scan_generation: Arc<AtomicU64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveredDevice {
    id: String,
    name: Option<String>,
    rssi: Option<i16>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusEvent {
    /// "scanning" | "scanStopped" | "connected" | "disconnected" | "bluetoothOff"
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// For scanStopped: "timeout" when the scan hit its TTL.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleSnapshot {
    scanning: bool,
    connected: Option<ConnectedDevice>,
}

fn emit_status(
    app: &AppHandle,
    kind: &'static str,
    device_id: Option<String>,
    name: Option<String>,
    reason: Option<&'static str>,
) {
    let _ = app.emit(
        STATUS_EVENT,
        StatusEvent {
            kind,
            device_id,
            name,
            reason,
        },
    );
}

#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WifiEvent {
    /// "idle" | "connecting" | "connected" | "failed"
    kind: &'static str,
    /// Present on "connected": the address to open the data socket to.
    #[serde(skip_serializing_if = "Option::is_none")]
    ip: Option<String>,
    /// Present on "failed": the raw 802.11 disconnect reason from the chip.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<u8>,
}

/// Builds the wifi_creds payload: `[ssid_len][ssid][pass_len][pass]`.
///
/// An empty password means an open network, which the firmware maps to
/// `WIFI_AUTH_OPEN`.
fn encode_wifi_creds(ssid: &str, password: &str) -> Result<Vec<u8>, String> {
    let ssid = ssid.as_bytes();
    let password = password.as_bytes();

    if ssid.is_empty() {
        return Err("network name cannot be empty".into());
    }
    if ssid.len() > SSID_MAX_LEN {
        return Err(format!(
            "network name is {} bytes; the maximum is {SSID_MAX_LEN}",
            ssid.len()
        ));
    }
    // Deliberately does not quote the password in the message.
    if password.len() > PASS_MAX_LEN {
        return Err(format!(
            "password is {} bytes; the maximum is {PASS_MAX_LEN}",
            password.len()
        ));
    }

    let mut out = Vec::with_capacity(2 + ssid.len() + password.len());
    out.push(ssid.len() as u8);
    out.extend_from_slice(ssid);
    out.push(password.len() as u8);
    out.extend_from_slice(password);
    Ok(out)
}

/// Decodes a wifi_state notification. Returns `None` for anything malformed,
/// so a firmware mismatch is ignored rather than surfaced as a bogus state.
fn parse_wifi_state(value: &[u8]) -> Option<WifiEvent> {
    let (&state, rest) = value.split_first()?;
    let event = match state {
        0 => WifiEvent {
            kind: "idle",
            ip: None,
            reason: None,
        },
        1 => WifiEvent {
            kind: "connecting",
            ip: None,
            reason: None,
        },
        2 => {
            let octets: [u8; 4] = rest.get(..4)?.try_into().ok()?;
            WifiEvent {
                kind: "connected",
                ip: Some(format!(
                    "{}.{}.{}.{}",
                    octets[0], octets[1], octets[2], octets[3]
                )),
                reason: None,
            }
        }
        3 => WifiEvent {
            kind: "failed",
            ip: None,
            reason: Some(*rest.first()?),
        },
        _ => return None,
    };
    Some(event)
}

fn find_characteristic(peripheral: &Peripheral, uuid: Uuid) -> Option<Characteristic> {
    peripheral.characteristics().into_iter().find(|c| c.uuid == uuid)
}

/// Resolves the currently connected peripheral, or explains why it can't.
async fn connected_peripheral(app: &AppHandle, state: &BleState) -> Result<Peripheral, String> {
    let id = state
        .connected
        .lock()
        .unwrap()
        .as_ref()
        .map(|device| device.id.clone())
        .ok_or("no monocle is connected")?;

    let mut inner = state.inner.lock().await;
    let adapter = ensure_adapter(app, &mut inner, state).await?;

    adapter
        .peripherals()
        .await
        .map_err(|err| err.to_string())?
        .into_iter()
        .find(|peripheral| peripheral.id().to_string() == id)
        .ok_or_else(|| "the connected device is no longer available".to_string())
}

/// Subscribes to wifi_state and forwards notifications to the frontend.
///
/// Devices without the characteristic — the stock `bleprph` example, say —
/// connect normally and simply never report; that keeps `ble_connect` usable
/// against firmware that predates this service.
async fn watch_wifi_state(app: &AppHandle, peripheral: &Peripheral) {
    let Some(characteristic) = find_characteristic(peripheral, WIFI_STATE_CHR_UUID) else {
        return;
    };

    if let Err(err) = peripheral.subscribe(&characteristic).await {
        eprintln!("ble: could not subscribe to wifi_state: {err}");
        return;
    }

    let mut notifications = match peripheral.notifications().await {
        Ok(stream) => stream,
        Err(err) => {
            eprintln!("ble: could not open the notification stream: {err}");
            return;
        }
    };

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Ends on its own when the peripheral disconnects.
        while let Some(notification) = notifications.next().await {
            if notification.uuid != WIFI_STATE_CHR_UUID {
                continue;
            }
            if let Some(event) = parse_wifi_state(&notification.value) {
                let _ = app.emit(WIFI_EVENT, event);
            }
        }
    });
}

/// Lazily initializes the adapter and the central-event listener task.
async fn ensure_adapter(
    app: &AppHandle,
    inner: &mut BleInner,
    state: &BleState,
) -> Result<Adapter, String> {
    if inner.adapter.is_none() {
        let manager = Manager::new().await.map_err(|err| err.to_string())?;
        let adapter = manager
            .adapters()
            .await
            .map_err(|err| err.to_string())?
            .into_iter()
            .next()
            .ok_or("no Bluetooth adapter found")?;
        inner.adapter = Some(adapter);
    }
    let adapter = inner.adapter.clone().expect("adapter set above");

    if !inner.events_task_started {
        let mut events = adapter.events().await.map_err(|err| err.to_string())?;
        let task_adapter = adapter.clone();
        let task_app = app.clone();
        let task_connected = state.connected.clone();
        let task_scanning = state.scanning.clone();
        let task_generation = state.scan_generation.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.next().await {
                match event {
                    CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => {
                        if let Ok(peripheral) = task_adapter.peripheral(&id).await {
                            if let Ok(Some(props)) = peripheral.properties().await {
                                let _ = task_app.emit(
                                    DEVICE_EVENT,
                                    DiscoveredDevice {
                                        id: id.to_string(),
                                        name: props.local_name,
                                        rssi: props.rssi,
                                    },
                                );
                            }
                        }
                    }
                    CentralEvent::DeviceDisconnected(id) => {
                        let id = id.to_string();
                        let mut guard = task_connected.lock().unwrap();
                        let was_connected =
                            guard.as_ref().map(|dev| dev.id == id).unwrap_or(false);
                        if was_connected {
                            *guard = None;
                        }
                        drop(guard);
                        if was_connected {
                            emit_status(&task_app, "disconnected", Some(id), None, None);
                        }
                    }
                    CentralEvent::StateUpdate(CentralState::PoweredOff) => {
                        task_generation.fetch_add(1, Ordering::SeqCst);
                        if task_scanning.swap(false, Ordering::SeqCst) {
                            let _ = task_adapter.stop_scan().await;
                        }
                        emit_status(&task_app, "bluetoothOff", None, None, None);
                    }
                    _ => {}
                }
            }
        });
        inner.events_task_started = true;
    }

    Ok(adapter)
}

/// CoreBluetooth reports `Unknown` briefly after startup; give it a moment
/// before judging. Only a definite PoweredOff is treated as an error —
/// scanning still proceeds on Unknown rather than false-negative.
async fn check_powered_on(adapter: &Adapter) -> Result<(), String> {
    for _ in 0..10 {
        match adapter.adapter_state().await {
            Ok(CentralState::PoweredOn) => return Ok(()),
            Ok(CentralState::PoweredOff) => {
                return Err(
                    "Bluetooth is turned off — enable it in System Settings, then scan again."
                        .into(),
                )
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn ble_status(state: State<'_, BleState>) -> Result<BleSnapshot, String> {
    Ok(BleSnapshot {
        scanning: state.scanning.load(Ordering::SeqCst),
        connected: state.connected.lock().unwrap().clone(),
    })
}

#[tauri::command]
pub async fn ble_start_scan(app: AppHandle, state: State<'_, BleState>) -> Result<(), String> {
    let mut inner = state.inner.lock().await;
    let adapter = ensure_adapter(&app, &mut inner, &state).await?;
    check_powered_on(&adapter).await?;

    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|err| format!("failed to start scan: {err}"))?;
    state.scanning.store(true, Ordering::SeqCst);
    let generation = state.scan_generation.fetch_add(1, Ordering::SeqCst) + 1;
    emit_status(&app, "scanning", None, None, None);

    // Scans are battery- and radio-hungry; stop automatically after the TTL
    // unless this scan was already stopped (generation moved on).
    let ttl_app = app.clone();
    let ttl_adapter = adapter.clone();
    let ttl_scanning = state.scanning.clone();
    let ttl_generation = state.scan_generation.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SCAN_TTL).await;
        if ttl_generation.load(Ordering::SeqCst) == generation
            && ttl_scanning.swap(false, Ordering::SeqCst)
        {
            let _ = ttl_adapter.stop_scan().await;
            emit_status(&ttl_app, "scanStopped", None, None, Some("timeout"));
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn ble_stop_scan(app: AppHandle, state: State<'_, BleState>) -> Result<(), String> {
    let inner = state.inner.lock().await;
    state.scan_generation.fetch_add(1, Ordering::SeqCst);
    if state.scanning.swap(false, Ordering::SeqCst) {
        if let Some(adapter) = inner.adapter.clone() {
            let _ = adapter.stop_scan().await;
        }
    }
    emit_status(&app, "scanStopped", None, None, None);
    Ok(())
}

#[tauri::command]
pub async fn ble_connect(
    app: AppHandle,
    state: State<'_, BleState>,
    id: String,
) -> Result<ConnectedDevice, String> {
    let mut inner = state.inner.lock().await;
    let adapter = ensure_adapter(&app, &mut inner, &state).await?;

    state.scan_generation.fetch_add(1, Ordering::SeqCst);
    if state.scanning.swap(false, Ordering::SeqCst) {
        let _ = adapter.stop_scan().await;
        emit_status(&app, "scanStopped", None, None, None);
    }

    let peripheral = adapter
        .peripherals()
        .await
        .map_err(|err| err.to_string())?
        .into_iter()
        .find(|p| p.id().to_string() == id)
        .ok_or("device no longer available — scan again")?;

    peripheral
        .connect()
        .await
        .map_err(|err| format!("failed to connect: {err}"))?;
    peripheral
        .discover_services()
        .await
        .map_err(|err| format!("connected, but service discovery failed: {err}"))?;

    watch_wifi_state(&app, &peripheral).await;

    let name = peripheral
        .properties()
        .await
        .ok()
        .flatten()
        .and_then(|props| props.local_name);
    let device = ConnectedDevice { id, name };
    *state.connected.lock().unwrap() = Some(device.clone());
    emit_status(
        &app,
        "connected",
        Some(device.id.clone()),
        device.name.clone(),
        None,
    );
    Ok(device)
}

/// Hands the monocle the network it should join for the Wi-Fi data plane.
///
/// The passphrase crosses an encrypted, bonded link — the firmware marks
/// wifi_creds `WRITE_ENC`, so macOS pairs before the write lands and the
/// firmware re-checks encryption before accepting it. Progress comes back
/// asynchronously as `WIFI_EVENT`, not as this call's return value.
#[tauri::command]
pub async fn ble_set_wifi_credentials(
    app: AppHandle,
    state: State<'_, BleState>,
    ssid: String,
    password: String,
) -> Result<(), String> {
    let payload = encode_wifi_creds(&ssid, &password)?;
    let peripheral = connected_peripheral(&app, &state).await?;

    let characteristic = find_characteristic(&peripheral, WIFI_CREDS_CHR_UUID).ok_or(
        "this device has no Wi-Fi provisioning characteristic — is it running minicole firmware?",
    )?;

    // WithResponse so the firmware's ATT error (bad length, unencrypted link)
    // comes back to us instead of being dropped silently.
    peripheral
        .write(&characteristic, &payload, WriteType::WithResponse)
        .await
        .map_err(|err| format!("failed to send Wi-Fi credentials: {err}"))
}

#[tauri::command]
pub async fn ble_disconnect(app: AppHandle, state: State<'_, BleState>) -> Result<(), String> {
    let inner = state.inner.lock().await;
    let Some(device) = state.connected.lock().unwrap().clone() else {
        return Ok(());
    };
    if let Some(adapter) = inner.adapter.clone() {
        if let Ok(peripherals) = adapter.peripherals().await {
            if let Some(peripheral) =
                peripherals.into_iter().find(|p| p.id().to_string() == device.id)
            {
                let _ = peripheral.disconnect().await;
            }
        }
    }
    *state.connected.lock().unwrap() = None;
    emit_status(&app, "disconnected", Some(device.id), None, None);
    Ok(())
}

#[cfg(test)]
#[path = "../tests/ble_test.rs"]
mod tests;
