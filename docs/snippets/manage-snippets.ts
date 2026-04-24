import { readFile, writeFile, rename, readdir, stat } from "fs/promises"
import { join, relative } from "path"
import { existsSync } from "fs"
import { unified } from "unified"
import remarkParse from "remark-parse"
import remarkMdx from "remark-mdx"
import { visit } from "unist-util-visit"
import { toString } from "mdast-util-to-string"
import crypto from "crypto"

// --- Types ---

type CodeBlock = {
  fenceLang: string | null
  meta: string | null
  code: string
  lineStart: number
  lineEnd: number
}

type TabContent = {
  label: string
  markdown: string
  codeBlocks: CodeBlock[]
}

type SnippetRecord = {
  id: string
  filePath: string
  lineStart: number
  lineEnd: number
  headingPath: { depth: number; text: string }[]
  sectionHeading: string
  languagesPresent: string[]
  tabs: Record<string, TabContent>
  rawTabsBlock: string
  sourceHash: string
  status: "pending" | "completed" | "skipped"
  completedAt?: string
  note?: string
  removed?: boolean
  warnings?: string[]
}

type ProgressFile = {
  version: number
  generatedAt: string
  counts: {
    total: number
    pending: number
    completed: number
    skipped: number
    removed: number
  }
  snippets: Record<string, SnippetRecord>
}

// --- Constants ---

const DOCS_ROOT = join(import.meta.dir, "..")
const PAGES_DIR = join(DOCS_ROOT, "src", "pages")
const PROGRESS_FILE = join(import.meta.dir, "progress.json")

const ALLOWED_LANG_LABELS = new Set(["typescript", "rust", "scala", "moonbit"])

const EXCLUDED_DIRS = new Set(["how-to-guides", "rest-api"])

// --- Utility ---

function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\w\s-]/g, "")
    .replace(/\s+/g, "-")
    .trim()
}

function sha256(content: string): string {
  return crypto.createHash("sha256").update(content).digest("hex").slice(0, 16)
}

// --- MDX file discovery ---

async function discoverMdxFiles(dir: string): Promise<string[]> {
  const results: string[] = []
  const entries = await readdir(dir, { withFileTypes: true })

  for (const entry of entries) {
    const fullPath = join(dir, entry.name)
    if (entry.isDirectory()) {
      if (dir === PAGES_DIR && EXCLUDED_DIRS.has(entry.name)) continue
      results.push(...(await discoverMdxFiles(fullPath)))
    } else if (entry.name.endsWith(".mdx") || entry.name.endsWith(".md")) {
      // Skip _app, _document, etc.
      if (entry.name.startsWith("_")) continue
      results.push(fullPath)
    }
  }

  return results
}

// --- AST parsing and snippet extraction ---

function parseTabsItems(node: any): string[] | null {
  // Find the `items` attribute in the JSX element
  const itemsAttr = node.attributes?.find(
    (attr: any) => attr.type === "mdxJsxAttribute" && attr.name === "items"
  )
  if (!itemsAttr) return null

  // The value is an expression like {["TypeScript", "Rust"]}
  const exprValue =
    itemsAttr.value?.type === "mdxJsxAttributeValueExpression"
      ? itemsAttr.value.value
      : typeof itemsAttr.value === "string"
        ? itemsAttr.value
        : null

  if (!exprValue) return null

  // Only handle literal arrays like ["TypeScript", "Rust"]
  if (!exprValue.trim().startsWith("[")) return null

  // Extract quoted strings from the expression
  const matches = [...exprValue.matchAll(/["']([^"']+)["']/g)]
  if (matches.length === 0) return null
  return matches.map(m => m[1])
}

function isLanguageTabs(items: string[]): boolean {
  const normalized = items.map(i => i.toLowerCase())
  return (
    normalized.every(i => ALLOWED_LANG_LABELS.has(i)) &&
    normalized.some(i => i === "typescript" || i === "rust")
  )
}

