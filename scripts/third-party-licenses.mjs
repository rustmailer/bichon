#!/usr/bin/env node
/**
 * Generate third-party license reports for Bichon's binaries.
 *
 * Output:
 *   third-party-licenses/bichon-server.txt
 *   third-party-licenses/bichon-admin.txt
 *   third-party-licenses/bichon-cli.txt
 *
 * `bichon-server` embeds the web UI, so its report covers both the Rust crates
 * in its dependency graph and the production npm packages bundled into
 * web/dist. The admin and CLI tools are Rust-only.
 *
 * Usage:
 *   node scripts/third-party-licenses.mjs
 *
 * Requirements:
 *   - cargo-about on PATH
 *   - pnpm on PATH
 *   - `pnpm install` has been run in web/
 *
 * Canonical SPDX texts for packages that declare a license but do not ship a
 * LICENSE file live in scripts/license-texts/ (copied from the SPDX
 * license-list-data project, CC0-1.0).
 *
 * The output is deterministic: no timestamps, everything sorted. Run it
 * after dependency changes and commit the updated files.
 */

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)))
const OUTPUT_DIR = join(ROOT, 'third-party-licenses')
const CONFIG = join(ROOT, 'about.toml')
const WEB_DIR = join(ROOT, 'web')
const LICENSE_TEXT_DIR = join(ROOT, 'scripts', 'license-texts')

const TARGETS = [
  {
    name: 'bichon-server',
    manifest: 'crates/server/Cargo.toml',
    npm: true,
  },
  {
    name: 'bichon-admin',
    manifest: 'crates/admin/Cargo.toml',
    npm: false,
  },
  {
    name: 'bichon-cli',
    manifest: 'crates/cli/Cargo.toml',
    npm: false,
  },
]

const PRODUCT = 'BICHON'
const WEB_LABEL = 'web/dist'
const PLATFORMS =
  'x86_64/aarch64 Linux (glibc and musl), macOS, and Windows'

/**
 * Bichon's own crates. They are the product, not third-party dependencies.
 */
function isFirstParty(name) {
  return /^bichon(-|$)/.test(name) || name === 'machineid' || name === 'i18n-editor'
}

function sha256(text) {
  return createHash('sha256').update(text).digest('hex')
}

function uniqueBy(items, key) {
  const seen = new Set()
  return items.filter((item) => {
    const value = key(item)
    if (seen.has(value)) return false
    seen.add(value)
    return true
  })
}

function compare(a, b) {
  return a.localeCompare(b, 'en')
}

// ── Rust dependency graph ───────────────────────────────────────────────────

function readRustLicenses(target) {
  const jsonPath = join(tmpdir(), `bichon-about-${process.pid}-${target.name}.json`)

  execFileSync(
    'cargo',
    [
      'about',
      'generate',
      '--manifest-path',
      target.manifest,
      '--format',
      'json',
      '--config',
      CONFIG,
      '--output-file',
      jsonPath,
    ],
    { cwd: ROOT, stdio: ['ignore', 'ignore', 'inherit'] },
  )

  const raw = JSON.parse(readFileSync(jsonPath, 'utf8'))
  rmSync(jsonPath, { force: true })

  // cargo-about emits one entry per distinct license *text* (which is why a
  // popular license like MIT appears many times with different copyright
  // lines). Merge identical texts, then group by SPDX identifier.
  const groups = new Map()

  for (const entry of raw.licenses) {
    const crates = uniqueBy(
      entry.used_by
        .map((used) => ({
          name: used.crate.name,
          version: used.crate.version,
        }))
        .filter((crate) => !isFirstParty(crate.name)),
      (crate) => `${crate.name}@${crate.version}`,
    )
    if (crates.length === 0) continue

    const text = entry.text.replace(/\r\n/g, '\n').trimEnd()
    const hash = sha256(text)

    if (!groups.has(entry.id)) {
      groups.set(entry.id, { id: entry.id, name: entry.name, variants: new Map() })
    }
    const group = groups.get(entry.id)
    if (!group.variants.has(hash)) {
      group.variants.set(hash, { text, crates: new Map() })
    }
    const variant = group.variants.get(hash)
    for (const crate of crates) {
      variant.crates.set(`${crate.name}@${crate.version}`, crate)
    }
  }

  return [...groups.values()]
    .map((group) => {
      const allCrates = new Map()
      const variants = [...group.variants.values()]
        .map((variant) => {
          for (const [key, crate] of variant.crates) allCrates.set(key, crate)
          return variant
        })
        .sort((a, b) => sha256(a.text).localeCompare(sha256(b.text)))
      return {
        id: group.id,
        name: group.name,
        crates: [...allCrates.values()].sort(
          (a, b) => compare(a.name, b.name) || compare(a.version, b.version),
        ),
        variants,
      }
    })
    .sort((a, b) => compare(a.name, b.name) || compare(a.id, b.id))
}

// ── Web UI npm dependency graph ─────────────────────────────────────────────

const LICENSE_FILE = /^(licen[cs]e|copying|notice)([._-].*)?$/i

