import * as React from "react"

export type View = "chat" | "models" | "connection"

type ViewContextValue = {
  view: View
  setView: (view: View) => void
}

const ViewContext = React.createContext<ViewContextValue | null>(null)

export function ViewProvider({ children }: { children: React.ReactNode }) {
  const [view, setView] = React.useState<View>("chat")
  const value = React.useMemo(() => ({ view, setView }), [view])
  return <ViewContext.Provider value={value}>{children}</ViewContext.Provider>
}

export function useView() {
  const context = React.useContext(ViewContext)
  if (!context) {
    throw new Error("useView must be used within a ViewProvider.")
  }
  return context
}
