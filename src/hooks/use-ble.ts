import * as React from "react"

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"

export type DiscoveredDevice = {
  id: string
  name: string | null
  rssi: number | null
  lastSeen: number
}

export type ConnectedDevice = {
  id: string
  name: string | null
}

type BleSnapshot = {
  scanning: boolean
  connected: ConnectedDevice | null
}

type StatusEvent = {
  kind:
    "scanning" | "scanStopped" | "connected" | "disconnected" | "bluetoothOff"
  deviceId?: string
  name?: string | null
  reason?: "timeout"
}

export type ScanEnd = "manual" | "timeout" | null

type DeviceEvent = {
  id: string
  name: string | null
  rssi: number | null
}

export function useBle() {
  const [scanning, setScanning] = React.useState(false)
  const [connected, setConnected] = React.useState<ConnectedDevice | null>(null)
  const [devices, setDevices] = React.useState<
    Record<string, DiscoveredDevice>
  >({})
  const [connectingId, setConnectingId] = React.useState<string | null>(null)
  const [error, setError] = React.useState<string | null>(null)
  const [lastScanEnd, setLastScanEnd] = React.useState<ScanEnd>(null)

  React.useEffect(() => {
    invoke<BleSnapshot>("ble_status")
      .then((snapshot) => {
        setScanning(snapshot.scanning)
        setConnected(snapshot.connected)
      })
      .catch((err) => setError(String(err)))

    const unlistenDevice = listen<DeviceEvent>("ble-device", (event) => {
      const { id, name, rssi } = event.payload
      setDevices((prev) => ({
        ...prev,
        [id]: {
          id,
          name: name ?? prev[id]?.name ?? null,
          rssi,
          lastSeen: Date.now(),
        },
      }))
    })
    const unlistenStatus = listen<StatusEvent>("ble-status", (event) => {
      const { kind, deviceId, name, reason } = event.payload
      if (kind === "scanning") setScanning(true)
      if (kind === "scanStopped") {
        setScanning(false)
        setLastScanEnd(reason === "timeout" ? "timeout" : "manual")
      }
      if (kind === "bluetoothOff") {
        setScanning(false)
        setConnectingId(null)
        setError(
          "Bluetooth is turned off — enable it in System Settings, then scan again."
        )
      }
      if (kind === "connected" && deviceId) {
        setConnected({ id: deviceId, name: name ?? null })
        setConnectingId(null)
        setError(null)
      }
      if (kind === "disconnected") {
        setConnected((prev) => (prev?.id === deviceId ? null : prev))
      }
    })
    return () => {
      unlistenDevice.then((fn) => fn())
      unlistenStatus.then((fn) => fn())
    }
  }, [])

  const startScan = React.useCallback(() => {
    setError(null)
    setLastScanEnd(null)
    setDevices({})
    invoke("ble_start_scan").catch((err) => setError(String(err)))
  }, [])

  const stopScan = React.useCallback(() => {
    invoke("ble_stop_scan").catch((err) => setError(String(err)))
  }, [])

  const connect = React.useCallback((id: string) => {
    setError(null)
    setConnectingId(id)
    invoke("ble_connect", { id }).catch((err) => {
      setError(String(err))
      setConnectingId(null)
    })
  }, [])

  const disconnect = React.useCallback(() => {
    invoke("ble_disconnect").catch((err) => setError(String(err)))
  }, [])

  const deviceList = React.useMemo(
    () =>
      Object.values(devices).sort(
        (a, b) => (b.rssi ?? -999) - (a.rssi ?? -999)
      ),
    [devices]
  )

  return {
    scanning,
    connected,
    devices: deviceList,
    connectingId,
    error,
    lastScanEnd,
    startScan,
    stopScan,
    connect,
    disconnect,
  }
}
