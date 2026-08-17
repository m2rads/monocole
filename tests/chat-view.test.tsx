import { act, fireEvent, render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { describe, expect, it, vi } from "vitest"

import { ChatView } from "@/components/chat-view"
import { SessionsProvider } from "@/hooks/use-sessions"

import { emitTauriEvent, invokeCalls, invokeMock } from "./tauri-mocks"

/** What the composer says when whisper heard no words. */
const NO_SPEECH = "Couldn't capture anything, try speaking louder."

function renderChat() {
  return render(
    <SessionsProvider>
      <ChatView />
    </SessionsProvider>
  )
}

/** Makes stop_recording return one transcript; everything else no-ops. */
function stubRecording(transcript: string) {
  invokeMock.mockImplementation((async (cmd: string) =>
    cmd === "stop_recording" ? transcript : undefined) as never)
}

function sessionIdFromLastCall() {
  const calls = invokeCalls("chat_stream")
  return (calls[calls.length - 1][1] as { sessionId: string }).sessionId
}

describe("ChatView", () => {
  it("shows the centered welcome state for a fresh session", () => {
    renderChat()

    expect(screen.getByText("Welcome to Minicole")).toBeInTheDocument()
    expect(
      screen.getByPlaceholderText("How can I help you?")
    ).toBeInTheDocument()
  })

  it("sends a message on Enter and renders streamed tokens", async () => {
    const user = userEvent.setup()
    renderChat()

    await user.type(
      screen.getByPlaceholderText("How can I help you?"),
      "Hello there{Enter}"
    )

    // The user bubble is rendered and the welcome state is gone.
    expect(screen.getByText("Hello there")).toBeInTheDocument()
    expect(screen.queryByText("Welcome to Minicole")).not.toBeInTheDocument()
    expect(invokeCalls("chat_stream")).toHaveLength(1)

    const sessionId = sessionIdFromLastCall()
    act(() => {
      emitTauriEvent("chat-stream", {
        sessionId,
        kind: "token",
        content: "Hi ",
      })
      emitTauriEvent("chat-stream", {
        sessionId,
        kind: "token",
        content: "back",
      })
    })
    expect(screen.getByText("Hi back")).toBeInTheDocument()
  })

  it("blocks sending while a response is streaming", async () => {
    const user = userEvent.setup()
    renderChat()

    await user.type(
      screen.getByPlaceholderText("How can I help you?"),
      "First{Enter}"
    )
    expect(screen.getByPlaceholderText("Generating…")).toBeInTheDocument()

    await user.type(screen.getByPlaceholderText("Generating…"), "Second{Enter}")
    expect(invokeCalls("chat_stream")).toHaveLength(1)

    // After the stream finishes, sending works again.
    const sessionId = sessionIdFromLastCall()
    act(() => {
      emitTauriEvent("chat-stream", { sessionId, kind: "token", content: "ok" })
      emitTauriEvent("chat-stream", { sessionId, kind: "done" })
    })
    await user.clear(screen.getByPlaceholderText("How can I help you?"))
    await user.type(
      screen.getByPlaceholderText("How can I help you?"),
      "Third{Enter}"
    )
    expect(invokeCalls("chat_stream")).toHaveLength(2)
  })

  it("dictates into the composer rather than sending straight away", async () => {
    const user = userEvent.setup()
    stubRecording("spoken words")
    renderChat()

    await user.click(screen.getByLabelText("Dictate"))
    // The open microphone is visible as such, and the control now stops it.
    const stop = await screen.findByLabelText("Stop dictating")

    await user.click(stop)
    expect(await screen.findByDisplayValue("spoken words")).toBeInTheDocument()
    // Nothing was sent — the transcript is editable first.
    expect(invokeCalls("chat_stream")).toHaveLength(0)
  })

  it("says so when the microphone heard no words", async () => {
    const user = userEvent.setup()
    // Rust flattens whisper's "[BLANK_AUDIO]" to nothing, so an empty
    // transcript is the signal — and it has to be said, or a recording that
    // produced no words looks like the app ignoring you.
    stubRecording("")
    renderChat()

    await user.click(screen.getByLabelText("Dictate"))
    await user.click(await screen.findByLabelText("Stop dictating"))

    expect(await screen.findByText(NO_SPEECH)).toBeInTheDocument()
    // And nothing lands in the composer.
    expect(screen.getByPlaceholderText("How can I help you?")).toHaveValue("")
  })

  it("retires the failure notice on its own", async () => {
    stubRecording("")
    renderChat()

    // fireEvent rather than userEvent: userEvent's own waiting has to be
    // taught about a faked clock, and the clock is the thing under test here.
    fireEvent.click(screen.getByLabelText("Dictate"))
    const stop = await screen.findByLabelText("Stop dictating")

    // Faked before the failure, so the effect's timeout is one this test owns.
    vi.useFakeTimers()
    await act(async () => {
      fireEvent.click(stop)
    })
    expect(screen.getByText(NO_SPEECH)).toBeInTheDocument()

    // Still there a moment later — long enough to read, not a flash.
    act(() => vi.advanceTimersByTime(4000))
    expect(screen.getByText(NO_SPEECH)).toBeInTheDocument()

    act(() => vi.advanceTimersByTime(1000))
    expect(screen.queryByText(NO_SPEECH)).not.toBeInTheDocument()
  })

  it("renders stream errors inline", async () => {
    const user = userEvent.setup()
    renderChat()

    await user.type(
      screen.getByPlaceholderText("How can I help you?"),
      "Hello{Enter}"
    )
    const sessionId = sessionIdFromLastCall()
    act(() => {
      emitTauriEvent("chat-stream", {
        sessionId,
        kind: "error",
        message: "No model selected.",
      })
    })

    expect(screen.getByText("No model selected.")).toBeInTheDocument()
  })
})
