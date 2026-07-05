"use client"

import * as React from "react"

import { NavMain } from "@/components/nav-main"
import { NavHistory } from "@/components/nav-history"
import { NavUser } from "@/components/nav-user"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from "@/components/ui/sidebar"
import { useSessions } from "@/hooks/use-sessions"
import { useView } from "@/hooks/use-view"
import { View, PlusIcon, Settings2Icon } from "lucide-react"

const user = {
  name: "shadcn",
  email: "m@example.com",
  avatar: "/avatars/shadcn.jpg",
}

export function AppSidebar({ ...props }: React.ComponentProps<typeof Sidebar>) {
  const { startNewSession } = useSessions()
  const { view, setView } = useView()

  const navMain = [
    {
      title: "New Session",
      url: "#",
      icon: <PlusIcon />,
      onClick: () => {
        startNewSession()
        setView("chat")
      },
    },
    {
      title: "Settings",
      url: "#",
      icon: <Settings2Icon />,
      items: [
        {
          title: "Models",
          url: "#",
          isActive: view === "models",
          onClick: () => setView("models"),
        },
        {
          title: "Connection",
          url: "#",
          isActive: view === "connection",
          onClick: () => setView("connection"),
        },
      ],
    },
  ]

  return (
    <Sidebar collapsible="icon" {...props}>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg">
              <div className="flex aspect-square size-8 items-center justify-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
                <View />
              </div>
              <div className="grid flex-1 text-left text-sm leading-tight">
                <span className="truncate font-medium">Minicole</span>
              </div>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <NavMain items={navMain} />
        <NavHistory />
      </SidebarContent>
      <SidebarFooter>
        <NavUser user={user} />
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  )
}
