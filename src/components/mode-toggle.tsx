import { MoonIcon, SunIcon } from "lucide-react"

import { Button } from "@/components/ui/button"
import { useTheme } from "@/components/theme-provider"

export function ModeToggle() {
  const { toggleTheme } = useTheme()

  return (
    <Button
      variant="ghost"
      size="icon-sm"
      className="relative"
      onClick={toggleTheme}
    >
      {/* transition-*! beats the global transition-kill the theme provider
          injects while switching themes, so the icons still animate. */}
      <SunIcon className="size-4 scale-100 rotate-0 transition-all! duration-500! ease-in-out! dark:scale-0 dark:-rotate-90" />
      <MoonIcon className="absolute size-4 scale-0 rotate-90 transition-all! duration-500! ease-in-out! dark:scale-100 dark:rotate-0" />
      <span className="sr-only">Toggle theme</span>
    </Button>
  )
}
