//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.


import { StrictMode } from 'react'
import ReactDOM from 'react-dom/client'
import { AxiosError } from 'axios'
import {
  QueryCache,
  QueryClient,
  QueryClientProvider,
} from '@tanstack/react-query'
import { RouterProvider, createRouter } from '@tanstack/react-router'
import { setToken, WEBUITOKEN } from '@/stores/authStore'
import { clearSessionState, clearSessionStorage } from '@/lib/session'
import { toast } from '@/hooks/use-toast'
import { ThemeProvider } from './context/theme-context'
import './index.css'
import './i18n'
// Generated Routes
import { routeTree } from './routeTree.gen'
import { ToastAction } from './components/ui/toast'
import i18n from './i18n'


const handleAxiosError = (error: any) => {
  if (!(error instanceof AxiosError)) return;

  switch (error.response?.status) {
    case 401:
      // The token expired: drop everything the previous session cached so a
      // different account signing in here cannot see it (even via Back).
      clearSessionState(queryClient);
      // No `redirect` back to the expired session's page: whoever signs in
      // next starts at the default page instead of inheriting that URL.
      if (router.history.location.pathname !== '/sign-in') {
        router.navigate({ to: '/sign-in' });
      }
      break;
    case 403:
      toast({
        variant: 'destructive',
        title: "Forbidden",
        description: error.response.data.message,
        action: <ToastAction altText={i18n.t('common.close')}>{i18n.t('common.close')}</ToastAction>,
      });
      break;
    case 500:
      toast({
        variant: 'destructive',
        title: i18n.t('errors.internalServerError'),
      });
      router.navigate({ to: '/500' });
      break;
    case 304:
      toast({
        variant: 'destructive',
        title: i18n.t('errors.contentNotModified'),
      });
      break;
    default:
      if (error.code === "ERR_NETWORK") {
        toast({
          variant: "destructive",
          title: i18n.t('errors.networkError'),
          description: i18n.t('errors.networkErrorDesc'),
          action: <ToastAction altText={i18n.t('common.tryAgain')}>{i18n.t('common.tryAgain')}</ToastAction>,
        });
      }
  }
};



const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        // eslint-disable-next-line no-console
        if (import.meta.env.DEV) console.log({ failureCount, error })

        if (failureCount >= 0 && import.meta.env.DEV) return false
        if (failureCount > 3 && import.meta.env.PROD) return false

        return !(
          error instanceof AxiosError &&
          [401, 403].includes(error.response?.status ?? 0)
        )
      },
      refetchOnWindowFocus: import.meta.env.PROD,
      staleTime: 10 * 1000, // 10s
    },
    mutations: {
      onError: (error) => {
        handleAxiosError(error)
      },
    },
  },
  queryCache: new QueryCache({
    onError: (error) => {
      handleAxiosError(error)
    },
  }),
})

const basepath = (window as any).__BICHON_BASE__ || '/';
console.log('Current Basepath:', basepath);

// Back-forward cache: a restored page is a frozen snapshot of whoever was
// signed in when it was left. Reload so the current token — possibly a
// different account — decides what is rendered.
window.addEventListener('pageshow', (event) => {
  if (event.persisted) window.location.reload();
});

// Another tab signing in or out swaps the shared token; this tab's rendered
// state and query cache belong to the previous account, so start over.
window.addEventListener('storage', (event) => {
  if (event.key === null || event.key === WEBUITOKEN) {
    window.location.reload();
  }
});

// OIDC SSO callback: the Pro server redirects back with ?access_token=.
// Store it and strip it from the URL before the router/auth flow runs.
const ssoToken = new URLSearchParams(window.location.search).get('access_token');
if (ssoToken) {
  clearSessionStorage();
  queryClient.removeQueries();
  setToken({ success: true, access_token: ssoToken });
  const clean = `${window.location.pathname}${window.location.hash}`;
  window.history.replaceState(null, '', clean);
}

// Create a new router instance
const router = createRouter({
  routeTree,
  basepath,
  context: { queryClient },
  defaultPreload: 'intent',
  defaultPreloadStaleTime: 0,
})

// Register the router instance for type safety
declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

// Render the app
const rootElement = document.getElementById('root')!
if (!rootElement.innerHTML) {
  const root = ReactDOM.createRoot(rootElement)
  root.render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <ThemeProvider defaultTheme='light' storageKey='vite-ui-theme'>
          <RouterProvider router={router} />
        </ThemeProvider>
      </QueryClientProvider>
    </StrictMode>
  )
}
