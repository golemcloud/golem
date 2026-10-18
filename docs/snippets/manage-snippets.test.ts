import { test, expect } from "bun:test"
import { mkdtemp, mkdir, writeFile, readFile, rm } from "fs/promises"
import { tmpdir } from "os"
import { join } from "path"

test("scanner CLI selects a version, recognizes six SDKs and never equates prose or review with compilation", async () => {
  const dir = await mkdtemp(join(tmpdir(), "docs-snippets-"))
  try {
    await mkdir(join(dir, "v1.6"))
    await mkdir(join(dir, "next"))
    const labels = ["TypeScript", "Effect TypeScript", "Rust", "Scala", "MoonBit", "Go"]
    const source = `<Tabs items={${JSON.stringify(labels)}}>\n${labels.map((_, i) => `<Tabs.Tab>\n\n${i ? "Prose only." : "```ts\nconst value = 1\n```"}\n\n</Tabs.Tab>`).join("\n")}\n</Tabs>\n`
    const file = join(dir, "v1.6/example.mdx")
    await writeFile(file, source)
    await writeFile(join(dir, "next/invalid.mdx"), "<invalid")
    const progress = join(dir, "progress.json")
    const run = async (...args: string[]) => {
      const child = Bun.spawn(
        [
          process.execPath,
          join(import.meta.dir, "manage-snippets.ts"),
          ...args,
          "--content-root",
          dir,
          "--version",
          "v1.6",
          "--progress",
          progress,
        ],
        { stdout: "pipe", stderr: "pipe" }
      )
      expect(await child.exited).toBe(0)
    }
    await run("scan")
    const data = JSON.parse(await readFile(progress, "utf8"))
    const record: any = Object.values(data.snippets)[0]
    expect(record.languagesPresent).toEqual(labels)
    expect(record.tabs["Effect TypeScript"].sdk).toBe("typescript-effect")
    expect(record.tabs.Go.coverage).toBe("prose-only")
    expect(record.tabs.TypeScript.coverage).toBe("fenced-unverified")
    await run("complete", record.id)
    expect(
      JSON.parse(await readFile(progress, "utf8")).snippets[record.id].tabs.TypeScript.validation
    ).toBe("not-checked")
    await writeFile(file, source.replace("value = 1", "value = 2"))
    await run("scan")
    expect(JSON.parse(await readFile(progress, "utf8")).snippets[record.id].status).toBe("pending")
    await writeFile(file, source.replaceAll("Effect TypeScript", "Effect"))
    await run("scan")
    expect(JSON.parse(await readFile(progress, "utf8")).snippets[record.id].tabs.Effect.sdk).toBe(
      "typescript-effect"
    )
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})
