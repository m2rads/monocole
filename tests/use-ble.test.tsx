import { act, renderHook, waitFor } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { useBle } from "@/hooks/use-ble"

import { emitTauriEvent, invokeCalls, invokeMock } from "./tauri-mocks"

function mockSnapshot(snapshot?: {
  scanning?: boolean
  connected?: { id: string; name: string | null } | null
}) {
  invokeMock.mockImplementation(async (cmd) =>
    cmd === "ble_status"
      ? ({
          scanning: snapshot?.scanning ?? false,
          connected: snapshot?.connected ?? null,
        } as never)
      : (undefined as never)
  )
}

describe("useBle", () => {
  it("loads the initial snapshot", async () => {
    mockSnapshot({
      scanning: true,
      connected: { id: "dev-1", name: "minicole-monocle" },
    })
    const { result } = renderHook(() => useBle())

    await waitFor(() => expect(result.current.scanning).toBe(true))
    expect(result.current.connected).toEqual({
      id: "dev-1",
      name: "minicole-monocle",
    })
  })

  it("collects discovered devices, deduplicates, and sorts by signal", async () => {
    mockSnapshot()
    const { result } = renderHook(() => useBle())

    act(() => {
      emitTauriEvent("ble-device", { id: "a", name: "far", rssi: -80 })
      emitTauriEvent("ble-device", { id: "b", name: "near", rssi: -40 })
      // Update for a known device replaces rssi but keeps identity.
      emitTauriEvent("ble-device", { id: "a", name: "far", rssi: -70 })
      // A later nameless advertisement must not erase a known name.
      emitTauriEvent("ble-device", { id: "b", name: null, rssi: -42 })
    })

    expect(result.current.devices.map((d) => d.id)).toEqual(["b", "a"])
    expect(result.current.devices[0].name).toBe("near")
    expect(result.current.devices[1].rssi).toBe(-70)
  })

  it("starts a scan and reflects scanning status events", async () => {
    mockSnapshot()
    const { result } = renderHook(() => useBle())

    act(() => result.current.startScan())
    expect(invokeCalls("ble_start_scan")).toHaveLength(1)

    act(() => emitTauriEvent("ble-status", { kind: "scanning" }))
    expect(result.current.scanning).toBe(true)

    act(() =>
      emitTauriEvent("ble-status", { kind: "scanStopped", reason: "timeout" })
    )
    expect(result.current.scanning).toBe(false)
    expect(result.current.lastScanEnd).toBe("timeout")
  })

  it("clears devices and messages when a new scan starts", async () => {
    mockSnapshot()
    const { result } = renderHook(() => useBle())

    act(() => emitTauriEvent("ble-device", { id: "a", name: "x", rssi: -50 }))
    act(() =>
      emitTauriEvent("ble-status", { kind: "scanStopped", reason: "timeout" })
    )
    act(() => result.current.startScan())

    expect(result.current.devices).toHaveLength(0)
    expect(result.current.lastScanEnd).toBeNull()
  })

  it("connects and handles disconnects", async () => {
    mockSnapshot()
    const { result } = renderHook(() => useBle())

    act(() => result.current.connect("dev-1"))
    expect(result.current.connectingId).toBe("dev-1")
    expect(invokeCalls("ble_connect")[0][1]).toEqual({ id: "dev-1" })

    act(() =>
      emitTauriEvent("ble-status", {
        kind: "connected",
        deviceId: "dev-1",
        name: "minicole-monocle",
      })
    )
    expect(result.current.connected).toEqual({
      id: "dev-1",
      name: "minicole-monocle",
    })
    expect(result.current.connectingId).toBeNull()

    act(() =>
      emitTauriEvent("ble-status", { kind: "disconnected", deviceId: "dev-1" })
    )
    expect(result.current.connected).toBeNull()
  })

  it("surfaces bluetooth-off as an error and stops scanning", async () => {
    mockSnapshot()
    const { result } = renderHook(() => useBle())

    act(() => emitTauriEvent("ble-status", { kind: "scanning" }))
    act(() => emitTauriEvent("ble-status", { kind: "bluetoothOff" }))

    expect(result.current.scanning).toBe(false)
    expect(result.current.error).toMatch(/Bluetooth is turned off/)
  })
})
