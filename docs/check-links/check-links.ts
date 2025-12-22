import { glob } from "glob"
import path from "path"
import { promises as fs } from "fs"
import markdownLinkExtractor from "markdown-link-extractor"

type BaseResult = { link: string }

type LinkCheckResult =
  | (BaseResult &
      (
        | {
            status: "ignored"
          }
        | {
            status: "alive"
            filePath?: string
          }
      ))
  | CheckLinkError

type CheckLinkError = BaseResult &
  (
    | {
        status: "dead"
      }
    | {
        status: "error"
        error: Error
      }
  )

type CheckFileResult =
  | {
      type: "success"
    }
  | {
      type: "dead-links"
      file: string
      deadLinks: CheckLinkError[]
    }
  | {
      type: "fatal"
      file: string
      error: Error
    }

// Helper to get base URL for file system paths
const makeBaseUrl = (file: string) =>
  process.platform === "win32" ? path.resolve(file).replace(/\\/g, "/") : path.resolve(file)

const codeBlockRegex = /^```[\S\s]+?^```$/gm
const headingRegex = /^#+ (.+)$/gm

// Extract all links from markdown content
const extractLinks = (content: string): string[] => {
  const links = markdownLinkExtractor(content)
  return Array.from<string>(new Set(links)).filter(
    link => link.length > 0 && !link.startsWith("mailto:")
  )
}

// Extract all headings and convert to valid anchor links
const extractHeadings = (content: string): string[] => {
  // First remove code blocks
  const contentWithoutCode = content.replace(codeBlockRegex, "")

  const headings: string[] = []
  let match

  while ((match = headingRegex.exec(contentWithoutCode)) !== null) {
    const heading = match[1]
      .toLowerCase()
      .replace(/[^\w\s-]/g, "")
      .replace(/\s+/g, "-")
    headings.push(heading)
  }

  // Handle duplicate headings by adding numbers
  const uniqueHeadings = new Map<string, number>()
  return headings.map(heading => {
    const count = uniqueHeadings.get(heading) || 0
    uniqueHeadings.set(heading, count + 1)
    return count === 0 ? heading : `${heading}-${count}`
  })
}

namespace MarkdownPath {
  const cache = new Map<string, string | null>()
  export const resolve = async (markdownPath: string): Promise<string | null> => {
    if (cache.has(markdownPath)) {
      return cache.get(markdownPath)!
    }

    try {
      // handle all page layout variants
      const variants = [
        markdownPath,
        `${markdownPath}.mdx`,
        `${markdownPath}.md`,
        path.join(markdownPath, "index.mdx"),
        path.join(markdownPath, "index.md"),
      ]
      const filePath = await Promise.any(
        variants.map(variant => fs.access(variant).then(() => variant))
      )
      cache.set(markdownPath, filePath)
      return filePath
    } catch {
      return null
    }
  }
}

const checkLink = async (params: {
  link: string
  baseUrl: string
  headings: string[]
  ignorePatterns?: RegExp[]
}): Promise<LinkCheckResult> => {
  let { link, baseUrl, headings, ignorePatterns = [] } = params
  // Check if link should be ignored
  if (ignorePatterns.some(pattern => pattern.test(link))) {
    return { link, status: "ignored" }
  }

  try {
    // Handle same page anchor links
    if (link.startsWith("#")) {
      const anchorId = link.slice(1)
      return {
        link,
        status: headings.includes(anchorId) ? "alive" : "dead",
      }
    }

    // For other page anchors links only check the base link
    const anchorIdx = link.indexOf("#")
    if (anchorIdx !== -1) {
      link = link.substring(0, anchorIdx)
    }

    // Handle absolute links from `src/pages/docs/`
    if (link.startsWith("/")) {
      const docPath = path.join(process.cwd(), "src/pages", link)
      const filePath = await MarkdownPath.resolve(docPath)
      return filePath !== null ? { link, status: "alive", filePath } : { link, status: "dead" }
    }

    // Handle relative links
    if (!link.startsWith("http")) {
      const absolutePath = path.resolve(path.dirname(baseUrl), link)
      const filePath = await MarkdownPath.resolve(absolutePath)
      return filePath !== null ? { link, status: "alive", filePath } : { link, status: "dead" }
    }

    // Handle external links
    const response = await fetch(link, { method: "HEAD" })
    return {
      link,
      status: response.ok ? "alive" : "dead",
    }
  } catch (error) {
    return {
      link,
      status: "error",
      error: error as Error,
    }
  }
}

const checkFile = async (fileStr: string): Promise<CheckFileResult> => {
  try {
    const content = await fs.readFile(fileStr, "utf8")
    const links = extractLinks(content)
    const headings = extractHeadings(content)
    const baseUrl = makeBaseUrl(fileStr)

    const results = await Promise.all(
      links.map(link =>
        checkLink({
          link,
          baseUrl,
          headings,
          ignorePatterns: [/^https?:\/\//, /^.*\/images\/.*/], // Ignore external links for now
        })
      )
    )

    const deadLinks: CheckLinkError[] = results.filter(
      (result): result is CheckLinkError => result.status === "dead" || result.status === "error"
    )

    if (deadLinks.length > 0) {
      return { type: "dead-links", file: fileStr, deadLinks }
    }
    return { type: "success" }
  } catch (err) {
    return { type: "fatal", file: fileStr, error: err as Error }
  }
}

const execute = async (files: string[]) => {
  const markdownFiles = files.filter(file => file.endsWith(".mdx") || file.endsWith(".md"))
  console.log(`Checking links in ${markdownFiles.length} files`)

  const errors = (await Promise.all(markdownFiles.map(checkFile))).filter(
    result => result.type !== "success"
  )

  if (errors.length > 0) {
    console.error(`\nFound ${errors.length} files with broken links:`)
    errors.forEach(error => {
      switch (error.type) {
        case "dead-links":
          console.error(`- ${error.file}:`)
          error.deadLinks.forEach(link => {
            if (link.status === "error") {
              console.error(`  - ${link.link} (${link.status} - ${link.error.message})`)
            } else {
              console.error(`  - ${link.link} (${link.status})`)
            }
          })
          break
        case "fatal":
          console.error(`- ${error.file}: ${error.error.message}`)
          break
      }
    })
    process.exit(1)
  }
}

const main = async () => {
  const args = process.argv.slice(2)
  const files =
    args.length > 0 ? args : await glob("src/pages/**/*.{mdx,md}", { ignore: "node_modules/**" })
  await execute(files)
}

main()
  .then(() => process.exit(0))
  .catch(err => {
    console.error("Failed to check links:", err)
    process.exit(1)
  })
