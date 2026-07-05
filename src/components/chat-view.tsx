import * as React from "react"

import { Textarea } from "@/components/ui/textarea"
import { useSessions, type ChatMessage } from "@/hooks/use-sessions"
import { cn } from "@/lib/utils"

export function ChatView() {
  const { activeSession, streamingSessionId, sendMessage } = useSessions()
  const [input, setInput] = React.useState("")
  const bottomRef = React.useRef<HTMLDivElement>(null)
  const textareaRef = React.useRef<HTMLTextAreaElement>(null)

  const messages = activeSession?.messages ?? []
  const streaming =
    streamingSessionId !== null && streamingSessionId === activeSession?.id
  const lastMessage = messages[messages.length - 1]

  React.useEffect(() => {
    bottomRef.current?.scrollIntoView()
  }, [messages.length, lastMessage?.content, activeSession?.id])

  React.useEffect(() => {
    textareaRef.current?.focus()
  }, [activeSession?.id])

  function submit() {
    if (!input.trim() || streamingSessionId !== null) return
    sendMessage(input)
    setInput("")
  }

  const composer = (
    <form
      className="w-full"
      onSubmit={(event) => {
        event.preventDefault()
        submit()
      }}
    >
      <Textarea
        ref={textareaRef}
        value={input}
        onChange={(event) => setInput(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey) {
            event.preventDefault()
            submit()
          }
        }}
        placeholder={streaming ? "Generating…" : "How can I help you?"}
        autoFocus
      />
    </form>
  )

  // Empty session: welcome message and composer centered in the window.
  if (messages.length === 0) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center px-4 pb-16">
        <div className="flex w-full max-w-2xl flex-col gap-6">
          <div className="text-center">
            <h2 className="font-heading text-lg font-semibold tracking-wider uppercase">
              Welcome to Minicole
            </h2>
            <p className="mt-1 text-sm text-muted-foreground">
              Connect your smart Monocole and start a session.
            </p>
          </div>
          {composer}
        </div>
      </div>
    )
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-2xl flex-col gap-4 px-4 py-6">
          {messages.map((message, index) => (
            <MessageBubble
              key={message.id}
              message={message}
              streaming={streaming && index === messages.length - 1}
            />
          ))}
          <div ref={bottomRef} />
        </div>
      </div>
      <div className="mx-auto w-full max-w-2xl shrink-0 px-4 pb-6">
        {composer}
      </div>
    </div>
  )
}

function MessageBubble({
  message,
  streaming,
}: {
  message: ChatMessage
  streaming: boolean
}) {
  if (message.role === "user") {
    return (
      <div className="ml-auto max-w-[80%] rounded-lg bg-muted px-3 py-2 text-sm whitespace-pre-wrap">
        {message.content}
      </div>
    )
  }

  if (message.error) {
    return (
      <div className="max-w-[80%] text-sm whitespace-pre-wrap">
        {message.content && <p className="mb-1">{message.content}</p>}
        <p className="text-destructive">{message.error}</p>
      </div>
    )
  }

  // Assistant placeholder before the first token arrives.
  if (!message.content && streaming) {
    return (
      <div className="flex h-6 items-center">
        <span className="size-2 animate-pulse rounded-full bg-muted-foreground" />
      </div>
    )
  }

  return (
    <div
      className={cn(
        "max-w-[80%] text-sm whitespace-pre-wrap",
        !message.content && "text-muted-foreground"
      )}
    >
      {message.content || "(no response)"}
      {streaming && (
        <span className="ml-0.5 inline-block h-3.5 w-1.5 animate-pulse bg-foreground align-middle" />
      )}
    </div>
  )
}
