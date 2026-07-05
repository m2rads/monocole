import { AppSidebar } from "@/components/app-sidebar"
import { ChatView } from "@/components/chat-view"
import { ModelsView } from "@/components/models-view"
import { ModeToggle } from "@/components/mode-toggle"
import { Separator } from "@/components/ui/separator"
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar"
import { useSessions } from "@/hooks/use-sessions"
import { useView } from "@/hooks/use-view"

export default function Page() {
  const { activeSession } = useSessions()
  const { view } = useView()

  return (
    <SidebarProvider>
      <AppSidebar />
      <SidebarInset className="h-svh overflow-hidden">
        <header className="flex h-16 shrink-0 items-center gap-2 transition-[width,height] ease-linear group-has-data-[collapsible=icon]/sidebar-wrapper:h-12">
          <div className="flex items-center gap-2 px-4">
            <SidebarTrigger className="-ml-1" />
            <Separator
              orientation="vertical"
              className="mr-2 self-center! data-[orientation=vertical]:h-4"
            />
            <span className="truncate text-sm font-medium">
              {view === "models"
                ? "Settings · Models"
                : (activeSession?.title ?? "New Session")}
            </span>
          </div>
          <div className="ml-auto px-4">
            <ModeToggle />
          </div>
        </header>
        {view === "models" ? <ModelsView /> : <ChatView />}
      </SidebarInset>
    </SidebarProvider>
  )
}
