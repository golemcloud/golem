import { writeFile, readFile, readdir, mkdir, rm } from "fs/promises"
import { join } from "path"
import { existsSync } from "fs"

const OUTPUT_PATH = "./src/content/how-to-guides"
const GITHUB_API_BASE = "https://api.github.com/repos/golemcloud/golem/contents"
const GITHUB_RAW_BASE = "https://raw.githubusercontent.com/golemcloud/golem/main"
const SKILLS_REL_PATH = "golem-skills/skills"

const CATEGORIES: Record<string, string> = {
  common: "General",
  rust: "Rust",
  ts: "TypeScript",
  scala: "Scala",
  moonbit: "MoonBit",
}

type Skill = {
  name: string
  title: string
  category: string
  content: string
}

main().catch(e => {
  console.error("Failed to sync skills:", e)
  process.exit(1)
})

async function main() {
  const args = process.argv.slice(2)
  const localIndex = args.indexOf("--local")
  const isLocal = localIndex !== -1
  const localPath = isLocal ? args[localIndex + 1] : undefined

  if (isLocal && !localPath) {
    throw new Error("--local requires a path argument, e.g.: --local ../golem")
  }

  console.log(
    isLocal ? `Reading skills from local path: ${localPath}` : "Fetching skills from GitHub..."
  )

  const skills = isLocal ? await discoverSkillsLocal(localPath!) : await discoverSkillsRemote()

  console.log(`Found ${skills.length} skills across ${Object.keys(CATEGORIES).length} categories`)

  const skillMap = buildSkillMap(skills)

  // Clean and recreate output directory
  if (existsSync(OUTPUT_PATH)) {
    await rm(OUTPUT_PATH, { recursive: true })
  }
  await mkdir(OUTPUT_PATH, { recursive: true })

  for (const category of Object.keys(CATEGORIES)) {
    await mkdir(join(OUTPUT_PATH, category), { recursive: true })
  }

  // Write skill pages
  for (const skill of skills) {
    const mdx = transformContent(skill, skillMap)
    const filePath = join(OUTPUT_PATH, skill.category, `${skill.name}.mdx`)
    await writeFile(filePath, mdx)
  }

  // Write _meta.json files for each category
  for (const [category, categoryTitle] of Object.entries(CATEGORIES)) {
    const categorySkills = skills
      .filter(s => s.category === category)
      .sort((a, b) => a.title.localeCompare(b.title))

    const meta: Record<string, string> = {}
    for (const skill of categorySkills) {
      meta[skill.name] = skill.title
    }

    await writeFile(
      join(OUTPUT_PATH, category, "_meta.js"),
      "export default " + JSON.stringify(meta, null, 2) + ";\n"
    )
  }

  // Write top-level _meta.js for how-to-guides
  const topMeta: Record<string, string | object> = {}
  for (const [category, categoryTitle] of Object.entries(CATEGORIES)) {
    topMeta[category] = { title: categoryTitle }
  }
  await writeFile(
    join(OUTPUT_PATH, "_meta.js"),
    "export default " + JSON.stringify(topMeta, null, 2) + ";\n"
  )

  // Write landing pages
  await writeLandingPage(skills)
  await writeCategoryLandingPages(skills)

  console.log("Finished syncing skills")
}

// --- Discovery ---

async function discoverSkillsLocal(golemRepoPath: string): Promise<Skill[]> {
  const skillsRoot = join(golemRepoPath, SKILLS_REL_PATH)
  const skills: Skill[] = []

  for (const category of Object.keys(CATEGORIES)) {
    const categoryPath = join(skillsRoot, category)
    if (!existsSync(categoryPath)) {
      console.warn(`Category directory not found: ${categoryPath}`)
      continue
    }

    const entries = await readdir(categoryPath, { withFileTypes: true })
    for (const entry of entries) {
      if (!entry.isDirectory()) continue

      const skillFile = join(categoryPath, entry.name, "SKILL.md")
      if (!existsSync(skillFile)) continue

      const raw = await readFile(skillFile, "utf-8")
      const { title, content } = parseSkillFile(raw, entry.name)
      skills.push({ name: entry.name, title, category, content })
    }
  }

  return skills
}

async function discoverSkillsRemote(): Promise<Skill[]> {
  const skills: Skill[] = []

  for (const category of Object.keys(CATEGORIES)) {
    const apiUrl = `${GITHUB_API_BASE}/${SKILLS_REL_PATH}/${category}`
    const response = await fetch(apiUrl, {
      headers: {
        Accept: "application/vnd.github.v3+json",
        ...(process.env.GITHUB_TOKEN ? { Authorization: `token ${process.env.GITHUB_TOKEN}` } : {}),
      },
    })

    if (!response.ok) {
      throw new Error(`GitHub API error for ${category}: ${response.status} ${response.statusText}`)
    }

    const entries = (await response.json()) as Array<{ name: string; type: string }>
    const dirs = entries.filter(e => e.type === "dir")

    console.log(`  ${CATEGORIES[category]}: ${dirs.length} skills`)

    // Fetch all SKILL.md files in parallel
    const fetches = dirs.map(async dir => {
      const rawUrl = `${GITHUB_RAW_BASE}/${SKILLS_REL_PATH}/${category}/${dir.name}/SKILL.md`
      const res = await fetch(rawUrl)
      if (!res.ok) {
        console.warn(`  Skipping ${dir.name}: SKILL.md not found`)
        return null
      }
      const raw = await res.text()
      const { title, content } = parseSkillFile(raw, dir.name)
      return { name: dir.name, title, category, content } as Skill
    })

    const results = await Promise.all(fetches)
    skills.push(...results.filter((s): s is Skill => s !== null))
  }

  return skills
}

