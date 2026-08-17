import * as React from "react"

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"

export type ChatMessage = {
  id: string
  role: "user" | "assistant"
  content: string
  error?: string
  /**
   * Set on a spoken turn while the words are still on their way: the monocle
   * is capturing, or whisper is working. Cleared when the transcript lands.
   * The bubble exists before its content does, so the user can see they were
   * heard rather than watching nothing happen for a few seconds.
   */
  voiceState?: "listening" | "transcribing"
}

export type Session = {
  id: string
  title: string
  /** true once the AI-generated title has been applied */
  titled: boolean
  messages: ChatMessage[]
  createdAt: number
}

type ChatStreamEvent = {
  sessionId: string
  kind: "token" | "done" | "error"
  content?: string
  message?: string
}

/** Mirrors VoiceEvent in src-tauri/src/voice.rs. */
type VoiceEvent = {
  kind: "listening" | "transcribing" | "transcript" | "ended" | "error"
  text?: string
  message?: string
}

type SessionsContextValue = {
  sessions: Session[]
  activeSession: Session | null
  streamingSessionId: string | null
  startNewSession: () => void
  selectSession: (id: string) => void
  deleteSession: (id: string) => void
  sendMessage: (content: string) => void
}

const SessionsContext = React.createContext<SessionsContextValue | null>(null)

