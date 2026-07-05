import { act, renderHook, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { describe, expect, it } from "vitest"

import { NavHistory } from "@/components/nav-history"
import { SidebarProvider } from "@/components/ui/sidebar"
import { SessionsProvider, useSessions } from "@/hooks/use-sessions"
import { ViewProvider } from "@/hooks/use-view"

import { emitTauriEvent, invokeCalls } from "./tauri-mocks"

// Renders the sidebar history next to a useSessions handle so tests can
// create sessions the same way the app does (send first message → session).
const wrapper = ({ children }: { children: React.ReactNode }) => (
  <ViewProvider>
    <SessionsProvider>
      <SidebarProvider>
        <NavHistory />
        {children}
      </SidebarProvider>
    </SessionsProvider>
  </ViewProvider>
)

function createSession(
  result: { current: ReturnType<typeof useSessions> },
  text: string
) {
  act(() => result.current.startNewSession())
  act(() => result.current.sendMessage(text))
  const calls = invokeCalls("chat_stream")
  const { sessionId } = calls[calls.length - 1][1] as { sessionId: string }
  act(() => emitTauriEvent("chat-stream", { sessionId, kind: "done" }))
}

describe("NavHistory", () => {
  it("shows an empty state without sessions", () => {
    renderHook(() => useSessions(), { wrapper })
    expect(screen.getByText("No sessions yet.")).toBeInTheDocument()
  })

  it("lists sessions as they are created", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    createSession(result, "chat one")
    createSession(result, "chat two")

    expect(screen.getByText("chat one")).toBeInTheDocument()
    expect(screen.getByText("chat two")).toBeInTheDocument()
    expect(screen.queryByText("More")).not.toBeInTheDocument()
  })

  it("caps the list at 7 items with More, and Less collapses again", async () => {
    const user = userEvent.setup()
    const { result } = renderHook(() => useSessions(), { wrapper })

    for (let i = 1; i <= 8; i++) {
      createSession(result, `chat ${i}`)
    }

    // Newest first: chats 8..2 visible, chat 1 hidden behind More.
    expect(screen.getAllByText(/^chat \d+$/)).toHaveLength(7)
    expect(screen.queryByText("chat 1")).not.toBeInTheDocument()

    await user.click(screen.getByText("More"))
    expect(screen.getAllByText(/^chat \d+$/)).toHaveLength(8)
    expect(screen.getByText("chat 1")).toBeInTheDocument()

    await user.click(screen.getByText("Less"))
    expect(screen.getAllByText(/^chat \d+$/)).toHaveLength(7)
    expect(screen.queryByText("chat 1")).not.toBeInTheDocument()
  })
})
