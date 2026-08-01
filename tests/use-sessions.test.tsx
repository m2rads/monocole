import { act, renderHook, waitFor } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SessionsProvider, useSessions } from "@/hooks/use-sessions"

import { emitTauriEvent, invokeCalls, invokeMock } from "./tauri-mocks"

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <SessionsProvider>{children}</SessionsProvider>
)

function lastChatStreamArgs() {
  const calls = invokeCalls("chat_stream")
  return calls[calls.length - 1]?.[1] as {
    sessionId: string
    messages: { role: string; content: string }[]
  }
}

async function flushTimers() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 5))
  })
}

describe("useSessions", () => {
  it("creates a session on the first message and streams tokens into it", async () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    act(() => result.current.sendMessage("  Hello world  "))

    expect(result.current.sessions).toHaveLength(1)
    const session = result.current.sessions[0]
    expect(session.title).toBe("Hello world")
    expect(session.messages.map((m) => m.role)).toEqual(["user", "assistant"])
    expect(result.current.activeSession?.id).toBe(session.id)
    expect(result.current.streamingSessionId).toBe(session.id)

    const args = lastChatStreamArgs()
    expect(args.sessionId).toBe(session.id)
    expect(args.messages).toEqual([{ role: "user", content: "Hello world" }])

    act(() => {
      emitTauriEvent("chat-stream", {
        sessionId: session.id,
        kind: "token",
        content: "Hi",
      })
      emitTauriEvent("chat-stream", {
        sessionId: session.id,
        kind: "token",
        content: " there",
      })
    })
    expect(result.current.sessions[0].messages[1].content).toBe("Hi there")

    act(() => {
      emitTauriEvent("chat-stream", { sessionId: session.id, kind: "done" })
    })
    expect(result.current.streamingSessionId).toBeNull()

    // Let the deferred title request settle inside this test so it can't
    // leak into the next one's invoke log.
    await flushTimers()
  })

  it("generates an AI title after the first completed exchange", async () => {
    invokeMock.mockImplementation(async (cmd) =>
      cmd === "generate_session_title"
        ? ("Greeting Chat" as never)
        : (undefined as never)
    )
    const { result } = renderHook(() => useSessions(), { wrapper })

    act(() => result.current.sendMessage("Hello"))
    const sessionId = lastChatStreamArgs().sessionId
    act(() => {
      emitTauriEvent("chat-stream", {
        sessionId,
        kind: "token",
        content: "Hey",
      })
      emitTauriEvent("chat-stream", { sessionId, kind: "done" })
    })
    await flushTimers()

    await waitFor(() => {
      expect(result.current.sessions[0].title).toBe("Greeting Chat")
    })
    expect(result.current.sessions[0].titled).toBe(true)
    const titleArgs = invokeCalls("generate_session_title")[0][1] as {
      messages: { role: string; content: string }[]
    }
    expect(titleArgs.messages).toEqual([
      { role: "user", content: "Hello" },
      { role: "assistant", content: "Hey" },
    ])
  })

  it("sends the full history on follow-up messages", async () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    act(() => result.current.sendMessage("First"))
    const sessionId = lastChatStreamArgs().sessionId
    act(() => {
      emitTauriEvent("chat-stream", {
        sessionId,
        kind: "token",
        content: "Reply",
      })
      emitTauriEvent("chat-stream", { sessionId, kind: "done" })
    })
    await flushTimers()

    act(() => result.current.sendMessage("Second"))

    expect(lastChatStreamArgs().messages).toEqual([
      { role: "user", content: "First" },
      { role: "assistant", content: "Reply" },
      { role: "user", content: "Second" },
    ])
    expect(result.current.sessions[0].messages).toHaveLength(4)
  })

  it("marks the assistant message on stream errors", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    act(() => result.current.sendMessage("Hello"))
    const sessionId = lastChatStreamArgs().sessionId
    act(() => {
      emitTauriEvent("chat-stream", {
        sessionId,
        kind: "error",
        message: "No model selected.",
      })
    })

    expect(result.current.sessions[0].messages[1].error).toBe(
      "No model selected."
    )
    expect(result.current.streamingSessionId).toBeNull()
  })

  it("ignores new messages while a response is streaming", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    act(() => result.current.sendMessage("First"))
    act(() => result.current.sendMessage("Second"))

    expect(invokeCalls("chat_stream")).toHaveLength(1)
    expect(result.current.sessions[0].messages).toHaveLength(2)
  })

  it("ignores blank messages", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    act(() => result.current.sendMessage("   "))

    expect(result.current.sessions).toHaveLength(0)
    expect(invokeCalls("chat_stream")).toHaveLength(0)
  })

  it("deletes sessions and clears the active selection", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    act(() => result.current.sendMessage("Hello"))
    const sessionId = result.current.sessions[0].id

    act(() => result.current.deleteSession(sessionId))

    expect(result.current.sessions).toHaveLength(0)
    expect(result.current.activeSession).toBeNull()
    expect(result.current.streamingSessionId).toBeNull()
  })
})
