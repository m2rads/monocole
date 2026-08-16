import * as React from "react"

import { Loader2Icon, MicIcon, SquareIcon } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import { useRecorder, type RecorderStatus } from "@/hooks/use-recorder"
import { useSessions, type ChatMessage } from "@/hooks/use-sessions"
import { cn } from "@/lib/utils"

export function ChatView() {
  const { activeSession, streamingSessionId, sendMessage } = useSessions()
  const [input, setInput] = React.useState("")
  const bottomRef = React.useRef<HTMLDivElement>(null)
  const textareaRef = React.useRef<HTMLTextAreaElement>(null)

  // Dictation lands in the composer rather than sending straight away: seeing
  // what whisper heard, and being able to fix it, matters more than one less
  // keystroke — especially while the model is the thing under test.
  const recorder = useRecorder(
    React.useCallback((text: string) => {
      setInput((current) => (current ? `${current} ${text}` : text))
      textareaRef.current?.focus()
    }, [])
  )

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
        placeholder={
          recorder.status === "recording"
            ? "Listening…"
            : recorder.status === "transcribing"
              ? "Transcribing…"
              : streaming
                ? "Generating…"
                : "How can I help you?"
        }
        autoFocus
      />
      {/* Its own row under the composer rather than floating inside it: the
          textarea is borderless by design, so a control overlapping it is
          genuinely hard to see. */}
      <div className="mt-2 flex items-center gap-2">
        <DictateButton recorder={recorder} />
        {recorder.error ? (
          <span className="text-xs text-destructive">{recorder.error}</span>
        ) : recorder.level !== null ? (
          // The number that says whether the microphone actually heard
          // anything. Whisper invents text when handed silence, so a quiet
          // recording is worth knowing about before blaming the model.
          <span className="text-xs text-muted-foreground">
            Recorded at level {Math.round(recorder.level)}
            {recorder.level < 400 && " — quiet; try speaking closer"}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">
            or dictate with your Mac's microphone
          </span>
        )}
      </div>
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

/**
 * Mic button for the composer.
 *
 * Three states in one control: press to record, press again to stop, then a
 * spinner while whisper works. Recording is deliberately loud visually — a
 * microphone that is on without the user realising is the failure worth
 * designing against.
 */
function DictateButton({
  recorder,
}: {
  recorder: {
    status: RecorderStatus
    seconds: number
    start: () => void
    stop: () => void
  }
}) {
  if (recorder.status === "transcribing") {
    return (
      <Button type="button" size="sm" variant="outline" disabled>
        <Loader2Icon data-icon="inline-start" className="animate-spin" />
        Transcribing…
      </Button>
    )
  }

  if (recorder.status === "recording") {
    return (
      <Button
        type="button"
        size="sm"
        variant="destructive"
        onClick={recorder.stop}
        className="tabular-nums"
      >
        <SquareIcon data-icon="inline-start" className="fill-current" />
        Stop · {recorder.seconds.toFixed(1)}s
      </Button>
    )
  }

  return (
    <Button type="button" size="sm" variant="outline" onClick={recorder.start}>
      <MicIcon data-icon="inline-start" />
      Dictate
    </Button>
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
    // A spoken turn whose words have not arrived yet. Showing the state
    // rather than an empty bubble is the difference between "it heard me"
    // and "nothing happened".
    if (message.voiceState) {
      return (
        <div className="ml-auto flex max-w-[80%] items-center gap-2 rounded-lg bg-muted px-3 py-2 text-sm text-muted-foreground">
          <MicIcon className="size-3.5 shrink-0" />
          <span className="animate-pulse">
            {message.voiceState === "listening"
              ? "Listening…"
              : "Transcribing…"}
          </span>
        </div>
      )
    }

    // A spoken turn can fail before it has any words — the mic, the link, or
    // transcription. Without this the bubble renders empty and the failure is
    // invisible, which looks like the app ignoring you.
    if (message.error) {
      return (
        <div className="ml-auto max-w-[80%] text-right text-sm">
          {message.content && (
            <p className="mb-1 rounded-lg bg-muted px-3 py-2 whitespace-pre-wrap">
              {message.content}
            </p>
          )}
          <p className="text-destructive">{message.error}</p>
        </div>
      )
    }

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
