import * as React from "react"

import { Textarea } from "@/components/ui/textarea"
import { useSessions } from "@/hooks/use-sessions"

export function ChatView() {
  const { activeSession, sendMessage } = useSessions()
  const [input, setInput] = React.useState("")
  const bottomRef = React.useRef<HTMLDivElement>(null)
  const textareaRef = React.useRef<HTMLTextAreaElement>(null)

  const messages = activeSession?.messages ?? []

  React.useEffect(() => {
    bottomRef.current?.scrollIntoView()
  }, [messages.length, activeSession?.id])

  React.useEffect(() => {
    textareaRef.current?.focus()
  }, [activeSession?.id])

  function submit() {
    if (!input.trim()) return
    sendMessage(input)
    setInput("")
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-2xl flex-col gap-4 px-4 py-6">
          {messages.length === 0 ? (
            <p className="py-12 text-center text-sm text-muted-foreground">
              Start a new conversation below.
            </p>
          ) : (
            messages.map((message) => (
              <div
                key={message.id}
                className={
                  message.role === "user"
                    ? "ml-auto max-w-[80%] rounded-lg bg-muted px-3 py-2 text-sm whitespace-pre-wrap"
                    : "max-w-[80%] text-sm whitespace-pre-wrap"
                }
              >
                {message.content}
              </div>
            ))
          )}
          <div ref={bottomRef} />
        </div>
      </div>
      <form
        className="mx-auto w-full max-w-2xl shrink-0 px-4 pb-6"
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
          placeholder="Type a message…"
          autoFocus
        />
      </form>
    </div>
  )
}
