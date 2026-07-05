// Helpers for driving the mocked Tauri IPC layer (mocked in tests/setup.ts).

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import { vi } from "vitest"

export const invokeMock = vi.mocked(invoke)
export const listenMock = vi.mocked(listen)

type Handler = (event: { payload: unknown }) => void

const listeners = new Map<string, Set<Handler>>()

export function resetTauriMocks() {
  listeners.clear()
  invokeMock.mockReset()
  invokeMock.mockResolvedValue(undefined as never)
  listenMock.mockReset()
  listenMock.mockImplementation((async (event: string, handler: Handler) => {
    if (!listeners.has(event)) listeners.set(event, new Set())
    listeners.get(event)!.add(handler)
    return () => {
      listeners.get(event)?.delete(handler)
    }
  }) as typeof listen)
}

/** Fires a backend event to everyone listening, like app.emit in Rust. */
export function emitTauriEvent(event: string, payload: unknown) {
  listeners.get(event)?.forEach((handler) => handler({ payload }))
}

/** All invoke calls for one command, as [command, args] pairs. */
export function invokeCalls(command: string) {
  return invokeMock.mock.calls.filter(([cmd]) => cmd === command)
}
