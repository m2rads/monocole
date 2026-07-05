"use client"

import * as React from "react"

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  SidebarGroup,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from "@/components/ui/sidebar"
import { useSessions } from "@/hooks/use-sessions"
import { cn } from "@/lib/utils"
import { MoreHorizontalIcon, Trash2Icon } from "lucide-react"

// SidebarMenuButton is h-8 (32px) and SidebarMenu uses gap-1 (4px).
const ITEM_HEIGHT = 32
const ITEM_GAP = 4

export function NavHistory() {
  const { isMobile } = useSidebar()
  const { sessions, activeSession, selectSession, deleteSession } =
    useSessions()
  const [expanded, setExpanded] = React.useState(false)
  const [maxVisible, setMaxVisible] = React.useState(Infinity)
  const listRef = React.useRef<HTMLDivElement>(null)

  React.useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    const update = () => {
      const rows = Math.floor(
        (el.clientHeight + ITEM_GAP) / (ITEM_HEIGHT + ITEM_GAP)
      )
      setMaxVisible(Math.max(rows, 2))
    }
    update()
    const observer = new ResizeObserver(update)
    observer.observe(el)
    return () => observer.disconnect()
  }, [])

  // Reserve one row for the More button when collapsed and overflowing.
  const overflowing = sessions.length > maxVisible
  const visibleSessions =
    expanded || !overflowing ? sessions : sessions.slice(0, maxVisible - 1)

  return (
    <SidebarGroup className="flex min-h-0 flex-1 flex-col group-data-[collapsible=icon]:hidden">
      <SidebarGroupLabel>History</SidebarGroupLabel>
      <div
        ref={listRef}
        className={cn(
          "min-h-0 flex-1",
          expanded ? "overflow-y-auto" : "overflow-hidden"
        )}
      >
        <SidebarMenu>
          {sessions.length === 0 && (
            <p className="px-2 py-1.5 text-xs text-muted-foreground">
              No sessions yet.
            </p>
          )}
          {visibleSessions.map((session) => (
            <SidebarMenuItem key={session.id}>
              <SidebarMenuButton
                isActive={session.id === activeSession?.id}
                onClick={() => selectSession(session.id)}
              >
                <span>{session.title}</span>
              </SidebarMenuButton>
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <SidebarMenuAction
                      showOnHover
                      className="aria-expanded:bg-muted"
                    />
                  }
                >
                  <MoreHorizontalIcon />
                  <span className="sr-only">More</span>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                  className="w-fit"
                  side={isMobile ? "bottom" : "right"}
                  align={isMobile ? "end" : "start"}
                >
                  <DropdownMenuItem
                    variant="destructive"
                    onClick={() => deleteSession(session.id)}
                  >
                    <Trash2Icon />
                    <span>Delete</span>
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </SidebarMenuItem>
          ))}
          {overflowing && !expanded && (
            <SidebarMenuItem>
              <SidebarMenuButton
                className="text-sidebar-foreground/70"
                onClick={() => setExpanded(true)}
              >
                <MoreHorizontalIcon className="text-sidebar-foreground/70" />
                <span>More</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          )}
        </SidebarMenu>
      </div>
    </SidebarGroup>
  )
}
