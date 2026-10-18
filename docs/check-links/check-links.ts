import { glob } from "glob"
import path from "path"
import { promises as fs } from "fs"
import { unified } from "unified"
import remarkParse from "remark-parse"
import remarkMdx from "remark-mdx"
import Slugger from "github-slugger"
import { toString } from "mdast-util-to-string"
import { visit } from "unist-util-visit"

const args = process.argv.slice(2)
const hosts = new Set<string>()
const files: string[] = []
for (let i = 0; i < args.length; i++) {
  if (args[i] === "--external-host") {
    if (!args[i + 1]) throw new Error("--external-host requires a host")
    hosts.add(args[++i])
  } else if (args[i].startsWith("--")) throw new Error(`Unknown option: ${args[i]}`)
  else files.push(args[i])
}
const root = path.resolve("src/content")
const publicRoot = path.resolve("public")
type Page = { links: string[]; anchors: Set<string> }
const pages = new Map<string, Promise<Page>>()
function page(file: string): Promise<Page> {
  if (!pages.has(file))
    pages.set(
      file,
      fs.readFile(file, "utf8").then(source => {
        const tree = unified().use(remarkParse).use(remarkMdx).parse(source)
        const links: string[] = []
        const anchors = new Set<string>()
        const slugger = new Slugger()
        const definitions = new Map<string, string>()
        visit(tree, "definition", node => {
          definitions.set(node.identifier, node.url)
        })
        visit(tree, (node, index, parent) => {
          if (node.type === "heading" && node.depth !== 1) anchors.add(slugger.slug(toString(node)))
          // Nextra allocates hidden headings for language tabs in document order.
          if (node.type === "mdxJsxFlowElement" && (node as any).name === "Tabs.Tab") {
            const items = (parent as any)?.attributes?.find((a: any) => a.name === "items")?.value
              ?.data?.estree?.body?.[0]?.expression?.elements
            const label = items?.[index!]?.value
            if (typeof label === "string") anchors.add(slugger.slug(label))
          }
          if (node.type === "link" || node.type === "image") links.push(node.url)
          if (node.type === "linkReference" || node.type === "imageReference") {
            const url = definitions.get(node.identifier)
            if (url) links.push(url)
          }
          if (node.type === "mdxJsxFlowElement" || node.type === "mdxJsxTextElement") {
            // Field renders its literal meta.path as an anchor (app-manifest-schema.tsx).
            if ((node as any).name === "Field") {
              const props =
                (node as any).attributes?.find((a: any) => a.name === "meta")?.value?.data?.estree
                  ?.body?.[0]?.expression?.properties ?? []
              const fieldPath = props.find((p: any) => p.key?.name === "path")?.value?.value
              if (typeof fieldPath === "string")
                anchors.add(`fields_${fieldPath.replaceAll(/[<>\[\].*-]/g, "_")}`)
            }
            for (const attr of (node as any).attributes ?? []) {
              if (typeof attr.value !== "string") continue
              if (attr.name === "id") anchors.add(attr.value)
              if (["href", "src", "lightSrc", "darkSrc"].includes(attr.name)) links.push(attr.value)
            }
          }
        })
        return { links: [...new Set(links)], anchors }
      })
    )
  return pages.get(file)!
}
async function isFile(file: string) {
  return fs.stat(file).then(
    s => s.isFile(),
    () => false
  )
}
async function resolvePage(base: string) {
  for (const candidate of [
    base,
    `${base}.mdx`,
    `${base}.md`,
    path.join(base, "index.mdx"),
    path.join(base, "index.md"),
  ]) {
    if (await isFile(candidate)) return candidate
  }
}
const external = new Map<string, Promise<void>>()
async function check(link: string, source: string) {
  if (/^(https?:)?\/\//.test(link)) {
    const url = new URL(link, "https://learn.golem.cloud")
    if (!hosts.has(url.host)) return
    if (!external.has(url.href) && external.size >= 50)
      throw new Error("External URL budget exceeded (50)")
    if (!external.has(url.href))
      external.set(
        url.href,
        (async () => {
          const response = await fetch(url, {
            method: "HEAD",
            redirect: "manual",
            signal: AbortSignal.timeout(5000),
          })
          if (!response.ok && !(response.status >= 300 && response.status < 400))
            throw new Error(`HTTP ${response.status}`)
        })()
      )
    return external.get(url.href)
  }
  if (/^(mailto:|tel:|data:)/.test(link)) return
  if (/^[a-z][a-z0-9+.-]*:/i.test(link)) throw new Error("Unsupported URL scheme")
  const [beforeHash, fragment] = link.split("#", 2)
  const pathname = decodeURIComponent(beforeHash.split("?", 1)[0])
  let target: string | undefined
  if (!pathname) target = source
  else if (pathname.startsWith("/")) {
    const asset = path.resolve(publicRoot, `.${pathname}`)
    if (asset.startsWith(publicRoot + path.sep) && (await isFile(asset))) return
    target = await resolvePage(path.resolve(root, `.${pathname}`))
  } else target = await resolvePage(path.resolve(path.dirname(source), pathname))
  if (!target) throw new Error("Missing file")
  if (
    fragment &&
    /\.mdx?$/.test(target) &&
    !(await page(target)).anchors.has(decodeURIComponent(fragment))
  ) {
    throw new Error(`Missing anchor #${fragment}`)
  }
}

async function main() {
  const selected = files.length ? files : await glob("src/content/**/*.{mdx,md}")
  console.log(
    `Checking links in ${selected.length} files (external hosts: ${[...hosts].join(", ") || "none; offline"})`
  )
  let failures = 0
  // Sequential requests bound external concurrency to one; every request has a timeout.
  for (const file of selected) {
    const source = path.resolve(file)
    try {
      for (const link of (await page(source)).links) {
        try {
          await check(link, source)
        } catch (error) {
          failures++
          console.error(`${file}: ${link}: ${(error as Error).message}`)
        }
      }
    } catch (error) {
      failures++
      console.error(`${file}: ${(error as Error).message}`)
    }
  }
  console.log(`Link errors: ${failures}`)
  process.exitCode = failures ? 1 : 0
}
main().catch(error => {
  console.error(error)
  process.exitCode = 1
})
