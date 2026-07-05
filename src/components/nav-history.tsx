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
import { ChevronUpIcon, MoreHorizontalIcon, Trash2Icon } from "lucide-react"

const MAX_VISIBLE = 7

export function NavHistory() {
  const { isMobile } = useSidebar()
  const { sessions, activeSession, selectSession, deleteSession } =
    useSessions()
  const [expanded, setExpanded] = React.useState(false)
  const listRef = React.useRef<HTMLDivElement>(null)

  function collapse() {
    setExpanded(false)
    listRef.current?.scrollTo({ top: 0 })
  }

  const overflowing = sessions.length > MAX_VISIBLE
  const visibleSessions =
    expanded || !overflowing ? sessions : sessions.slice(0, MAX_VISIBLE)

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
          {overflowing && (
            <SidebarMenuItem>
              <SidebarMenuButton
                className="text-sidebar-foreground/70"
                onClick={() => (expanded ? collapse() : setExpanded(true))}
              >
                {expanded ? (
                  <ChevronUpIcon className="text-sidebar-foreground/70" />
                ) : (
                  <MoreHorizontalIcon className="text-sidebar-foreground/70" />
                )}
                <span>{expanded ? "Less" : "More"}</span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          )}
        </SidebarMenu>
      </div>
    </SidebarGroup>
  )
}