type HeadingEntry = { depth: number; text: string; line: number }

function collectHeadings(tree: any): HeadingEntry[] {
  const headings: HeadingEntry[] = []
  visit(tree, "heading", (node: any) => {
    headings.push({
      depth: node.depth,
      text: toString(node),
      line: node.position?.start?.line ?? 0,
    })
  })
  return headings
}

function getHeadingPath(headings: HeadingEntry[], line: number): { depth: number; text: string }[] {
  // Find all headings before this line, build a stack
  const relevant = headings.filter(h => h.line < line)
  const stack: { depth: number; text: string }[] = []

  for (const h of relevant) {
    // Pop everything at same or deeper level
    while (stack.length > 0 && stack[stack.length - 1].depth >= h.depth) {
      stack.pop()
    }
    stack.push({ depth: h.depth, text: h.text })
  }

  return [...stack]
}

function extractCodeBlocks(tabNode: any, sourceLines: string[]): CodeBlock[] {
  const blocks: CodeBlock[] = []
  visit(tabNode, (node: any, _index: any, parent: any) => {
    // Skip descending into nested <Tabs> elements
    if (node.type === "mdxJsxFlowElement" && node.name === "Tabs" && node !== tabNode) {
      return "skip"
    }
    if (node.type === "code") {
      blocks.push({
        fenceLang: node.lang ?? null,
        meta: node.meta ?? null,
        code: node.value,
        lineStart: node.position?.start?.line ?? 0,
        lineEnd: node.position?.end?.line ?? 0,
      })
    }
  })
  return blocks
}

function extractTabMarkdown(tabNode: any, sourceLines: string[]): string {
  if (!tabNode.position) return ""
  const start = tabNode.position.start.line - 1
  const end = tabNode.position.end.line
  return sourceLines.slice(start, end).join("\n")
}

async function extractSnippetsFromFile(filePath: string): Promise<SnippetRecord[]> {
  const source = await readFile(filePath, "utf-8")
  const sourceLines = source.split("\n")
  const relPath = relative(DOCS_ROOT, filePath)

  const tree = unified().use(remarkParse).use(remarkMdx).parse(source)

  const headings = collectHeadings(tree)
  const snippets: SnippetRecord[] = []

  // Track occurrence counts per heading path for stable IDs
  const occurrenceCounts = new Map<string, number>()

  visit(tree, "mdxJsxFlowElement", (node: any) => {
    if (node.name !== "Tabs") return

    const items = parseTabsItems(node)
    if (!items || !isLanguageTabs(items)) return

    const line = node.position?.start?.line ?? 0
    const endLine = node.position?.end?.line ?? 0
    const headingPath = getHeadingPath(headings, line)
    const sectionHeading =
      headingPath.length > 0 ? headingPath[headingPath.length - 1].text : "(top-level)"

    // Build stable ID
    const headingSlug = headingPath.map(h => slugify(h.text)).join("/") || "top"
    const occKey = `${relPath}#${headingSlug}`
    const occCount = (occurrenceCounts.get(occKey) ?? 0) + 1
    occurrenceCounts.set(occKey, occCount)
    const id = `${relPath}#${headingSlug}@${occCount}`

    // Extract tabs
    const tabNodes = (node.children ?? []).filter(
      (child: any) => child.type === "mdxJsxFlowElement" && child.name === "Tabs.Tab"
    )

    const tabs: Record<string, TabContent> = {}
    const warnings: string[] = []

    if (tabNodes.length !== items.length) {
      warnings.push(
        `Tab count mismatch: ${items.length} items but ${tabNodes.length} Tabs.Tab elements`
      )
    }

    for (let i = 0; i < Math.min(items.length, tabNodes.length); i++) {
      const label = items[i]
      const tabNode = tabNodes[i]
      tabs[label] = {
        label,
        markdown: extractTabMarkdown(tabNode, sourceLines),
        codeBlocks: extractCodeBlocks(tabNode, sourceLines),
      }
    }

    const rawStart = (node.position?.start?.line ?? 1) - 1
    const rawEnd = node.position?.end?.line ?? sourceLines.length
    const rawTabsBlock = sourceLines.slice(rawStart, rawEnd).join("\n")

    snippets.push({
      id,
      filePath: relPath,
      lineStart: line,
      lineEnd: endLine,
      headingPath,
      sectionHeading,
      languagesPresent: items.filter((_, i) => i < tabNodes.length),
      tabs,
      rawTabsBlock,
      sourceHash: sha256(rawTabsBlock),
      status: "pending",
      warnings: warnings.length > 0 ? warnings : undefined,
    })
  })

  return snippets
}