// --- Parsing ---

function parseSkillFile(raw: string, fallbackName: string): { title: string; content: string } {
  // Strip YAML frontmatter
  let content = raw
  if (raw.startsWith("---")) {
    const endIndex = raw.indexOf("---", 3)
    if (endIndex !== -1) {
      content = raw.slice(endIndex + 3).trimStart()
    }
  }

  // Extract H1 title
  const h1Match = content.match(/^#\s+(.+)$/m)
  const title = h1Match ? h1Match[1].trim() : humanize(fallbackName)

  return { title, content }
}

function humanize(slug: string): string {
  return slug
    .replace(/^golem-/, "")
    .replace(/-(rust|ts|scala)$/, "")
    .split("-")
    .map(w => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ")
}

// --- Content transformation ---

function buildSkillMap(skills: Skill[]): Map<string, string> {
  const map = new Map<string, string>()
  for (const skill of skills) {
    map.set(skill.name, `/how-to-guides/${skill.category}/${skill.name}`)
  }
  return map
}

function transformContent(skill: Skill, skillMap: Map<string, string>): string {
  let content = skill.content

  // Rename "Related Skills" section header
  content = content.replace(/^###?\s+Related Skills\s*$/gm, "### Related Guides")

  // Clean up table headers for doc context
  content = content.replace(/\|\s*When to Load\s*\|/g, "| Description |")
  content = content.replace(/\|\s*Skill\s*\|\s*Description\s*\|/g, "| Guide | Description |")

  // Convert backticked skill references to links
  // Match `skill-name` where skill-name is a known skill
  Array.from(skillMap.entries()).forEach(([name, path]) => {
    // Replace `skill-name` with link (but not inside already-linked text)
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    const pattern = new RegExp("(?<!\\[)`" + escaped + "`(?!\\])", "g")
    content = content.replace(pattern, `[\`${name}\`](${path})`)
  })

  // Convert relative markdown links to sibling skill SKILL.md files into doc links
  // e.g. [`golem-custom-snapshot-ts`](../golem-custom-snapshot-ts/SKILL.md) -> doc path
  Array.from(skillMap.entries()).forEach(([name, path]) => {
    const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
    const pattern = new RegExp("\\(\\.\\.\\/" + escaped + "\\/SKILL\\.md\\)", "g")
    content = content.replace(pattern, `(${path})`)
  })

  // Clean up AI-agent language
  content = content.replace(/[Ll]oad the \[/g, "See the [")
  content = content.replace(/[Ll]oad the (`[^`]+`)/g, "See the $1")
  content = content.replace(/ skill\)/g, " guide)")
  content = content.replace(/ skill\./g, " guide.")
  content = content.replace(/ skill$/gm, " guide")

  return content
}

// --- Output ---

async function writeLandingPage(skills: Skill[]) {
  const categoryCounts = Object.entries(CATEGORIES).map(([cat, title]) => {
    const count = skills.filter(s => s.category === cat).length
    return { cat, title, count }
  })

  const cards = categoryCounts
    .map(
      ({ cat, title, count }) =>
        `  <Cards.Card title="${title} (${count})" href="how-to-guides/${cat}" />`
    )
    .join("\n")

  const page = `import { Cards } from "nextra/components"

# How-To Guides

Practical, step-by-step guides for building with Golem. Each guide covers a specific task with code examples and best practices.

<Cards num={2}>
${cards}
</Cards>
`

  await writeFile("./src/content/how-to-guides.mdx", page)
}

async function writeCategoryLandingPages(skills: Skill[]) {
  for (const [category, categoryTitle] of Object.entries(CATEGORIES)) {
    const categorySkills = skills
      .filter(s => s.category === category)
      .sort((a, b) => a.title.localeCompare(b.title))

    if (categorySkills.length === 0) continue

    const cards = categorySkills
      .map(s => `  <Cards.Card title="${s.title}" href="${category}/${s.name}" />`)
      .join("\n")

    const description =
      category === "common"
        ? "Language-agnostic guides covering the Golem CLI, project setup, deployment, and configuration."
        : `Guides specific to developing Golem agents in ${categoryTitle}.`

    const page = `import { Cards } from "nextra/components"

# ${categoryTitle} How-To Guides

${description}

<Cards num={1}>
${cards}
</Cards>
`

    await writeFile(join(OUTPUT_PATH, category + ".mdx"), page)
  }
}
