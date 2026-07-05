import { act, render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { describe, expect, it } from "vitest"

import { ChatView } from "@/components/chat-view"
import { SessionsProvider } from "@/hooks/use-sessions"

import { emitTauriEvent, invokeCalls } from "./tauri-mocks"

function renderChat() {
  return render(
    <SessionsProvider>
      <ChatView />
    </SessionsProvider>
  )
}

function sessionIdFromLastCall() {
  const calls = invokeCalls("chat_stream")
  return (calls[calls.length - 1][1] as { sessionId: string }).sessionId
}

describe("ChatView", () => {
  it("shows the centered welcome state for a fresh session", () => {
    renderChat()

    expect(screen.getByText("Welcome to Minicole")).toBeInTheDocument()
    expect(screen.getByPlaceholderText("How can I help you?")).toBeInTheDocument()
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
      emitTauriEvent("chat-stream", { sessionId, kind: "token", content: "Hi " })
      emitTauriEvent("chat-stream", { sessionId, kind: "token", content: "back" })
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
