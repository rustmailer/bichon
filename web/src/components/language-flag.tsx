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

// Flags are bundled locally as SVG data URLs: emoji flags (regional
// indicators) do not render on Windows, and a remote flag CDN would break
// air-gapped installs. The mapping is language -> the country users
// associate with it, e.g. British English, Brazilian Portuguese.

import ArFlag from 'country-flag-icons/string/3x2/SA'
import CsFlag from 'country-flag-icons/string/3x2/CZ'
import DaFlag from 'country-flag-icons/string/3x2/DK'
import DeFlag from 'country-flag-icons/string/3x2/DE'
import EnFlag from 'country-flag-icons/string/3x2/GB'
import EsFlag from 'country-flag-icons/string/3x2/ES'
import FiFlag from 'country-flag-icons/string/3x2/FI'
import FrFlag from 'country-flag-icons/string/3x2/FR'
import ItFlag from 'country-flag-icons/string/3x2/IT'
import JpFlag from 'country-flag-icons/string/3x2/JP'
import KoFlag from 'country-flag-icons/string/3x2/KR'
import NlFlag from 'country-flag-icons/string/3x2/NL'
import NoFlag from 'country-flag-icons/string/3x2/NO'
import PlFlag from 'country-flag-icons/string/3x2/PL'
import PtFlag from 'country-flag-icons/string/3x2/BR'
import RuFlag from 'country-flag-icons/string/3x2/RU'
import SvFlag from 'country-flag-icons/string/3x2/SE'
import ZhFlag from 'country-flag-icons/string/3x2/CN'
import ZhTwFlag from 'country-flag-icons/string/3x2/TW'

import { cn } from '@/lib/utils'

const FLAG_SVGS: Record<string, string> = {
  ar: ArFlag,
  cs: CsFlag,
  da: DaFlag,
  de: DeFlag,
  en: EnFlag,
  es: EsFlag,
  fi: FiFlag,
  fr: FrFlag,
  it: ItFlag,
  jp: JpFlag,
  ko: KoFlag,
  nl: NlFlag,
  no: NoFlag,
  pl: PlFlag,
  pt: PtFlag,
  ru: RuFlag,
  sv: SvFlag,
  zh: ZhFlag,
  'zh-tw': ZhTwFlag,
}

const FLAG_URLS: Record<string, string> = Object.fromEntries(
  Object.entries(FLAG_SVGS).map(([code, svg]) => [
    code,
    `data:image/svg+xml,${encodeURIComponent(svg)}`,
  ])
)

interface LanguageFlagProps {
  code: string
  className?: string
}

/** Decorative 3:2 flag for a language code; renders nothing if unknown. */
export function LanguageFlag({ code, className }: LanguageFlagProps) {
  const src = FLAG_URLS[code]
  if (!src) return null
  return (
    <img
      src={src}
      alt=''
      aria-hidden
      draggable={false}
      className={cn(
        'h-3.5 w-[21px] shrink-0 rounded-[2px] object-cover ring-1 ring-inset ring-black/10 dark:ring-white/15',
        className
      )}
    />
  )
}