// --- Progress file management ---

async function loadProgress(): Promise<ProgressFile | null> {
  if (!existsSync(PROGRESS_FILE)) return null
  const content = await readFile(PROGRESS_FILE, "utf-8")
  return JSON.parse(content) as ProgressFile
}

function updateCounts(progress: ProgressFile): void {
  const snippets = Object.values(progress.snippets)
  progress.counts = {
    total: snippets.filter(s => !s.removed).length,
    pending: snippets.filter(s => s.status === "pending" && !s.removed).length,
    completed: snippets.filter(s => s.status === "completed" && !s.removed).length,
    skipped: snippets.filter(s => s.status === "skipped" && !s.removed).length,
    removed: snippets.filter(s => s.removed).length,
  }
}

async function saveProgress(progress: ProgressFile): Promise<void> {
  updateCounts(progress)
  progress.generatedAt = new Date().toISOString()
  const tmpFile = PROGRESS_FILE + ".tmp"
  await writeFile(tmpFile, JSON.stringify(progress, null, 2) + "\n")
  await rename(tmpFile, PROGRESS_FILE)
}

// --- Commands ---

async function cmdScan(): Promise<void> {
  console.log("Scanning MDX files for language-tab code snippets...")

  const files = await discoverMdxFiles(PAGES_DIR)
  console.log(`Found ${files.length} MDX files to scan`)

  const allSnippets: SnippetRecord[] = []
  for (const file of files) {
    try {
      const snippets = await extractSnippetsFromFile(file)
      if (snippets.length > 0) {
        console.log(`  ${relative(DOCS_ROOT, file)}: ${snippets.length} snippet(s)`)
      }
      allSnippets.push(...snippets)
    } catch (e) {
      console.warn(`  ⚠️  Failed to parse ${relative(DOCS_ROOT, file)}: ${e}`)
    }
  }

  console.log(`\nTotal snippets found: ${allSnippets.length}`)

  // Merge with existing progress
  const existing = await loadProgress()
  const newProgress: ProgressFile = {
    version: 1,
    generatedAt: new Date().toISOString(),
    counts: { total: 0, pending: 0, completed: 0, skipped: 0, removed: 0 },
    snippets: {},
  }

  // Index new snippets
  const newIds = new Set<string>()
  for (const snippet of allSnippets) {
    newIds.add(snippet.id)
    const oldEntry = existing?.snippets[snippet.id]

    if (oldEntry) {
      // Preserve status and metadata, update content
      newProgress.snippets[snippet.id] = {
        ...snippet,
        status: oldEntry.status,
        completedAt: oldEntry.completedAt,
        note: oldEntry.note,
      }
    } else {
      newProgress.snippets[snippet.id] = snippet
    }
  }

  // Mark removed snippets
  if (existing) {
    for (const [id, entry] of Object.entries(existing.snippets)) {
      if (!newIds.has(id)) {
        newProgress.snippets[id] = { ...entry, removed: true }
      }
    }
  }

  await saveProgress(newProgress)

  updateCounts(newProgress)
  console.log("\nProgress saved to snippets/progress.json")
  printCounts(newProgress)
}

