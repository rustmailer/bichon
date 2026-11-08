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


import * as React from 'react'
import Logo from '@/assets/logo.svg'
import { cn } from '@/lib/utils'

type BichonMascotProps = React.HTMLAttributes<HTMLDivElement>

const clamp = (value: number) => Math.max(-1, Math.min(1, value))

/**
 * The product logo, brought to life: it tilts in 3D toward the pointer with a
 * highlight that follows it, and idles with a slow bob. Using the logo artwork
 * itself keeps the login panel visually identical to the brand mark — a
 * redrawn mascot drifted too far from it.
 *
 * Decorative only: pointer-events-none and hidden from assistive tech. The
 * pointer tracking is skipped under `prefers-reduced-motion: reduce`.
 */
export function BichonMascot({ className, ...props }: BichonMascotProps) {
  const rootRef = React.useRef<HTMLDivElement>(null)
  const frame = React.useRef(0)
  const [look, setLook] = React.useState({ x: 0, y: 0 })

  React.useEffect(() => {
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      return
    }

    const onPointerMove = (event: PointerEvent) => {
      cancelAnimationFrame(frame.current)
      frame.current = requestAnimationFrame(() => {
        const rect = rootRef.current?.getBoundingClientRect()
        if (!rect) return
        const x = (event.clientX - (rect.left + rect.width / 2)) / (rect.width / 2)
        const y = (event.clientY - (rect.top + rect.height / 2)) / (rect.height / 2)
        setLook({ x: clamp(x), y: clamp(y) })
      })
    }

    window.addEventListener('pointermove', onPointerMove)
    return () => {
      window.removeEventListener('pointermove', onPointerMove)
      cancelAnimationFrame(frame.current)
    }
  }, [])

  return (
    <div
      ref={rootRef}
      aria-hidden='true'
      className={cn('pointer-events-none select-none [perspective:900px]', className)}
      {...props}
    >
      <div className='h-full w-full motion-safe:animate-[mascot-float_6s_ease-in-out_infinite]'>
        <div
          className='relative h-full w-full rounded-[14%] shadow-2xl shadow-black/20 motion-safe:transition-transform motion-safe:duration-200 motion-safe:ease-out'
          style={{
            transform: `rotateX(${-look.y * 10}deg) rotateY(${look.x * 12}deg)`,
          }}
        >
          <img
            src={Logo}
            alt=''
            draggable={false}
            className='h-full w-full rounded-[14%] object-contain'
          />
          <div
            className='absolute inset-0 rounded-[14%]'
            style={{
              background: `radial-gradient(circle at ${50 + look.x * 35}% ${
                50 + look.y * 35
              }%, rgba(255,255,255,0.2), rgba(255,255,255,0) 60%)`,
            }}
          />
        </div>
      </div>
    </div>
  )
}
