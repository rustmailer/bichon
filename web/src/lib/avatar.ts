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

// Avatars are stored as base64 text in the in-memory database, so the
// uploader downscales every picture before it is sent. The largest edge
// below keeps a retina-crisp 96px avatar (the largest place it is shown)
// at roughly 10-25 KB, well under the server's 128 KB guard.

const AVATAR_MAX_EDGE = 256
const AVATAR_QUALITY = 0.9

/** Largest original file the picker accepts; it is resized before upload. */
export const AVATAR_PICKER_MAX_BYTES = 10 * 1024 * 1024

/** Raster formats the browser can draw before re-encoding. */
export const AVATAR_ACCEPT = 'image/png,image/jpeg,image/webp,image/gif'

const AVATAR_SOURCE_TYPES = AVATAR_ACCEPT.split(',')

const AVATAR_MIME_PREFIXES: Array<[string, string]> = [
  ['iVBORw0KGgo', 'image/png'],
  ['/9j/', 'image/jpeg'],
  ['R0lGOD', 'image/gif'],
  ['UklGR', 'image/webp'],
]

export function isSupportedAvatarType(type: string): boolean {
  return AVATAR_SOURCE_TYPES.includes(type)
}

/** Build a data URL with the format the stored bytes actually have. */
export function avatarDataUrl(base64?: string | null): string | undefined {
  if (!base64) return undefined
  // Legacy rows are not guaranteed to be PNG; browsers sniff the real
  // format from the bytes anyway, so the fallback only affects the label.
  const mime =
    AVATAR_MIME_PREFIXES.find(([prefix]) => base64.startsWith(prefix))?.[1] ??
    'image/png'
  return `data:${mime};base64,${base64}`
}

function loadImage(url: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () =>
      reject(new Error('The selected file is not a readable image.'))
    image.src = url
  })
}

function canvasToBlob(
  canvas: HTMLCanvasElement,
  type: string,
  quality: number
): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, type, quality))
}

/**
 * Downscale an image to at most 256px on its longest edge and re-encode it
 * as WebP (JPEG with a white backdrop on browsers without a WebP encoder).
 * The result is usually one to two orders of magnitude smaller than the
 * original, which is what keeps the in-memory user record small.
 */
export async function compressAvatar(file: File): Promise<File> {
  if (!isSupportedAvatarType(file.type)) {
    throw new Error('Avatar must be a PNG, JPEG, WebP or GIF image.')
  }

  const objectUrl = URL.createObjectURL(file)
  try {
    const image = await loadImage(objectUrl)
    if (!image.naturalWidth || !image.naturalHeight) {
      throw new Error('The selected file is not a readable image.')
    }

    const scale = Math.min(
      1,
      AVATAR_MAX_EDGE / Math.max(image.naturalWidth, image.naturalHeight)
    )
    const width = Math.max(1, Math.round(image.naturalWidth * scale))
    const height = Math.max(1, Math.round(image.naturalHeight * scale))

    const canvas = document.createElement('canvas')
    canvas.width = width
    canvas.height = height
    const context = canvas.getContext('2d')
    if (!context) {
      throw new Error('Image processing is not available in this browser.')
    }
    context.drawImage(image, 0, 0, width, height)

    let blob = await canvasToBlob(canvas, 'image/webp', AVATAR_QUALITY)
    if (blob?.type !== 'image/webp') {
      // No WebP encoder: keep transparency from turning black by laying a
      // white backdrop before falling back to JPEG.
      context.globalCompositeOperation = 'destination-over'
      context.fillStyle = '#ffffff'
      context.fillRect(0, 0, width, height)
      blob = await canvasToBlob(canvas, 'image/jpeg', AVATAR_QUALITY)
    }
    if (!blob) {
      throw new Error('Failed to process the selected image.')
    }

    const extension =
      blob.type === 'image/jpeg' ? 'jpg' : blob.type === 'image/webp' ? 'webp' : 'png'
    const stem = file.name.replace(/\.[^.]+$/, '') || 'avatar'
    return new File([blob], `${stem}.${extension}`, { type: blob.type })
  } finally {
    URL.revokeObjectURL(objectUrl)
  }
}
