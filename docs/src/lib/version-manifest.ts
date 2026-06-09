import "server-only"
import * as fs from "fs"
import * as path from "path"
import { VERSION_SLUGS } from "./versions"

const CONTENT_ROOT = path.join(process.cwd(), "src/content")

export type VersionManifest = Record<string, string[]>

let cache: VersionManifest | null = null

function walk(dir: string, base = ""): string[] {
  const out: string[] = []
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const rel = base ? `${base}/${entry.name}` : entry.name
    if (entry.isDirectory()) {
      out.push(...walk(path.join(dir, entry.name), rel))
    } else if (entry.isFile() && /\.(mdx|md)$/i.test(entry.name)) {
      out.push(rel.replace(/\.(mdx|md)$/i, ""))
    }
  }
  return out
}

export async function getVersionManifest(): Promise<VersionManifest> {
  if (cache) return cache
  const result: VersionManifest = {}
  for (const slug of VERSION_SLUGS) {
    const dir = path.join(CONTENT_ROOT, slug)
    const routes = new Set<string>()
    for (const file of walk(dir)) {
      if (file === "index") routes.add("")
      else if (file.endsWith("/index")) routes.add(file.slice(0, -"/index".length))
      else routes.add(file)
    }
    result[slug] = [...routes].sort()
  }
  cache = result
  return result
}
