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
 * One-time-code input: six display-only boxes and one hidden text field for
 * the keyboard (a mobile soft keyboard needs a real input).
 *
 * The field is deliberately disguised so password managers do not identify it
 * as a verification-code box on page load:
 *
 * - no `inputMode`, `pattern`, `maxLength` or "code" aria-label — those are
 *   exactly what managers use to recognise an OTP field;
 * - `readOnly` on mount, unlocked by a real pointer/key/paste interaction (the
 *   login form autofocuses the field, so unlocking on `focus` alone would
 *   reopen the hole);
 * - `autocomplete="off"`, a random `name` per mount, and the manager opt-out
 *   attributes;
 * - the MFA step is rendered outside any `<form>` (see user-auth-form), so a
 *   manager cannot read it as the tail of the sign-in form.
 *
 * The visible boxes are plain elements fed by state, so even if a manager
 * writes the field, nothing shows unless a real interaction is in progress.
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
    const inputRef = React.useRef<HTMLInputElement | null>(null)

    // A random field name per mount: managers key on names like "code"/"otp".
    const fieldName = React.useMemo(
      () => `otp-${Math.random().toString(36).slice(2)}`,
      []
    )

    const split = React.useCallback(
      (raw: string) => {
        const digits = (raw ?? '').replace(/\D/g, '').slice(0, length)
        return Array.from({ length }, (_, i) => digits[i] ?? '')
      },
      [length]
    )

    const [digits, setDigits] = React.useState<string[]>(() => split(value))
    const [focused, setFocused] = React.useState(false)

    // Locked on mount. A manager that fills on load targets an editable
    // field; this one is read-only until the user actually touches it.
    const [locked, setLocked] = React.useState(true)
    const lockedRef = React.useRef(true)
    const unlock = React.useCallback(() => {
      if (!lockedRef.current) return
      lockedRef.current = false
      setLocked(false)
    }, [])

    React.useEffect(() => {
      const incoming = split(value)
      setDigits((prev) =>
        prev.join('') === incoming.join('') ? prev : incoming
      )
    }, [value, split])

    React.useEffect(() => {
      if (autoFocus) inputRef.current?.focus()
    }, [autoFocus])

    // Keep the caret at the end: the field is one continuous code.
    React.useEffect(() => {
      if (!focused) return
      const input = inputRef.current
      if (!input) return
      const end = input.value.length
      input.setSelectionRange(end, end)
    }, [digits, focused])

    const code = digits.join('')

    const applyCode = (raw: string) => {
      const next = raw.replace(/\D/g, '').slice(0, length)
      setDigits(split(next))
      onChange(next)
    }

    const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
      // A change before the user touched the field is a fill, not typing.
      if (lockedRef.current) {
        e.target.value = code
        return
      }
      const next = e.target.value.replace(/\D/g, '').slice(0, length)
      if (e.target.value !== next) e.target.value = next
      applyCode(next)
    }

    const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
      if (!lockedRef.current) return
      unlock()
      // The read-only attribute swallowed this keystroke; commit it so the
      // first digit typed into an autofocused field is not lost.
      if (/^[0-9]$/.test(e.key)) {
        e.preventDefault()
        applyCode(code + e.key)
      }
    }

    const handlePaste = (e: React.ClipboardEvent<HTMLInputElement>) => {
      const pasted = e.clipboardData.getData('text').replace(/\D/g, '')
      if (!pasted) return
      e.preventDefault()
      unlock()
      applyCode(pasted)
    }

    // The box the user is "in": the next empty one, clamped to the last.
    const caretIndex = Math.min(code.length, length - 1)

    const setRefs = (el: HTMLInputElement | null) => {
      inputRef.current = el
      if (typeof ref === 'function') ref(el)
      else if (ref) ref.current = el
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
        className={cn('relative', disabled && 'opacity-50')}
      >
        <div
          className={cn('flex items-center gap-2', className)}
          aria-hidden='true'
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
                  const current = focused && index === caretIndex
                  return (
                    <div
                      key={index}
                      data-otp-digit=''
                      className={cn(
                        'relative flex h-10 w-10 items-center justify-center border-y border-r border-input text-base transition-all first:rounded-l-md first:border-l last:rounded-r-md md:text-lg',
                        current && 'z-10 border-ring ring-1 ring-ring'
                      )}
                    >
                      {digit}
                    </div>
                  )
                })}
              </div>
            </React.Fragment>
          ))}
        </div>

        {/* The only editable element. No inputMode/pattern/maxLength: the
            mode and length are enforced here, not advertised to managers. */}
        <input
          ref={setRefs}
          id={id}
          name={fieldName}
          type='text'
          autoComplete='off'
          value={code}
          disabled={disabled}
          readOnly={locked}
          aria-label={ariaLabel ?? 'Verification digits'}
          aria-describedby={ariaDescribedBy}
          spellCheck={false}
          autoCapitalize='off'
          autoCorrect='off'
          data-lpignore='true'
          data-1p-ignore='true'
          data-bwignore='true'
          data-form-type='other'
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          onPointerDown={unlock}
          onTouchStart={unlock}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          className='absolute inset-0 h-full w-full cursor-text bg-transparent text-transparent caret-transparent opacity-0 outline-none disabled:cursor-not-allowed'
        />
      </div>
    )
  }
)
OtpInput.displayName = 'OtpInput'

export { OtpInput }