function printCounts(progress: ProgressFile): void {
  const c = progress.counts
  console.log(
    `  Total: ${c.total} | Pending: ${c.pending} | Completed: ${c.completed} | Skipped: ${c.skipped} | Removed: ${c.removed}`
  )
}

async function cmdStatus(): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  updateCounts(progress)
  printCounts(progress)

  // Show warnings
  const withWarnings = Object.values(progress.snippets).filter(
    s => s.warnings && s.warnings.length > 0 && !s.removed
  )
  if (withWarnings.length > 0) {
    console.log(`\nSnippets with warnings (${withWarnings.length}):`)
    for (const s of withWarnings) {
      console.log(`  ${s.id}: ${s.warnings!.join(", ")}`)
    }
  }

  // Show files breakdown
  const byFile = new Map<string, { pending: number; completed: number; skipped: number }>()
  for (const s of Object.values(progress.snippets)) {
    if (s.removed) continue
    if (!byFile.has(s.filePath)) {
      byFile.set(s.filePath, { pending: 0, completed: 0, skipped: 0 })
    }
    const counts = byFile.get(s.filePath)!
    counts[s.status]++
  }

  console.log("\nBy file:")
  for (const [file, counts] of Array.from(byFile.entries()).sort((a, b) =>
    a[0].localeCompare(b[0])
  )) {
    const parts = []
    if (counts.pending > 0) parts.push(`${counts.pending} pending`)
    if (counts.completed > 0) parts.push(`${counts.completed} done`)
    if (counts.skipped > 0) parts.push(`${counts.skipped} skipped`)
    console.log(`  ${file}: ${parts.join(", ")}`)
  }
}

async function cmdNext(): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  const next = Object.values(progress.snippets).find(s => s.status === "pending" && !s.removed)
  if (!next) {
    console.log("All snippets are completed or skipped!")
    return
  }

  console.log(`Next pending snippet: ${next.id}`)
  console.log(`  File: ${next.filePath}`)
  console.log(`  Lines: ${next.lineStart}-${next.lineEnd}`)
  console.log(`  Section: ${next.sectionHeading}`)
  console.log(`  Languages: ${next.languagesPresent.join(", ")}`)
  if (next.warnings?.length) {
    console.log(`  Warnings: ${next.warnings.join(", ")}`)
  }
}

