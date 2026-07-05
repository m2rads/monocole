import Page from "@/dashboard/page"
import { SessionsProvider } from "@/hooks/use-sessions"
import { ViewProvider } from "@/hooks/use-view"

export function App() {
  return (
    <ViewProvider>
      <SessionsProvider>
        <Page />
      </SessionsProvider>
    </ViewProvider>
  )
}

export default App
