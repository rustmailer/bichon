import * as React from 'react'

type SidebarContextValue = {
  state: 'expanded' | 'collapsed'
  open: boolean
  setOpen: (open: boolean) => void
  openMobile: boolean
  setOpenMobile: (open: boolean) => void
  isMobile: boolean
  toggleSidebar: () => void
}

// The context lives in its own module on purpose. Fast Refresh re-evaluates
// `sidebar.tsx` whenever one of its dependencies changes (an edit to
// `scroll-area.tsx`, for example), and a context created inside that module is
// then recreated while the mounted provider and consumers still hold the old
// one — which surfaced as an intermittent "useSidebar must be used within a
// SidebarProvider" crash. Keeping it here keeps the identity stable.
const SidebarContext = React.createContext<SidebarContextValue | null>(null)

/** Like `useSidebar`, but returns `null` outside a provider instead of throwing. */
function useOptionalSidebar() {
  return React.useContext(SidebarContext)
}

function useSidebar() {
  const context = useOptionalSidebar()
  if (!context) {
    throw new Error('useSidebar must be used within a SidebarProvider.')
  }

  return context
}

export { SidebarContext, useOptionalSidebar, useSidebar }
export type { SidebarContextValue }
