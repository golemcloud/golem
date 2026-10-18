import { test, expect } from "bun:test"
import { mkdtemp, mkdir, writeFile, rm } from "fs/promises"
import { tmpdir } from "os"
import { join } from "path"

test("link CLI validates cross-page anchors, reference images and literal JSX assets offline", async () => {
  const dir = await mkdtemp(join(tmpdir(), "docs-links-"))
  try {
    await mkdir(join(dir, "src/content/v1.6"), { recursive: true })
    await mkdir(join(dir, "public/images"), { recursive: true })
    await writeFile(join(dir, "public/images/a.svg"), "<svg />")
    await writeFile(
      join(dir, "src/content/v1.6/target.mdx"),
      '## Héllo `world`!\n\n## Héllo world!\n\n<span id="custom" />\n\n<Field meta={{ path: "includes" }} />\n\n<Tabs items={["Go"]}>\n<Tabs.Tab>\n\nText\n\n</Tabs.Tab>\n</Tabs>\n\n## Go\n'
    )
    const source = join(dir, "src/content/v1.6/source.mdx")
    const run = async () => {
      const child = Bun.spawn([process.execPath, join(import.meta.dir, "check-links.ts"), source], {
        cwd: dir,
        stdout: "pipe",
        stderr: "pipe",
      })
      return {
        code: await child.exited,
        output:
          (await new Response(child.stdout).text()) + (await new Response(child.stderr).text()),
      }
    }
    await writeFile(
      source,
      '## Local\n\n[local](#local)\n\n[tab](target#go-1)\n\n[field](target#fields_includes)\n\n[ok](/v1.6/target?q=1#héllo-world-1)\n\n[custom](target#custom)\n\n![asset][a]\n\n[a]: /images/a.svg?x=1\n\n<img src="/images/a.svg" />\n\n[offline](https://must-not-fetch.invalid)\n'
    )
    expect((await run()).code).toBe(0)
    await writeFile(
      source,
      '[bad](/v1.6/target#absent)\n\n![bad](/images/missing.svg)\n\n<img src="/images/missing2.svg" />\n\n<ThemedImage lightSrc="/images/missing-light.webp" darkSrc="/images/missing-dark.webp" />\n'
    )
    const result = await run()
    expect(result.code).toBe(1)
    expect(result.output).toContain("Missing anchor #absent")
    expect(result.output).toContain("missing-light.webp")
    expect(result.output).toContain("missing-dark.webp")
    expect(result.output).toContain("Link errors: 5")
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test("external checks are host-selective, deduplicated and report HTTP failure", async () => {
  let requests = 0
  const server = Bun.serve({
    port: 0,
    fetch() {
      requests++
      return new Response(null, { status: 404 })
    },
  })
  const dir = await mkdtemp(join(tmpdir(), "docs-external-"))
  try {
    const file = join(dir, "links.mdx")
    await writeFile(file, `[bad](${server.url})\n\n[skip](https://must-not-fetch.invalid)\n`)
    const child = Bun.spawn(
      [
        process.execPath,
        join(import.meta.dir, "check-links.ts"),
        "--external-host",
        server.url.host,
        file,
      ],
      { cwd: dir, stdout: "pipe", stderr: "pipe" }
    )
    expect(await child.exited).toBe(1)
    expect(await new Response(child.stderr).text()).toContain("HTTP 404")
    expect(requests).toBe(1)
  } finally {
    server.stop()
    await rm(dir, { recursive: true, force: true })
  }
})
