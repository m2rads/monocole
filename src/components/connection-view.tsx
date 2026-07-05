import * as React from "react"

import { Button } from "@/components/ui/button"
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { useBle, type DiscoveredDevice } from "@/hooks/use-ble"
import { cn } from "@/lib/utils"
import { BluetoothIcon, BluetoothConnectedIcon, LoaderIcon } from "lucide-react"

export function ConnectionView() {
  const {
    scanning,
    connected,
    devices,
    connectingId,
    error,
    lastScanEnd,
    startScan,
    stopScan,
    connect,
    disconnect,
  } = useBle()
  const [showUnnamed, setShowUnnamed] = React.useState(false)

  const visibleDevices = devices.filter((device) => showUnnamed || device.name)

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto flex w-full max-w-2xl flex-col gap-4 px-4 py-6">
        <div>
          <h2 className="text-sm font-medium">Connection</h2>
          <p className="text-sm text-muted-foreground">
            Pair your monocle device. Keep it powered on and nearby while
            scanning.
          </p>
        </div>
        {error && <p className="text-sm text-destructive">{error}</p>}

        <Card size="sm">
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              {connected ? (
                <BluetoothConnectedIcon className="size-4" />
              ) : (
                <BluetoothIcon className="size-4" />
              )}
              Monocle
              <span
                className={cn(
                  "size-2 rounded-full",
                  connected ? "bg-emerald-500" : "bg-muted-foreground/40"
                )}
              />
            </CardTitle>
            <CardDescription>
              {connected
                ? `Connected to ${connected.name ?? "unnamed device"}`
                : "Not connected."}
            </CardDescription>
            {connected && (
              <CardAction>
                <Button variant="outline" size="xs" onClick={disconnect}>
                  Disconnect
                </Button>
              </CardAction>
            )}
          </CardHeader>
          {connected && (
            <CardContent>
              <p className="truncate font-mono text-xs text-muted-foreground">
                {connected.id}
              </p>
            </CardContent>
          )}
        </Card>

        <Card size="sm">
          <CardHeader>
            <CardTitle>Nearby devices</CardTitle>
            <CardDescription>
              {scanning
                ? "Devices appear as they are discovered."
                : lastScanEnd === "timeout"
                  ? "Scan stopped after 30 seconds."
                  : "Start a scan to look for your monocle."}
            </CardDescription>
            <CardAction>
              <Button
                size="xs"
                variant={scanning ? "outline" : "default"}
                onClick={scanning ? stopScan : startScan}
              >
                {scanning && (
                  <LoaderIcon
                    data-icon="inline-start"
                    className="animate-spin"
                  />
                )}
                {scanning ? "Stop scanning" : "Scan"}
              </Button>
            </CardAction>
          </CardHeader>
          <CardContent className="flex flex-col gap-1">
            {visibleDevices.length === 0 ? (
              <p className="py-2 text-sm text-muted-foreground">
                {scanning
                  ? "No devices found yet."
                  : lastScanEnd === "timeout"
                    ? "Couldn't find your device. Make sure it's powered on and nearby."
                    : "Nothing discovered in the last scan."}
              </p>
            ) : (
              visibleDevices.map((device) => (
                <DeviceRow
                  key={device.id}
                  device={device}
                  connecting={connectingId === device.id}
                  connected={connected?.id === device.id}
                  disabled={connectingId !== null}
                  onConnect={() => connect(device.id)}
                />
              ))
            )}
            <button
              type="button"
              className="mt-2 self-start text-xs text-muted-foreground underline-offset-2 hover:underline"
              onClick={() => setShowUnnamed((prev) => !prev)}
            >
              {showUnnamed ? "Hide unnamed devices" : "Show unnamed devices"}
            </button>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

function DeviceRow({
  device,
  connecting,
  connected,
  disabled,
  onConnect,
}: {
  device: DiscoveredDevice
  connecting: boolean
  connected: boolean
  disabled: boolean
  onConnect: () => void
}) {
  return (
    <div className="flex items-center gap-3 border-b border-border/50 py-2 last:border-b-0">
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm">
          {device.name ?? (
            <span className="text-muted-foreground">Unnamed device</span>
          )}
        </p>
        <p className="truncate font-mono text-xs text-muted-foreground">
          {device.id}
        </p>
      </div>
      {device.rssi !== null && (
        <span className="shrink-0 font-mono text-xs text-muted-foreground">
          {device.rssi} dBm
        </span>
      )}
      {connected ? (
        <span className="text-xs text-muted-foreground">Connected</span>
      ) : (
        <Button
          variant="outline"
          size="xs"
          disabled={disabled}
          onClick={onConnect}
        >
          {connecting && (
            <LoaderIcon data-icon="inline-start" className="animate-spin" />
          )}
          {connecting ? "Connecting…" : "Connect"}
        </Button>
      )}
    </div>
  )
}
