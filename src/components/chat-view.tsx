import * as React from "react"

import { Loader2Icon, MicIcon, SquareIcon } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip"
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
      {/* The mic sits inside the field, anchored to the bottom right. Bottom
          rather than top because the textarea grows with its content, and a
          control that drifts down the page as you type reads as a different
          control each time. */}
      <div className="relative">
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
          // `min-h-11` is one line of text plus its padding and nothing more,
          // so the field hugs its content and the last line sits level with
          // the button rather than floating above it. The arithmetic holds at
          // any height: both the line and the button move down by exactly one
          // line-height as the field grows.
          //
          // The right padding is room for the button, so a long line never
          // runs underneath it — and more of it while the elapsed count is
          // alongside.
          className={cn(
            "min-h-11",
            recorder.status === "recording" ? "pr-20" : "pr-9"
          )}
          // A textarea is two rows tall by default, which would leave the
          // field a line taller than its content and put the button back below
          // the text. `field-sizing-content` still grows it as you type.
          rows={1}
          autoFocus
        />
        <div className="absolute right-0 bottom-2 flex items-center gap-1.5">
          {recorder.status === "recording" && (
            // The elapsed count is the honest signal that the microphone is
            // open — the icon alone could be mistaken for a hover state.
            <span className="text-xs text-destructive tabular-nums">
              {recorder.seconds.toFixed(1)}s
            </span>
          )}
          <DictateButton recorder={recorder} />
        </div>
      </div>
      {/* Only ever shown when something went wrong. The affordance itself is
          the icon and its tooltip, so a standing line of instructions under
          every composer would be noise. */}
      {recorder.error && (
        <p className="mt-2 text-xs text-destructive">{recorder.error}</p>
      )}
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
 * spinner while whisper works. Idle is deliberately quiet — a muted glyph that
 * only picks up a background on hover — but recording is not, because a
 * microphone that is open without the user realising is the failure worth
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
  // Whisper is working and there is nothing to press. Rendered outside the
  // tooltip because a disabled trigger never receives the hover that would
  // open one.
  if (recorder.status === "transcribing") {
    return (
      <Button
        type="button"
        size="icon-xs"
        variant="ghost"
        className="text-muted-foreground"
        disabled
        aria-label="Transcribing"
      >
        <Loader2Icon className="size-4 animate-spin" />
      </Button>
    )
  }

  const recording = recorder.status === "recording"

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <Button
            type="button"
            size="icon-xs"
            variant="ghost"
            onClick={recording ? recorder.stop : recorder.start}
            aria-label={recording ? "Stop dictating" : "Dictate"}
            className={cn(
              "text-muted-foreground hover:text-foreground",
              recording &&
                "text-destructive hover:bg-destructive/10 hover:text-destructive"
            )}
          >
            {recording ? (
              <SquareIcon className="size-3.5 fill-current" />
            ) : (
              <MicIcon className="size-4" />
            )}
          </Button>
        }
      />
      <TooltipContent>
        {recording ? "Stop dictating" : "Dictate"}
      </TooltipContent>
    </Tooltip>
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
