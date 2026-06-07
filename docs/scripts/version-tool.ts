#!/usr/bin/env bun
/**
 * Maintenance tool for versioned documentation content under `src/content/`.
 *
 * Usage:
 *
 *   bun run scripts/version-tool.ts prefix <version>
 *     Rewrite every absolute internal MDX link inside
 *     `src/content/<version>/` to be prefixed with `/<version>` (unless it
 *     already targets a known version). Idempotent.
 *
 *   bun run scripts/version-tool.ts rename <oldVersion> <newVersion>
 *     Rename `src/content/<oldVersion>/` to `src/content/<newVersion>/` and
 *     rewrite all `/<oldVersion>/...` links inside it to `/<newVersion>/...`.
 *     Used when promoting `next` to a real release slug (e.g., `v1.6`).
 *
 *   bun run scripts/version-tool.ts clone <source> <target>
 *     Copy `src/content/<source>/` to `src/content/<target>/` and rewrite
 *     all `/<source>/...` links inside it to `/<target>/...`.
 *     Used when re-seeding `next` from a freshly-cut release.
 *
 *   bun run scripts/version-tool.ts check [<version>]
 *     Dry-run: report links that `prefix` would rewrite (i.e., absolute
 *     internal links that are not version-prefixed). Exits non-zero if any.
 *     If `<version>` is omitted, checks every version directory.
 *
 * Notes:
 *   - Code fences (``` and ~~~) are skipped to avoid touching example links.
 *   - Known version slugs are read from `src/lib/versions.ts` so links that
 *     already point at another version (e.g., `/next/foo` while processing
 *     `v1.5`) are left alone.
 *   - The tool only rewrites two link shapes used in the docs today:
 *       Markdown:  `](/path)` and `](/path "title")`
 *       JSX/HTML:  `href="/path"`, `href='/path'`, `to="/path"`, `to='/path'`
 *     `href={...}` expressions are not touched (none exist today).
 */

import { glob } from "glob"
import { readFile, writeFile, rename, cp, stat, readdir } from "fs/promises"
import { existsSync } from "fs"
import path from "path"
import { VERSION_SLUGS } from "../src/lib/versions"

const CONTENT_ROOT = "src/content"
const PUBLIC_ROOT = "public"

let publicTopLevelCache: Set<string> | null = null
async function publicTopLevelEntries(): Promise<Set<string>> {
  if (publicTopLevelCache) return publicTopLevelCache
  publicTopLevelCache = new Set(
    existsSync(PUBLIC_ROOT) ? await readdir(PUBLIC_ROOT) : []
  )
  return publicTopLevelCache
}

type Rewrite = (link: string) => string | null

async function listMdx(dir: string): Promise<string[]> {
  return glob(`${dir}/**/*.{mdx,md}`)
}

/**
 * Apply `rewrite` to all eligible link strings in `content`, leaving fenced
 * code blocks untouched.
 */
function rewriteLinks(content: string, rewrite: Rewrite): string {
  // Split on fenced code blocks (``` ... ``` or ~~~ ... ~~~) at line start so
  // we can transform the prose segments only. Even-indexed parts are prose,
  // odd-indexed parts are code fences and are passed through verbatim.
  const fenceRegex = /(^[ \t]*(?:```|~~~)[\s\S]*?^[ \t]*(?:```|~~~)$)/gm
  const parts = content.split(fenceRegex)
  return parts
    .map((part, i) => (i % 2 === 0 ? rewriteSegment(part, rewrite) : part))
    .join("")
}

function rewriteSegment(segment: string, rewrite: Rewrite): string {
  // Markdown link: ](/path) optionally followed by a title
  let out = segment.replace(
    /\]\((\/[^)\s]*)(\s+"[^"]*")?\)/g,
    (match, link: string, title = "") => {
      const r = rewrite(link)
      return r === null ? match : `](${r}${title})`
    }
  )
  // JSX/HTML attributes: href="/...", href='/...', to="/...", to='/...'
  out = out.replace(
    /\b(href|to)=("|')(\/[^"']*)\2/g,
    (match, attr: string, quote: string, link: string) => {
      const r = rewrite(link)
      return r === null ? match : `${attr}=${quote}${r}${quote}`
    }
  )
  return out
}

function isAbsoluteInternal(link: string): boolean {
  return link.startsWith("/") && !link.startsWith("//")
}

function firstSegment(link: string): string {
  return link.split("/")[1] ?? ""
}

function startsWithVersion(link: string, version: string): boolean {
  return link === `/${version}` || link.startsWith(`/${version}/`)
}

function replaceVersionPrefix(link: string, from: string, to: string): string {
  if (link === `/${from}`) return `/${to}`
  if (link.startsWith(`/${from}/`)) return `/${to}/${link.slice(from.length + 2)}`
  return link
}

async function processFiles(files: string[], rewrite: Rewrite): Promise<number> {
  let changed = 0
  for (const file of files) {
    const before = await readFile(file, "utf8")
    const after = rewriteLinks(before, rewrite)
    if (after !== before) {
      await writeFile(file, after)
      changed++
    }
  }
  return changed
}

