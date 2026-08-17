import { act, renderHook } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { SessionsProvider, useSessions } from "@/hooks/use-sessions"

import { emitTauriEvent, invokeCalls } from "./tauri-mocks"

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <SessionsProvider>{children}</SessionsProvider>
)

/** Fires one voice-session event from the backend. */
function voice(kind: string, extra: Record<string, unknown> = {}) {
  act(() => emitTauriEvent("voice-session", { kind, ...extra }))
}

function ble(kind: string) {
  act(() => emitTauriEvent("ble-status", { kind }))
}

function lastChatStreamArgs() {
  const calls = invokeCalls("chat_stream")
  return calls[calls.length - 1]?.[1] as {
    sessionId: string
    messages: { role: string; content: string }[]
  }
}

describe("voice sessions", () => {
  it("creates no session until something is actually said", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    // Connecting arms voice but must not file anything: BLE links drop, and a
    // row per connection would litter the history.
    ble("connected")
    expect(result.current.sessions).toHaveLength(0)
  })

  it("shows a listening bubble on the first utterance", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")

    expect(result.current.sessions).toHaveLength(1)
    const message = result.current.sessions[0].messages[0]
    expect(message.role).toBe("user")
    expect(message.voiceState).toBe("listening")
    expect(message.content).toBe("")
    // The session is selected, so the user sees it happen.
    expect(result.current.activeSession?.id).toBe(result.current.sessions[0].id)
  })

  it("moves to transcribing, then fills in the transcript and generates", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")
    voice("transcribing")
    expect(result.current.sessions[0].messages[0].voiceState).toBe(
      "transcribing"
    )

    voice("transcript", { text: "what is the weather" })

    const session = result.current.sessions[0]
    expect(session.messages[0].content).toBe("what is the weather")
    expect(session.messages[0].voiceState).toBeUndefined()
    // Titled from what was said, replacing the provisional name.
    expect(session.title).toBe("what is the weather")
    // And it generates a reply exactly as a typed message would.
    expect(session.messages.map((m) => m.role)).toEqual(["user", "assistant"])
    expect(lastChatStreamArgs().messages).toEqual([
      { role: "user", content: "what is the weather" },
    ])
  })

  it("keeps later utterances in the same session while connected", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")
    voice("transcript", { text: "first question" })

    const sessionId = result.current.sessions[0].id
    act(() => emitTauriEvent("chat-stream", { sessionId, kind: "done" }))

    voice("listening")
    voice("transcript", { text: "second question" })

    expect(result.current.sessions).toHaveLength(1)
    // The follow-up carries the earlier turn as context.
    expect(lastChatStreamArgs().messages).toEqual([
      { role: "user", content: "first question" },
      { role: "user", content: "second question" },
    ])
  })

  it("starts a new session after a reconnect", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")
    voice("transcript", { text: "before" })
    const sessionId = result.current.sessions[0].id
    act(() => emitTauriEvent("chat-stream", { sessionId, kind: "done" }))

    ble("disconnected")
    ble("connected")
    voice("listening")
    voice("transcript", { text: "after" })

    expect(result.current.sessions).toHaveLength(2)
    expect(lastChatStreamArgs().messages).toEqual([
      { role: "user", content: "after" },
    ])
  })

  it("leaves no trace when an utterance produced no words", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")
    // A false trigger, or the wake word said into silence.
    voice("ended")

    expect(result.current.sessions).toHaveLength(0)
    expect(invokeCalls("chat_stream")).toHaveLength(0)
  })

  it("keeps earlier turns when a later utterance comes to nothing", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")
    voice("transcript", { text: "a real question" })
    const sessionId = result.current.sessions[0].id
    act(() => emitTauriEvent("chat-stream", { sessionId, kind: "done" }))

    voice("listening")
    voice("ended")

    // The session survives; only the empty bubble goes.
    expect(result.current.sessions).toHaveLength(1)
    expect(result.current.sessions[0].messages).toHaveLength(2)
  })

  it("reports capture errors on the bubble", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")
    voice("error", { message: "The monocle's microphone failed mid-sentence." })

    const message = result.current.sessions[0].messages[0]
    expect(message.error).toBe(
      "The monocle's microphone failed mid-sentence."
    )
    expect(message.voiceState).toBeUndefined()
    expect(invokeCalls("chat_stream")).toHaveLength(0)
  })

  it("ignores an utterance that arrives mid-generation", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    // A typed message is streaming; llama-server handles one at a time, and
    // sendMessage guards typed input the same way.
    act(() => result.current.sendMessage("typed question"))
    expect(result.current.streamingSessionId).not.toBeNull()

    ble("connected")
    voice("listening")

    expect(result.current.sessions).toHaveLength(1)
    expect(result.current.sessions[0].messages).toHaveLength(2)
  })

  it("drops a half-captured utterance when the link goes away", () => {
    const { result } = renderHook(() => useSessions(), { wrapper })

    ble("connected")
    voice("listening")
    ble("disconnected")

    expect(result.current.sessions).toHaveLength(0)
  })
})