function generatePrompt(snippet: SnippetRecord, golemRepoPath: string): string {
  const headingPathText = snippet.headingPath
    .map(h => `${"#".repeat(h.depth)} ${h.text}`)
    .join(" > ")

  const tabSummaries = Object.entries(snippet.tabs)
    .map(([label, tab]) => {
      const codeInfo =
        tab.codeBlocks.length > 0
          ? tab.codeBlocks
              .map(
                cb => `~~~~${cb.fenceLang ?? ""}${cb.meta ? " " + cb.meta : ""}\n${cb.code}\n~~~~`
              )
              .join("\n\n")
          : "(no fenced code blocks — tab contains prose or shell commands)"
      return `### ${label}\n\n${codeInfo}`
    })
    .join("\n\n")

  const missingLangs = ["TypeScript", "Rust", "Scala"].filter(
    l => !snippet.languagesPresent.includes(l)
  )
  const missingNote =
    missingLangs.length > 0
      ? `\n6. **Add missing language tabs**: ${missingLangs.join(", ")}. The \`<Tabs items={[...]}\` attribute must be updated to include them, and new \`<Tabs.Tab>\` blocks must be added.`
      : ""

  const blogUrls = [
    "https://blog.vigoo.dev/posts/golem15-part1-code-first-routes/",
    "https://blog.vigoo.dev/posts/golem15-part2-webhooks/",
    "https://blog.vigoo.dev/posts/golem15-part3-mcp/",
    "https://blog.vigoo.dev/posts/golem15-part4-nodejs/",
    "https://blog.vigoo.dev/posts/golem15-part5-scala/",
    "https://blog.vigoo.dev/posts/golem15-part6-user-defined-snapshotting/",
    "https://blog.vigoo.dev/posts/golem15-part7-config-and-secrets/",
    "https://blog.vigoo.dev/posts/golem15-part8-template-simplifications/",
    "https://blog.vigoo.dev/posts/golem15-part9-skills/",
    "https://blog.vigoo.dev/posts/golem15-part10-websocket/",
  ]

  return `Update the following Nextra MDX language-tabs snippet for **Golem 1.5**.

**Target file**: \`${snippet.filePath}\`
**Line range**: ${snippet.lineStart}–${snippet.lineEnd}
**Section**: ${headingPathText || "(top-level)"}
**Languages present**: ${snippet.languagesPresent.join(", ")}
**Snippet ID**: \`${snippet.id}\`

## Instructions

1. **Check correctness** — Determine whether the existing TypeScript and/or Rust code snippets are still correct for Golem 1.5. Look for API changes, renamed imports, changed type signatures, deprecated patterns, etc.
2. **Use these references** (read them as needed):
   - How-to guides in this repo: \`src/pages/how-to-guides/\` (these are authoritative for Golem 1.5 patterns)
   - Golem 1.5 blog posts:
${blogUrls.map(u => `     - ${u}`).join("\n")}
   - The Golem repository: \`${golemRepoPath}\` — especially test components under \`test-components/\` and SDK code
3. **Update** any outdated TypeScript or Rust snippets in-place in the MDX file.
4. **Preserve structure** — Keep \`<Tabs>\` / \`<Tabs.Tab>\` structure, \`storageKey\`, surrounding prose, and code block meta (e.g. \`copy\`).
5. **Keep the same abstraction level** — Don't add unnecessary complexity or remove intentional simplifications from the examples.${missingNote}
7. **Verify compilation** — For each language version:
   - Run \`golem new\` to create a minimal project
   - Place the snippet in enough context that it compiles (add necessary imports, wrapper code, Cargo.toml/package.json entries)
   - Run \`golem build\` to verify it compiles
   - The full project scaffolding does NOT need to be in the docs — only the snippet needs to be correct
   - If the snippet is intentionally partial (uses \`// ...\`), verify the non-elided parts at least
8. **After editing**, run this command to mark the snippet as completed:
   \`\`\`shell
   bun run snippets/manage-snippets.ts complete ${snippet.id}
   \`\`\`
9. **Summarize** what changed, what references were used, and what was verified.

## Current snippet block

~~~~mdx
${snippet.rawTabsBlock}
~~~~

## Extracted per-language content

${tabSummaries}
`
}

function generateAddLangPrompt(
  snippet: SnippetRecord,
  golemRepoPath: string,
  language: string
): string {
  const headingPathText = snippet.headingPath
    .map(h => `${"#".repeat(h.depth)} ${h.text}`)
    .join(" > ")

  const tabSummaries = Object.entries(snippet.tabs)
    .map(([label, tab]) => {
      const codeInfo =
        tab.codeBlocks.length > 0
          ? tab.codeBlocks
              .map(
                cb => `~~~~${cb.fenceLang ?? ""}${cb.meta ? " " + cb.meta : ""}\n${cb.code}\n~~~~`
              )
              .join("\n\n")
          : "(no fenced code blocks — tab contains prose or shell commands)"
      return `### ${label}\n\n${codeInfo}`
    })
    .join("\n\n")

  return `Add a **${language}** tab to the following Nextra MDX language-tabs snippet.

**Target file**: \`${snippet.filePath}\`
**Line range**: ${snippet.lineStart}–${snippet.lineEnd}
**Section**: ${headingPathText || "(top-level)"}
**Languages present**: ${snippet.languagesPresent.join(", ")}
**Snippet ID**: \`${snippet.id}\`

## Instructions

1. **Do NOT modify existing tabs** — The existing language tabs (${snippet.languagesPresent.join(", ")}) are already correct. Do not change them.
2. **Add a \`${language}\` tab** — Add a new \`<Tabs.Tab>\` block for ${language} with equivalent code/content.
3. **Update the \`<Tabs items={[...]}\` attribute** to include "${language}" in the list.
4. **Update the \`storageKey\`** attribute to include ${language} (keep the same format: language names separated by \`|\`).
5. **Use these references** to write correct ${language} code (read them as needed):
   - How-to guides in this repo: \`src/pages/how-to-guides/\` — look for ${language} examples
   - The Golem repository: \`${golemRepoPath}\` — especially test components under \`test-components/\` and SDK code for ${language}
6. **Match the abstraction level** of the existing tabs — if existing tabs show a simple example, keep the ${language} version equally simple.
7. **Verify compilation** — For the new ${language} tab:
   - Run \`golem new\` to create a minimal ${language} project
   - Place the snippet in enough context that it compiles
   - Run \`golem build\` to verify it compiles
   - If the snippet is intentionally partial (uses \`// ...\`), verify the non-elided parts at least
8. **After editing**, run this command to mark the snippet as completed:
   \`\`\`shell
   bun run snippets/manage-snippets.ts complete ${snippet.id}
   \`\`\`
9. **Summarize** what was added and what was verified.

## Current snippet block

~~~~mdx
${snippet.rawTabsBlock}
~~~~

## Extracted per-language content

${tabSummaries}
`
}