export function SessionsProvider({ children }: { children: React.ReactNode }) {
  const [sessions, setSessions] = React.useState<Session[]>([])
  const [activeSessionId, setActiveSessionId] = React.useState<string | null>(
    null
  )
  const [streamingSessionId, setStreamingSessionId] = React.useState<
    string | null
  >(null)

  const sessionsRef = React.useRef(sessions)
  React.useEffect(() => {
    sessionsRef.current = sessions
  }, [sessions])

  // The voice listener is registered once and would otherwise close over a
  // stale value; it needs to know whether a generation is in flight right now.
  const streamingSessionIdRef = React.useRef(streamingSessionId)
  React.useEffect(() => {
    streamingSessionIdRef.current = streamingSessionId
  }, [streamingSessionId])
  const titleRequestsRef = React.useRef(new Set<string>())

  const updateSession = React.useCallback(
    (id: string, updater: (session: Session) => Session) => {
      setSessions((prev) =>
        prev.map((session) => (session.id === id ? updater(session) : session))
      )
    },
    []
  )

  const failStream = React.useCallback(
    (sessionId: string, message: string) => {
      updateSession(sessionId, (session) => {
        const messages = [...session.messages]
        const last = messages[messages.length - 1]
        if (last?.role === "assistant") {
          messages[messages.length - 1] = { ...last, error: message }
        }
        return { ...session, messages }
      })
      setStreamingSessionId((prev) => (prev === sessionId ? null : prev))
    },
    [updateSession]
  )

  const requestTitle = React.useCallback(
    (sessionId: string) => {
      const session = sessionsRef.current.find((s) => s.id === sessionId)
      if (
        !session ||
        session.titled ||
        titleRequestsRef.current.has(sessionId)
      ) {
        return
      }
      const firstUser = session.messages.find((m) => m.role === "user")
      const firstAssistant = session.messages.find(
        (m) => m.role === "assistant" && m.content && !m.error
      )
      if (!firstUser || !firstAssistant) return

      titleRequestsRef.current.add(sessionId)
      invoke<string>("generate_session_title", {
        messages: [
          { role: "user", content: firstUser.content },
          { role: "assistant", content: firstAssistant.content },
        ],
      })
        .then((title) => {
          if (title) {
            updateSession(sessionId, (s) => ({ ...s, title, titled: true }))
          }
        })
        .catch(() => {
          // Keep the provisional title; we'll retry after the next reply.
        })
        .finally(() => {
          titleRequestsRef.current.delete(sessionId)
        })
    },
    [updateSession]
  )

  React.useEffect(() => {
    const unlisten = listen<ChatStreamEvent>("chat-stream", (event) => {
      const { sessionId, kind, content, message } = event.payload
      if (kind === "token") {
        updateSession(sessionId, (session) => {
          const messages = [...session.messages]
          const last = messages[messages.length - 1]
          if (last?.role === "assistant") {
            messages[messages.length - 1] = {
              ...last,
              content: last.content + (content ?? ""),
            }
          }
          return { ...session, messages }
        })
      } else if (kind === "done") {
        setStreamingSessionId((prev) => (prev === sessionId ? null : prev))
        // Defer so the last token updates land in sessionsRef first.
        setTimeout(() => requestTitle(sessionId), 0)
      } else {
        failStream(sessionId, message ?? "Generation failed.")
      }
    })
    return () => {
      unlisten.then((fn) => fn())
    }
  }, [updateSession, failStream, requestTitle])

  /**
   * Appends an assistant placeholder and starts a generation.
   *
   * Shared by the composer and by voice, so a spoken turn and a typed one take
   * exactly the same path once there are words to send. Declared above the
   * voice listener because that effect closes over it.
   */
  const startStream = React.useCallback(
    (sessionId: string, history: { role: string; content: string }[]) => {
      updateSession(sessionId, (session) => ({
        ...session,
        messages: [
          ...session.messages,
          { id: crypto.randomUUID(), role: "assistant", content: "" },
        ],
      }))
      setStreamingSessionId(sessionId)
      invoke("chat_stream", { sessionId, messages: history }).catch((err) => {
        failStream(sessionId, String(err))
      })
    },
    [updateSession, failStream]
  )

  /**
   * Voice turns.
   *
   * The session is created lazily, on the first utterance rather than on
   * connect: BLE links drop, and a row per connection would litter the history
   * with empty sessions. Everything said while connected joins that same
   * session; disconnecting ends it, so the next conversation starts fresh.
   */
  React.useEffect(() => {
    // Which session spoken turns are joining, and the bubble currently waiting
    // for words. Refs because the listener is registered once and must not be
    // torn down and rebuilt on every state change.
    let voiceSessionId: string | null = null
    let pendingMessageId: string | null = null

    const setVoiceState = (state: ChatMessage["voiceState"]) => {
      if (!voiceSessionId || !pendingMessageId) return
      updateSession(voiceSessionId, (session) => ({
        ...session,
        messages: session.messages.map((message) =>
          message.id === pendingMessageId
            ? { ...message, voiceState: state }
            : message
        ),
      }))
    }

    /** Removes the placeholder when an utterance produced no words. */
    const dropPending = () => {
      if (!voiceSessionId || !pendingMessageId) return
      const messageId = pendingMessageId
      const sessionId = voiceSessionId
      pendingMessageId = null
      setSessions((prev) =>
        prev
          .map((session) =>
            session.id === sessionId
              ? {
                  ...session,
                  messages: session.messages.filter((m) => m.id !== messageId),
                }
              : session
          )
          // A session whose only turn came to nothing should not linger in
          // the history as an empty row.
          .filter((session) => session.messages.length > 0)
      )
    }

    const unlistenVoice = listen<VoiceEvent>("voice-session", (event) => {
      const { kind, text, message } = event.payload

      if (kind === "listening") {
        // Refuse rather than queue: llama-server handles one generation, and
        // sendMessage already guards typed input the same way.
        if (streamingSessionIdRef.current !== null) return

        const pending: ChatMessage = {
          id: crypto.randomUUID(),
          role: "user",
          content: "",
          voiceState: "listening",
        }
        pendingMessageId = pending.id

        if (voiceSessionId === null) {
          const session: Session = {
            id: crypto.randomUUID(),
            title: "Voice session",
            titled: false,
            messages: [pending],
            createdAt: Date.now(),
          }
          voiceSessionId = session.id
          setSessions((prev) => [session, ...prev])
          setActiveSessionId(session.id)
        } else {
          updateSession(voiceSessionId, (session) => ({
            ...session,
            messages: [...session.messages, pending],
          }))
        }
        return
      }

      if (kind === "transcribing") {
        setVoiceState("transcribing")
        return
      }

      if (kind === "transcript" && text && voiceSessionId && pendingMessageId) {
        const sessionId = voiceSessionId
        const messageId = pendingMessageId
        pendingMessageId = null

        updateSession(sessionId, (session) => ({
          ...session,
          // Name the session from what was actually said, replacing the
          // provisional title, exactly as typing does.
          title: session.titled
            ? session.title
            : text.length > 40
              ? `${text.slice(0, 40)}…`
              : text,
          messages: session.messages.map((m) =>
            m.id === messageId
              ? { ...m, content: text, voiceState: undefined }
              : m
          ),
        }))

        const existing = sessionsRef.current.find((s) => s.id === sessionId)
        const history = [
          ...(existing?.messages ?? [])
            .filter((m) => m.id !== messageId && m.content && !m.error)
            .map((m) => ({ role: m.role, content: m.content })),
          { role: "user", content: text },
        ]
        startStream(sessionId, history)
        return
      }

      if (kind === "error") {
        if (!voiceSessionId || !pendingMessageId) return
        const messageId = pendingMessageId
        pendingMessageId = null
        // One update, not a clear followed by a set: the bubble stops waiting
        // and reports the failure in the same render.
        updateSession(voiceSessionId, (session) => ({
          ...session,
          messages: session.messages.map((m) =>
            m.id === messageId
              ? {
                  ...m,
                  voiceState: undefined,
                  error: message ?? "Voice capture failed.",
                }
              : m
          ),
        }))
        return
      }

      if (kind === "ended") {
        // Nothing worth transcribing — a false trigger, or someone testing the
        // wake word. Leave no trace.
        dropPending()
      }
    })

    const unlistenBle = listen<{ kind: string }>("ble-status", (event) => {
      if (event.payload.kind === "connected") {
        voiceSessionId = null
      }
      if (event.payload.kind === "disconnected") {
        dropPending()
        voiceSessionId = null
      }
    })

    return () => {
      unlistenVoice.then((fn) => fn())
      unlistenBle.then((fn) => fn())
    }
  }, [updateSession, startStream])

  const startNewSession = React.useCallback(() => {
    setActiveSessionId(null)
  }, [])

  const selectSession = React.useCallback((id: string) => {
    setActiveSessionId(id)
  }, [])

  const deleteSession = React.useCallback((id: string) => {
    setSessions((prev) => prev.filter((session) => session.id !== id))
    setActiveSessionId((prev) => (prev === id ? null : prev))
    setStreamingSessionId((prev) => (prev === id ? null : prev))
  }, [])

  const sendMessage = React.useCallback(
    (content: string) => {
      const trimmed = content.trim()
      if (!trimmed) return
      if (streamingSessionId !== null) return

      const userMessage: ChatMessage = {
        id: crypto.randomUUID(),
        role: "user",
        content: trimmed,
      }
      const assistantMessage: ChatMessage = {
        id: crypto.randomUUID(),
        role: "assistant",
        content: "",
      }

      let sessionId = activeSessionId
      let history: { role: string; content: string }[]

      if (sessionId === null) {
        sessionId = crypto.randomUUID()
        const session: Session = {
          id: sessionId,
          title: trimmed.length > 40 ? `${trimmed.slice(0, 40)}…` : trimmed,
          titled: false,
          messages: [userMessage, assistantMessage],
          createdAt: Date.now(),
        }
        setSessions((prev) => [session, ...prev])
        setActiveSessionId(sessionId)
        history = [{ role: "user", content: trimmed }]
      } else {
        const existing = sessionsRef.current.find((s) => s.id === sessionId)
        history = [
          ...(existing?.messages ?? [])
            .filter((m) => m.content && !m.error)
            .map((m) => ({ role: m.role, content: m.content })),
          { role: "user", content: trimmed },
        ]
        updateSession(sessionId, (session) => ({
          ...session,
          messages: [...session.messages, userMessage, assistantMessage],
        }))
      }

      setStreamingSessionId(sessionId)
      invoke("chat_stream", { sessionId, messages: history }).catch((err) => {
        failStream(sessionId, String(err))
      })
    },
    [activeSessionId, streamingSessionId, updateSession, failStream]
  )

  const activeSession =
    sessions.find((session) => session.id === activeSessionId) ?? null

  const value = React.useMemo(
    () => ({
      sessions,
      activeSession,
      streamingSessionId,
      startNewSession,
      selectSession,
      deleteSession,
      sendMessage,
    }),
    [
      sessions,
      activeSession,
      streamingSessionId,
      startNewSession,
      selectSession,
      deleteSession,
      sendMessage,
    ]
  )

  return (
    <SessionsContext.Provider value={value}>
      {children}
    </SessionsContext.Provider>
  )
}

export function useSessions() {
  const context = React.useContext(SessionsContext)
  if (!context) {
    throw new Error("useSessions must be used within a SessionsProvider.")
  }
  return context
}
