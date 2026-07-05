import Page from "@/dashboard/page"
import { SessionsProvider } from "@/hooks/use-sessions"

export function App() {
  return (
    <SessionsProvider>
      <Page />
    </SessionsProvider>
  )
}

export default App
