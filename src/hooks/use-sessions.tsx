import * as React from "react"

import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"

export type ChatMessage = {
  id: string
  role: "user" | "assistant"
  content: string
  error?: string
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
