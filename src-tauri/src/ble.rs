use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::Duration;

use btleplug::api::{
    Central, CentralEvent, CentralState, Manager as _, Peripheral as _, ScanFilter,
};
use btleplug::platform::{Adapter, Manager};
use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

pub const DEVICE_EVENT: &str = "ble-device";
pub const STATUS_EVENT: &str = "ble-status";
const SCAN_TTL: Duration = Duration::from_secs(30);

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