async function cmdPrompt(idOrNext: string, golemRepoPath: string): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  let snippet: SnippetRecord | undefined

  if (idOrNext === "--next") {
    snippet = Object.values(progress.snippets).find(s => s.status === "pending" && !s.removed)
    if (!snippet) {
      console.log("All snippets are completed or skipped!")
      return
    }
  } else {
    snippet = progress.snippets[idOrNext]
    if (!snippet) {
      console.log(`Snippet not found: ${idOrNext}`)
      return
    }
  }

  console.log(generatePrompt(snippet, golemRepoPath))
}

async function cmdComplete(id: string, note?: string): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  const snippet = progress.snippets[id]
  if (!snippet) {
    console.log(`Snippet not found: ${id}`)
    console.log("\nAvailable IDs (first 10):")
    Object.keys(progress.snippets)
      .slice(0, 10)
      .forEach(k => console.log(`  ${k}`))
    return
  }

  snippet.status = "completed"
  snippet.completedAt = new Date().toISOString()
  if (note) snippet.note = note

  await saveProgress(progress)
  console.log(`Marked as completed: ${id}`)
  printCounts(progress)
}

async function cmdSkip(id: string, note?: string): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  const snippet = progress.snippets[id]
  if (!snippet) {
    console.log(`Snippet not found: ${id}`)
    return
  }

  snippet.status = "skipped"
  if (note) snippet.note = note

  await saveProgress(progress)
  console.log(`Marked as skipped: ${id}`)
  printCounts(progress)
}

async function cmdList(filter?: string): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  const snippets = Object.values(progress.snippets)
    .filter(s => !s.removed)
    .filter(s => !filter || s.status === filter)
    .sort((a, b) => a.filePath.localeCompare(b.filePath) || a.lineStart - b.lineStart)

  for (const s of snippets) {
    const statusIcon = s.status === "completed" ? "✅" : s.status === "skipped" ? "⏭️" : "⬜"
    console.log(
      `${statusIcon} ${s.id}  [${s.languagesPresent.join(",")}]  L${s.lineStart}-${s.lineEnd}`
    )
  }
  console.log(`\n${snippets.length} snippet(s)`)
}

async function cmdPromptAddLang(
  idOrNext: string,
  golemRepoPath: string,
  language: string
): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  let snippet: SnippetRecord | undefined

  if (idOrNext === "--next") {
    snippet = Object.values(progress.snippets).find(s => s.status === "pending" && !s.removed)
    if (!snippet) {
      console.log("All snippets are completed or skipped!")
      return
    }
  } else {
    snippet = progress.snippets[idOrNext]
    if (!snippet) {
      console.log(`Snippet not found: ${idOrNext}`)
      return
    }
  }

  console.log(generateAddLangPrompt(snippet, golemRepoPath, language))
}

