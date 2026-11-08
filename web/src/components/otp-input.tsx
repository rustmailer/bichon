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
import { Minus } from 'lucide-react'
import { cn } from '@/lib/utils'

type OtpInputProps = Omit<
  React.HTMLAttributes<HTMLDivElement>,
  'onChange' | 'defaultValue'
> & {
  value: string
  onChange: (value: string) => void
  length?: number
  disabled?: boolean
  autoFocus?: boolean
}

/**
 * A one-time-code input rendered as one box per digit.
 *
 * The single-input version was attacked by password managers even with
 * `autocomplete="one-time-code"`: a lone text field in a login form still looks
 * like a credential box to them. One-character boxes are not a credential
 * shape, so a manager can at most touch one of them, and a value that is not
 * exactly `length` digits is discarded as noise instead of being spread across
 * the boxes.
 */
const OtpInput = React.forwardRef<HTMLInputElement, OtpInputProps>(
  (
    {
      value,
      onChange,
      length = 6,
      disabled,
      autoFocus,
      className,
      id,
      'aria-label': ariaLabel,
      'aria-describedby': ariaDescribedBy,
      ...props
    },
    ref
  ) => {
    const inputs = React.useRef<(HTMLInputElement | null)[]>([])

    const split = React.useCallback(
      (raw: string) => {
        const digits = (raw ?? '').replace(/\D/g, '')
        return Array.from({ length }, (_, i) => digits[i] ?? '')
      },
      [length]
    )

    // The boxes keep their positions when the user leaves a gap (typing into
    // box 3 first must not shift that digit to box 1), so the displayed digits
    // are local state. The parent still receives the compact code.
    const [digits, setDigits] = React.useState<string[]>(() => split(value))

    React.useEffect(() => {
      const incoming = split(value)
      setDigits((prev) =>
        prev.join('') === incoming.join('') ? prev : incoming
      )
    }, [value, split])

    const focusAt = (index: number) => {
      const target = inputs.current[Math.min(Math.max(index, 0), length - 1)]
      target?.focus()
      target?.select()
    }

    const commit = (next: string[]) => {
      setDigits(next)
      onChange(next.join(''))
    }

    React.useEffect(() => {
      if (autoFocus) inputs.current[0]?.focus()
    }, [autoFocus])

    const handleChange = (index: number, raw: string) => {
      const cleaned = raw.replace(/\D/g, '')
      if (raw.length > 1) {
        // A paste or a password manager. Only a complete code is meaningful;
        // anything else is noise, so the boxes are left untouched.
        if (cleaned.length === length) {
          commit(split(cleaned))
          focusAt(length - 1)
        }
        return
      }
      if (!cleaned) {
        const next = [...digits]
        next[index] = ''
        commit(next)
        return
      }
      const next = [...digits]
      next[index] = cleaned
      commit(next)
      if (index < length - 1) focusAt(index + 1)
    }

    const handleKeyDown = (
      index: number,
      e: React.KeyboardEvent<HTMLInputElement>
    ) => {
      if (e.key === 'Backspace') {
        e.preventDefault()
        const next = [...digits]
        if (next[index]) {
          next[index] = ''
          commit(next)
        } else if (index > 0) {
          next[index - 1] = ''
          commit(next)
          focusAt(index - 1)
        }
      } else if (e.key === 'Delete') {
        e.preventDefault()
        const next = [...digits]
        next[index] = ''
        commit(next)
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault()
        focusAt(index - 1)
      } else if (e.key === 'ArrowRight') {
        e.preventDefault()
        focusAt(index + 1)
      }
    }

    const handlePaste = (
      index: number,
      e: React.ClipboardEvent<HTMLInputElement>
    ) => {
      const pasted = e.clipboardData.getData('text').replace(/\D/g, '')
      if (!pasted) return
      e.preventDefault()
      const next = [...digits]
      const chars = pasted.slice(0, length - index).split('')
      chars.forEach((digit, offset) => {
        next[index + offset] = digit
      })
      commit(next)
      focusAt(index + chars.length - 1)
    }

    // Two groups split down the middle (six digits → 3 + 3), with a dash in
    // between — the shadcn Input OTP "separator" layout.
    const half = Math.ceil(length / 2)
    const groups = [digits.slice(0, half), digits.slice(half)].filter(
      (group) => group.length > 0
    )

    return (
      <div
        {...props}
        role='group'
        aria-label={ariaLabel}
        dir='ltr'
        className={cn('flex items-center gap-2', className)}
      >
        {groups.map((group, groupIndex) => (
          <React.Fragment key={groupIndex}>
            {groupIndex > 0 && (
              <div role='separator' className='px-0.5 text-muted-foreground'>
                <Minus size={14} />
              </div>
            )}
            <div className='flex items-center'>
              {group.map((digit, offset) => {
                const index = groupIndex * half + offset
                return (
                  <input
                    key={index}
                    id={index === 0 ? id : undefined}
                    ref={(el) => {
                      inputs.current[index] = el
                      if (index === 0) {
                        if (typeof ref === 'function') ref(el)
                        else if (ref) ref.current = el
                      }
                    }}
                    type='text'
                    inputMode='numeric'
                    pattern='[0-9]*'
                    autoComplete={index === 0 ? 'one-time-code' : 'off'}
                    aria-label={`${ariaLabel ?? 'One-time code'}, digit ${index + 1}`}
                    aria-describedby={ariaDescribedBy}
                    maxLength={1}
                    value={digit}
                    disabled={disabled}
                    spellCheck={false}
                    autoCapitalize='off'
                    autoCorrect='off'
                    onChange={(e) => handleChange(index, e.target.value)}
                    onKeyDown={(e) => handleKeyDown(index, e)}
                    onPaste={(e) => handlePaste(index, e)}
                    onFocus={(e) => e.currentTarget.select()}
                    data-lpignore='true'
                    data-1p-ignore='true'
                    data-bwignore='true'
                    data-form-type='other'
                    className='relative h-10 w-10 border-y border-r border-input bg-transparent text-center text-base transition-all first:rounded-l-md first:border-l last:rounded-r-md focus-visible:z-10 focus-visible:border-ring focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50 md:text-lg'
                  />
                )
              })}
            </div>
          </React.Fragment>
        ))}
      </div>
    )
  }
)
OtpInput.displayName = 'OtpInput'

export { OtpInput }