function readLicenseFiles(packageDir) {
  if (!existsSync(packageDir)) return []

  let entries
  try {
    entries = readdirSync(packageDir)
  } catch {
    return []
  }

  return entries
    .filter((name) => LICENSE_FILE.test(name))
    .map((name) => join(packageDir, name))
    .filter((file) => {
      try {
        return statSync(file).isFile()
      } catch {
        return false
      }
    })
    .sort()
    .map((file) => readFileSync(file, 'utf8').replace(/\r\n/g, '\n').trimEnd())
    .filter(Boolean)
}

/**
 * Some npm packages declare a license in package.json but do not ship a
 * LICENSE file (the Radix UI packages are a common example). For these we
 * fall back to the canonical SPDX text, committed under
 * scripts/license-texts/. Compound expressions such as `MIT AND ISC` include
 * the text of every referenced license.
 */
function canonicalTextFor(licenseId) {
  const ids = licenseId
    .replace(/[()]/g, ' ')
    .split(/\s+(?:AND|OR)\s+/i)
    .map((id) => id.trim())
    .filter(Boolean)

  if (ids.length === 0) return null

  const texts = []
  for (const id of ids) {
    const file = join(LICENSE_TEXT_DIR, `${id}.txt`)
    if (!existsSync(file)) return null
    texts.push(readFileSync(file, 'utf8').replace(/\r\n/g, '\n').trimEnd())
  }
  return texts.join('\n\n' + '-'.repeat(80) + '\n\n')
}

function readNpmLicenses() {
  if (!existsSync(join(WEB_DIR, 'node_modules'))) {
    throw new Error(
      `web/node_modules is missing. Run "(cd web && pnpm install)" first.`,
    )
  }

  const raw = JSON.parse(
    execFileSync('pnpm', ['licenses', 'list', '--prod', '--json'], {
      cwd: WEB_DIR,
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
    }),
  )

  const groups = []

  for (const [licenseId, entries] of Object.entries(raw)) {
    const packages = new Map()
    const variants = new Map()
    const missingText = []

    for (const entry of entries) {
      const versions = entry.versions?.length ? entry.versions : ['unknown']
      const paths = entry.paths ?? []

      versions.forEach((version, index) => {
        const label = `${entry.name}@${version}`
        packages.set(label, {
          label,
          name: entry.name,
          version,
          homepage: entry.homepage,
          license: entry.license ?? licenseId,
        })

        const packageDir = paths[index] ?? paths[0]
        if (!packageDir) {
          if (!missingText.includes(label)) missingText.push(label)
          return
        }

        const texts = readLicenseFiles(packageDir)
        if (texts.length === 0) {
          if (!missingText.includes(label)) missingText.push(label)
          return
        }

        for (const text of texts) {
          const hash = sha256(text)
          if (!variants.has(hash)) variants.set(hash, { text, packages: new Map() })
          variants.get(hash).packages.set(label, label)
        }
      })
    }

    if (packages.size === 0) continue

    // Packages that declared a license but did not ship a file: attach the
    // canonical SPDX text when we have one locally.
    if (missingText.length > 0) {
      const canonical = canonicalTextFor(licenseId)
      if (canonical) {
        const hash = sha256(canonical)
        if (!variants.has(hash)) {
          variants.set(hash, {
            text: canonical,
            packages: new Map(),
            canonical: true,
          })
        }
        for (const label of missingText) {
          variants.get(hash).packages.set(label, label)
        }
        missingText.length = 0
      }
    }

    groups.push({
      id: licenseId,
      packages: [...packages.values()].sort(
        (a, b) => compare(a.name, b.name) || compare(a.version, b.version),
      ),
      variants: [...variants.values()].sort((a, b) =>
        sha256(a.text).localeCompare(sha256(b.text)),
      ),
      missingText: missingText.sort(compare),
    })
  }

  return groups.sort((a, b) => compare(a.id, b.id))
}

// ── Rendering ─────────────────────────────────────────────────────────────────

function renderRust(rustGroups) {
  const lines = []
  const crateCount = new Set()
  const licenseIds = new Set()
  let variantCount = 0

  for (const group of rustGroups) {
    licenseIds.add(group.id)
    variantCount += group.variants.length
    for (const crate of group.crates) crateCount.add(`${crate.name}@${crate.version}`)

    lines.push('')
    lines.push('='.repeat(80))
    lines.push(`${group.name} (${group.id})`)
    lines.push('='.repeat(80))
    lines.push('')
    lines.push(`Used by ${group.crates.length} crate(s):`)
    for (const crate of group.crates) lines.push(`  - ${crate.name} ${crate.version}`)

    if (group.variants.length === 1) {
      lines.push('')
      lines.push('License text:')
      lines.push('')
      lines.push(group.variants[0].text)
    } else {
      group.variants.forEach((variant, index) => {
        lines.push('')
        lines.push('-'.repeat(80))
        lines.push(
          `License text variant ${index + 1} of ${group.variants.length} ` +
            `(used by ${variant.crates.size} crate(s)):`,
        )
        for (const crate of [...variant.crates.values()].sort(
          (a, b) => compare(a.name, b.name) || compare(a.version, b.version),
        )) {
          lines.push(`  - ${crate.name} ${crate.version}`)
        }
        lines.push('')
        lines.push(variant.text)
      })
    }
    lines.push('')
  }

  return {
    text: lines.join('\n'),
    summary: {
      crateCount: crateCount.size,
      licenseIds: licenseIds.size,
      variantCount,
    },
  }
}

