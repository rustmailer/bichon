//
// Copyright (c) 2025 rustmailer.com (https://rustmailer.com)
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

import { enUS, zhCN, zhTW, arSA, de, es, fi, fr, it, ja, ko, nl, ptBR, ru, da, sv, nb, Locale } from 'date-fns/locale';
import { type ClassValue, clsx } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}


export const formatBytes = (sizeInBytes: number): string => {
  if (sizeInBytes < 1024) {
    return `${sizeInBytes} B`;
  } else if (sizeInBytes < 1024 * 1024) {
    return `${(sizeInBytes / 1024).toFixed(2)} KB`;
  } else if (sizeInBytes < 1024 * 1024 * 1024) {
    return `${(sizeInBytes / (1024 * 1024)).toFixed(2)} MB`;
  } else {
    return `${(sizeInBytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
  }
};



export const validateFlag = (input: string): string | null => {
  // Check if the string is empty
  if (input.length === 0) {
    return `'input' cannot be empty.`;
  }

  // Check if the length is greater than 64 characters
  if (input.length > 64) {
    return `'input' cannot be longer than 64 characters.`;
  }

  // Check if the string starts with a letter and contains only letters, numbers, underscores, or dashes
  const regex = /^[a-zA-Z][a-zA-Z0-9_-]*$/;
  if (!regex.test(input)) {
    return `'input' must start with a letter and can only contain letters, numbers, underscores, or dashes.`;
  }

  // If all checks pass, return null
  return null;
};



export function mapToRecordOfArrays(
  map: Map<number, Set<number>>
): Record<string, number[]> {
  return Object.fromEntries(
    Array.from(map.entries()).map(([key, value]) => [String(key), Array.from(value)])
  );
}

export function formatNumber(num: number): string {
  const userLocale = navigator.language;

  return new Intl.NumberFormat(userLocale, {
    maximumFractionDigits: 2,
  }).format(num);
}



export function validateTag(facetPath: string) {
  if (!facetPath || facetPath.length === 0) {
    return {
      valid: false,
      error: "Tag path cannot be empty"
    };
  }

  if (!facetPath.startsWith('/')) {
    return {
      valid: false,
      error: "Tag path must start with '/'"
    };
  }

  let escaped = false;
  for (let i = 1; i < facetPath.length; i++) {
    const char = facetPath[i];

    if (escaped) {
      escaped = false;
    } else if (char === '\\') {
      escaped = true;
    }
  }

  if (escaped) {
    return {
      valid: false,
      error: "Tag path has unmatched escape character at the end"
    };
  }

  return { valid: true };
}


export function formatTimestamp(milliseconds: number): string {
  const date = new Date(milliseconds);
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  const hours = String(date.getHours()).padStart(2, '0');
  const minutes = String(date.getMinutes()).padStart(2, '0');
  const seconds = String(date.getSeconds()).padStart(2, '0');
  const timezoneOffset = date.getTimezoneOffset();
  const offsetSign = timezoneOffset > 0 ? '-' : '+';
  const offsetHours = String(Math.floor(Math.abs(timezoneOffset) / 60)).padStart(2, '0');
  const offsetMinutes = String(Math.abs(timezoneOffset) % 60).padStart(2, '0');
  return `${year}-${month}-${day}T${hours}:${minutes}:${seconds}${offsetSign}${offsetHours}:${offsetMinutes}`;
}



// i18n.language -> date-fns locale
export const dateFnsLocaleMap: Record<string, Locale> = {
  en: enUS,
  'en-us': enUS,
  zh: zhCN,
  'zh-cn': zhCN,
  'zh-tw': zhTW,
  'zh_hk': zhTW,
  ar: arSA,
  'ar-sa': arSA,
  de: de,
  'de-de': de,
  es: es,
  'es-es': es,
  fi: fi,
  'fi-fi': fi,
  fr: fr,
  'fr-fr': fr,
  it: it,
  'it-it': it,
  jp: ja,
  ja: ja,
  'ja-jp': ja,
  ko: ko,
  'ko-kr': ko,
  nl: nl,
  'nl-nl': nl,
  pt: ptBR,
  'pt-br': ptBR,
  ru: ru,
  'ru-ru': ru,
  da: da,
  'da-dk': da,
  sv: sv,
  'sv-se': sv,
  no: nb,
  'no-no': nb,
};