import "@testing-library/jest-dom/vitest"

import { beforeEach, vi } from "vitest"

import { resetTauriMocks } from "./tauri-mocks"

// The Tauri IPC modules don't exist outside a Tauri webview; every test runs
// against these mocks (driven via tests/tauri-mocks.ts).
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }))
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }))

// jsdom doesn't implement these browser APIs the app uses.
window.matchMedia = vi.fn().mockImplementation((query: string) => ({
  matches: false,
  media: query,
  onchange: null,
  addEventListener: vi.fn(),
  removeEventListener: vi.fn(),
  addListener: vi.fn(),
  removeListener: vi.fn(),
  dispatchEvent: vi.fn(),
}))
Element.prototype.scrollIntoView = vi.fn()
Element.prototype.scrollTo = vi.fn()

beforeEach(() => {
  resetTauriMocks()
})
