import * as React from "react"

export type ChatMessage = {
  id: string
  role: "user" | "assistant"
  content: string
}

export type Session = {
  id: string
  title: string
  messages: ChatMessage[]
  createdAt: number
}

type SessionsContextValue = {
  sessions: Session[]
  activeSession: Session | null
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

  const startNewSession = React.useCallback(() => {
    setActiveSessionId(null)
  }, [])

  const selectSession = React.useCallback((id: string) => {
    setActiveSessionId(id)
  }, [])

  const deleteSession = React.useCallback((id: string) => {
    setSessions((prev) => prev.filter((session) => session.id !== id))
    setActiveSessionId((prev) => (prev === id ? null : prev))
  }, [])

  const sendMessage = React.useCallback(
    (content: string) => {
      const trimmed = content.trim()
      if (!trimmed) return
      const message: ChatMessage = {
        id: crypto.randomUUID(),
        role: "user",
        content: trimmed,
      }
      if (activeSessionId === null) {
        const session: Session = {
          id: crypto.randomUUID(),
          title: trimmed.length > 40 ? `${trimmed.slice(0, 40)}…` : trimmed,
          messages: [message],
          createdAt: Date.now(),
        }
        setSessions((prev) => [session, ...prev])
        setActiveSessionId(session.id)
      } else {
        setSessions((prev) =>
          prev.map((session) =>
            session.id === activeSessionId
              ? { ...session, messages: [...session.messages, message] }
              : session
          )
        )
      }
    },
    [activeSessionId]
  )

  const activeSession =
    sessions.find((session) => session.id === activeSessionId) ?? null

  const value = React.useMemo(
    () => ({
      sessions,
      activeSession,
      startNewSession,
      selectSession,
      deleteSession,
      sendMessage,
    }),
    [
      sessions,
      activeSession,
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