function renderNpm(npmGroups) {
  const lines = []
  const packageCount = new Set()
  const licenseIds = new Set()
  let variantCount = 0
  let missingCount = 0

  for (const group of npmGroups) {
    licenseIds.add(group.id)
    variantCount += group.variants.length
    missingCount += group.missingText.length
    for (const pkg of group.packages) packageCount.add(pkg.label)

    lines.push('')
    lines.push('='.repeat(80))
    lines.push(`${group.id}`)
    lines.push('='.repeat(80))
    lines.push('')
    lines.push(`Used by ${group.packages.length} package(s):`)
    for (const pkg of group.packages) lines.push(`  - ${pkg.label}`)

    if (group.variants.length === 1) {
      lines.push('')
      lines.push(
        group.variants[0].canonical
          ? 'License text (canonical SPDX text; the package does not ship a license file):'
          : 'License text:',
      )
      lines.push('')
      lines.push(group.variants[0].text)
    } else if (group.variants.length > 1) {
      group.variants.forEach((variant, index) => {
        lines.push('')
        lines.push('-'.repeat(80))
        lines.push(
          `License text variant ${index + 1} of ${group.variants.length} ` +
            `(used by ${variant.packages.size} package(s)` +
            `${variant.canonical ? '; canonical SPDX text, no license file in the package' : ''}):`,
        )
        for (const pkg of [...variant.packages.keys()].sort(compare)) {
          lines.push(`  - ${pkg}`)
        }
        lines.push('')
        lines.push(variant.text)
      })
    }

    if (group.missingText.length > 0) {
      lines.push('')
      lines.push(`No license file found in the package for:`)
      for (const pkg of group.missingText) lines.push(`  - ${pkg}`)
    }
    lines.push('')
  }

  return {
    text: lines.join('\n'),
    summary: {
      packageCount: packageCount.size,
      licenseIds: licenseIds.size,
      variantCount,
      missingCount,
    },
  }
}

function renderReport(target, rust, npm) {
  const header = [
    '='.repeat(80),
    `${PRODUCT} - THIRD-PARTY LICENSES`,
    `Binary: ${target.name}`,
    '='.repeat(80),
    '',
    'This file lists the third-party components distributed with this binary,',
    'together with the full text of every license they use.',
    '',
    'Scope:',
    `  * Rust crates in the dependency graph of \`${target.name}\`.`,
  ]

  if (target.npm) {
    header.push(
      `  * Production npm packages bundled into the web UI (${WEB_LABEL})`,
      '    and embedded in this binary.',
    )
  }

  header.push(
    `  * Targets: ${PLATFORMS}.`,
    '',
    `Bichon's own crates are excluded.`,
    '',
    'Generated by scripts/third-party-licenses.mjs. Do not edit by hand;',
    'regenerate the file after dependency changes.',
    '',
    'Summary:',
    `  Rust:   ${rust.summary.crateCount} third-party crates, ` +
      `${rust.summary.licenseIds} license identifiers, ` +
      `${rust.summary.variantCount} license-text variants`,
  )

  if (target.npm) {
    header.push(
      `  Web UI: ${npm.summary.packageCount} third-party packages, ` +
        `${npm.summary.licenseIds} license identifiers, ` +
        `${npm.summary.variantCount} license-text variants`,
    )
  }

  header.push(
    '',
    'Contents:',
    '  1. Rust / Cargo dependencies',
  )
  if (target.npm) header.push('  2. Web UI / npm dependencies')

  const rustHeader = [
    '',
    '',
    '='.repeat(80),
    '1. RUST / CARGO DEPENDENCIES',
    '='.repeat(80),
    '',
  ].join('\n')

  const npmHeader = [
    '',
    '',
    '='.repeat(80),
    '2. WEB UI / NPM DEPENDENCIES',
    '='.repeat(80),
    '',
  ].join('\n')

  const footer = ['', '', '='.repeat(80), 'END OF THIRD-PARTY LICENSES', '='.repeat(80), ''].join(
    '\n',
  )

  let body = rust.text
  if (target.npm) body += npmHeader + npm.text
  return header.join('\n') + rustHeader + body + footer
}

function main() {
  mkdirSync(OUTPUT_DIR, { recursive: true })

  for (const target of TARGETS) {
    console.log(`Collecting licenses for ${target.name}...`)

    const rust = renderRust(readRustLicenses(target))
    const npm = target.npm ? renderNpm(readNpmLicenses()) : null

    const output = join(OUTPUT_DIR, `${target.name}.txt`)
    writeFileSync(output, renderReport(target, rust, npm))
    console.log(`  wrote ${output}`)

    if (target.npm && npm.summary.missingCount > 0) {
      console.log(
        `  Warning: ${npm.summary.missingCount} npm package(s) had no license file in node_modules.`,
      )
    }
  }
}

try {
  main()
} catch (error) {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
}