async function cmdResetMissingLang(language: string): Promise<void> {
  const progress = await loadProgress()
  if (!progress) {
    console.log("No progress file found. Run 'scan' first.")
    return
  }

  let resetCount = 0
  for (const snippet of Object.values(progress.snippets)) {
    if (snippet.removed) continue
    if (snippet.status !== "completed") continue
    const hasLang = snippet.languagesPresent.some(l => l.toLowerCase() === language.toLowerCase())
    if (!hasLang) {
      snippet.status = "pending"
      delete snippet.completedAt
      snippet.note = `Reset for adding ${language}`
      resetCount++
    }
  }

  await saveProgress(progress)
  console.log(`Reset ${resetCount} snippet(s) to pending (missing ${language})`)
  printCounts(progress)
}

// --- Main ---

async function main() {
  const args = process.argv.slice(2)
  const command = args[0]

  const golemRepoFlag = args.indexOf("--golem-repo")
  const golemRepoPath =
    golemRepoFlag !== -1 && args[golemRepoFlag + 1] ? args[golemRepoFlag + 1] : "../golem"

  switch (command) {
    case "scan":
      await cmdScan()
      break

    case "status":
      await cmdStatus()
      break

    case "next":
      await cmdNext()
      break

    case "prompt": {
      const target = args[1] ?? "--next"
      await cmdPrompt(target, golemRepoPath)
      break
    }

    case "complete": {
      const id = args[1]
      if (!id) {
        console.log("Usage: complete <snippet-id> [--note <text>]")
        return
      }
      const noteIdx = args.indexOf("--note")
      const note = noteIdx !== -1 ? args.slice(noteIdx + 1).join(" ") : undefined
      await cmdComplete(id, note)
      break
    }

    case "skip": {
      const id = args[1]
      if (!id) {
        console.log("Usage: skip <snippet-id> [--note <text>]")
        return
      }
      const noteIdx = args.indexOf("--note")
      const note = noteIdx !== -1 ? args.slice(noteIdx + 1).join(" ") : undefined
      await cmdSkip(id, note)
      break
    }

    case "list": {
      const filter = args[1] // optional: "pending", "completed", "skipped"
      await cmdList(filter)
      break
    }

    case "prompt-add-lang": {
      const target = args[1] ?? "--next"
      const langIdx = args.indexOf("--language")
      const language = langIdx !== -1 ? args[langIdx + 1] : undefined
      if (!language) {
        console.log("Usage: prompt-add-lang [id|--next] --language <lang>")
        return
      }
      await cmdPromptAddLang(target, golemRepoPath, language)
      break
    }

    case "reset-missing-lang": {
      const langIdx = args.indexOf("--language")
      const language = langIdx !== -1 ? args[langIdx + 1] : args[1]
      if (!language) {
        console.log("Usage: reset-missing-lang --language <lang>")
        return
      }
      await cmdResetMissingLang(language)
      break
    }

    default:
      console.log(`Usage: bun run snippets/manage-snippets.ts <command>

Commands:
  scan                        Scan MDX files and update progress.json
  status                      Show progress summary
  next                        Show next pending snippet
  list [status]               List all snippets, optionally filtered by status
  prompt [id|--next]          Generate Amp prompt for a snippet
  prompt-add-lang [id|--next] --language <lang>
                              Generate prompt to add a specific language tab
  reset-missing-lang --language <lang>
                              Reset completed snippets missing a language to pending
  complete <id> [--note ...]  Mark snippet as completed
  skip <id> [--note ...]      Mark snippet as skipped

Options:
  --golem-repo <path>         Path to golem repo (default: ../golem)
`)
  }
}

main().catch(e => {
  console.error("Error:", e)
  process.exit(1)
})