async function prefixCommand(version: string) {
  const dir = path.join(CONTENT_ROOT, version)
  if (!existsSync(dir)) throw new Error(`Directory ${dir} does not exist`)
  const files = await listMdx(dir)
  const publicEntries = await publicTopLevelEntries()

  const rewrite: Rewrite = link => {
    if (!isAbsoluteInternal(link)) return null
    const first = firstSegment(link)
    if (VERSION_SLUGS.includes(first)) return null
    if (publicEntries.has(first)) return null // e.g. /images/foo.png, /favicon.ico
    return `/${version}${link}`
  }

  const changed = await processFiles(files, rewrite)
  console.log(
    `prefix ${version}: rewrote links in ${changed}/${files.length} file(s) under ${dir}`
  )
}

async function renameCommand(oldVersion: string, newVersion: string) {
  const oldDir = path.join(CONTENT_ROOT, oldVersion)
  const newDir = path.join(CONTENT_ROOT, newVersion)
  if (!existsSync(oldDir)) throw new Error(`Source ${oldDir} does not exist`)
  if (existsSync(newDir)) throw new Error(`Target ${newDir} already exists`)

  await rename(oldDir, newDir)

  const files = await listMdx(newDir)
  const rewrite: Rewrite = link => {
    if (!startsWithVersion(link, oldVersion)) return null
    return replaceVersionPrefix(link, oldVersion, newVersion)
  }
  const changed = await processFiles(files, rewrite)
  console.log(
    `rename ${oldVersion} -> ${newVersion}: moved ${oldDir} to ${newDir}; ` +
      `rewrote links in ${changed}/${files.length} file(s)`
  )
  console.log(
    "Don't forget to update src/lib/versions.ts (slugs, default version) and " +
      "regenerate any drift-checked content if applicable."
  )
}

async function cloneCommand(source: string, target: string) {
  const srcDir = path.join(CONTENT_ROOT, source)
  const dstDir = path.join(CONTENT_ROOT, target)
  if (!existsSync(srcDir)) throw new Error(`Source ${srcDir} does not exist`)
  if (existsSync(dstDir)) throw new Error(`Target ${dstDir} already exists`)
  const st = await stat(srcDir)
  if (!st.isDirectory()) throw new Error(`Source ${srcDir} is not a directory`)

  await cp(srcDir, dstDir, { recursive: true })

  const files = await listMdx(dstDir)
  const rewrite: Rewrite = link => {
    if (!startsWithVersion(link, source)) return null
    return replaceVersionPrefix(link, source, target)
  }
  const changed = await processFiles(files, rewrite)
  console.log(
    `clone ${source} -> ${target}: copied ${srcDir} to ${dstDir}; ` +
      `rewrote links in ${changed}/${files.length} file(s)`
  )
}

async function checkCommand(version?: string) {
  const versionsToCheck = version ? [version] : [...VERSION_SLUGS]
  const publicEntries = await publicTopLevelEntries()
  let totalOffenders = 0

  for (const v of versionsToCheck) {
    const dir = path.join(CONTENT_ROOT, v)
    if (!existsSync(dir)) {
      console.warn(`skip: ${dir} does not exist`)
      continue
    }
    const files = await listMdx(dir)
    let offendersInVersion = 0

    for (const file of files) {
      const content = await readFile(file, "utf8")
      const offenders: string[] = []
      rewriteLinks(content, link => {
        if (!isAbsoluteInternal(link)) return null
        const first = firstSegment(link)
        if (VERSION_SLUGS.includes(first)) return null
        if (publicEntries.has(first)) return null
        offenders.push(link)
        return null
      })
      if (offenders.length > 0) {
        offendersInVersion += offenders.length
        const rel = path.relative(process.cwd(), file)
        console.log(`- ${rel}:`)
        for (const link of offenders) console.log(`    ${link}`)
      }
    }

    if (offendersInVersion === 0) {
      console.log(`check ${v}: OK`)
    } else {
      console.log(`check ${v}: ${offendersInVersion} unversioned link(s)`)
    }
    totalOffenders += offendersInVersion
  }

  if (totalOffenders > 0) process.exit(1)
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2)
  switch (cmd) {
    case "prefix": {
      const [version] = rest
      if (!version) throw new Error("usage: version-tool.ts prefix <version>")
      await prefixCommand(version)
      break
    }
    case "rename": {
      const [oldVersion, newVersion] = rest
      if (!oldVersion || !newVersion)
        throw new Error("usage: version-tool.ts rename <oldVersion> <newVersion>")
      await renameCommand(oldVersion, newVersion)
      break
    }
    case "clone": {
      const [source, target] = rest
      if (!source || !target)
        throw new Error("usage: version-tool.ts clone <source> <target>")
      await cloneCommand(source, target)
      break
    }
    case "check": {
      const [version] = rest
      await checkCommand(version)
      break
    }
    default:
      console.error("Unknown command. See top of file for usage.")
      process.exit(2)
  }
}

main().catch(err => {
  console.error(err)
  process.exit(1)
})
