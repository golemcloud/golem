#!/usr/bin/env bun
/**
 * Automated snippet update loop.
 *
 * Picks the next pending snippet, generates a prompt, spawns an `amp -x` thread
 * to process it, and repeats until all snippets are done.
 *
 * Usage:
 *   bun run snippets/run-loop.ts [options]
 *
 * Options:
 *   --golem-repo <path>   Path to golem repo (default: ../golem)
 *   --max <n>             Max snippets to process in this run (default: unlimited)
 *   --dry-run             Print the prompt but don't run Amp
 *   --mode <mode>         Amp agent mode (default: smart)
 *   --continue-on-error   Continue to next snippet if one fails
 *   --add-language <lang> Only add a missing language tab (resets completed snippets missing it)
 */

import { readFile, writeFile, appendFile } from "fs/promises"
import { join } from "path"
import { existsSync } from "fs"
import { spawn } from "child_process"

const SNIPPETS_DIR = import.meta.dir
const PROGRESS_FILE = join(SNIPPETS_DIR, "progress.json")
const LOG_FILE = join(SNIPPETS_DIR, "loop.log")
const MANAGE_SCRIPT = join(SNIPPETS_DIR, "manage-snippets.ts")

type SnippetRecord = {
  id: string
  filePath: string
  lineStart: number
  lineEnd: number
  sectionHeading: string
  languagesPresent: string[]
  status: "pending" | "completed" | "skipped"
  removed?: boolean
}

type ProgressFile = {
  version: number
  counts: {
    total: number
    pending: number
    completed: number
    skipped: number
    removed: number
  }
  snippets: Record<string, SnippetRecord>
}

function parseArgs() {
  const args = process.argv.slice(2)
  const opts = {
    golemRepo: "../golem",
    max: Infinity,
    dryRun: false,
    mode: "smart",
    continueOnError: false,
    addLanguage: undefined as string | undefined,
  }

  for (let i = 0; i < args.length; i++) {
    switch (args[i]) {
      case "--golem-repo":
        if (!args[i + 1] || args[i + 1].startsWith("--")) {
          console.error("Error: --golem-repo requires a path argument")
          process.exit(1)
        }
        opts.golemRepo = args[++i]
        break
      case "--max": {
        const val = parseInt(args[++i], 10)
        if (isNaN(val) || val <= 0) {
          console.error("Error: --max requires a positive integer")
          process.exit(1)
        }
        opts.max = val
        break
      }
      case "--dry-run":
        opts.dryRun = true
        break
      case "--mode":
        if (!args[i + 1] || args[i + 1].startsWith("--")) {
          console.error("Error: --mode requires a value")
          process.exit(1)
        }
        opts.mode = args[++i]
        break
      case "--continue-on-error":
        opts.continueOnError = true
        break
      case "--add-language":
        if (!args[i + 1] || args[i + 1].startsWith("--")) {
          console.error("Error: --add-language requires a language name")
          process.exit(1)
        }
        opts.addLanguage = args[++i]
        break
      case "--help":
      case "-h":
        printHelp()
        process.exit(0)
        break
      default:
        console.error(`Unknown option: ${args[i]}`)
        printHelp()
        process.exit(1)
    }
  }
  return opts
}

function printHelp() {
  console.log(`Usage: bun run snippets/run-loop.ts [options]

Automated snippet update loop. Picks the next pending snippet, generates a
prompt, spawns an Amp thread to process it, and repeats.

Options:
  --golem-repo <path>     Path to golem repo (default: ../golem)
  --max <n>               Max snippets to process in this run (default: all)
  --dry-run               Print prompts without running Amp or changing progress
  --mode <mode>           Amp agent mode: smart, deep, large, rush (default: smart)
  --continue-on-error     Skip failed snippets instead of stopping the loop
  --add-language <lang>   Only add a missing language tab (e.g. MoonBit)
  -h, --help              Show this help message

Examples:
  bun run snippets:loop                                 # process all pending
  bun run snippets:loop -- --max 5                      # do 5 at a time
  bun run snippets:loop -- --continue-on-error          # don't stop on failures
  bun run snippets:loop -- --max 3 --dry-run            # preview prompts
  bun run snippets:loop -- --mode deep                  # use deep mode
  bun run snippets:loop -- --golem-repo /path/to/golem  # custom repo path
  bun run snippets:loop -- --add-language MoonBit --mode deep  # add MoonBit to all snippets
`)
}

async function log(msg: string) {
  const line = `[${new Date().toISOString()}] ${msg}`
  console.log(line)
  await appendFile(LOG_FILE, line + "\n")
}

async function getNextPending(): Promise<SnippetRecord | null> {
  if (!existsSync(PROGRESS_FILE)) return null
  const progress: ProgressFile = JSON.parse(await readFile(PROGRESS_FILE, "utf-8"))
  return Object.values(progress.snippets).find(s => s.status === "pending" && !s.removed) ?? null
}

async function generatePrompt(
  snippetId: string,
  golemRepo: string,
  addLanguage?: string
): Promise<string> {
  const cmd = addLanguage ? "prompt-add-lang" : "prompt"
  const args = ["bun", "run", MANAGE_SCRIPT, cmd, snippetId, "--golem-repo", golemRepo]
  if (addLanguage) {
    args.push("--language", addLanguage)
  }
  const proc = Bun.spawn(args, { stdout: "pipe", stderr: "pipe" })
  const output = await new Response(proc.stdout).text()
  await proc.exited
  return output
}

async function runAmp(prompt: string, mode: string): Promise<{ success: boolean; output: string }> {
  return new Promise(resolve => {
    const child = spawn(
      "amp",
      [
        "-x",
        "--dangerously-allow-all",
        "--no-notifications",
        "--no-ide",
        "-m",
        mode,
        "-l",
        "snippet-update",
        "--archive",
      ],
      {
        stdio: ["pipe", "pipe", "pipe"],
        env: process.env,
      }
    )

    let stdout = ""
    let stderr = ""

    child.stdout.on("data", (data: Buffer) => {
      stdout += data.toString()
    })
    child.stderr.on("data", (data: Buffer) => {
      stderr += data.toString()
    })

    // Send prompt via stdin
    child.stdin.write(prompt)
    child.stdin.end()

    child.on("close", (code: number | null) => {
      if (code === 0) {
        resolve({ success: true, output: stdout })
      } else {
        resolve({
          success: false,
          output: stderr || stdout || `exit code ${code}`,
        })
      }
    })

    child.on("error", (err: Error) => {
      resolve({ success: false, output: err.message })
    })
  })
}

async function getPendingCount(): Promise<number> {
  if (!existsSync(PROGRESS_FILE)) return 0
  const progress: ProgressFile = JSON.parse(await readFile(PROGRESS_FILE, "utf-8"))
  return Object.values(progress.snippets).filter(s => s.status === "pending" && !s.removed).length
}

async function getSnippetStatus(id: string): Promise<SnippetRecord | null> {
  if (!existsSync(PROGRESS_FILE)) return null
  const progress: ProgressFile = JSON.parse(await readFile(PROGRESS_FILE, "utf-8"))
  return progress.snippets[id] ?? null
}

async function runManageCmd(...args: string[]): Promise<string> {
  const proc = Bun.spawn(["bun", "run", MANAGE_SCRIPT, ...args], {
    stdout: "pipe",
    stderr: "pipe",
  })
  const output = await new Response(proc.stdout).text()
  await proc.exited
  return output
}

async function main() {
  const opts = parseArgs()
  await log(`=== Snippet update loop started ===`)
  await log(
    `Options: max=${opts.max === Infinity ? "unlimited" : opts.max}, mode=${opts.mode}, dryRun=${opts.dryRun}, golemRepo=${opts.golemRepo}${opts.addLanguage ? `, addLanguage=${opts.addLanguage}` : ""}`
  )

  // Ensure progress file exists
  if (!existsSync(PROGRESS_FILE)) {
    await log("No progress.json found. Running scan first...")
    await runManageCmd("scan")
  }

  // If --add-language, first rescan to pick up current state, then reset completed snippets missing the language
  if (opts.addLanguage) {
    await log(`Rescanning snippets to pick up current state...`)
    await runManageCmd("scan")
    await log(`Resetting completed snippets missing ${opts.addLanguage}...`)
    const resetOutput = await runManageCmd("reset-missing-lang", "--language", opts.addLanguage)
    await log(resetOutput.trim())
  }

  let processed = 0
  let failed = 0

  while (processed < opts.max) {
    const snippet = await getNextPending()
    if (!snippet) {
      await log("No more pending snippets. All done!")
      break
    }

    const remaining = await getPendingCount()
    processed++
    await log(
      `\n--- [${processed}/${opts.max === Infinity ? "∞" : opts.max}] Processing snippet (${remaining} remaining) ---`
    )
    await log(`ID:      ${snippet.id}`)
    await log(`File:    ${snippet.filePath}`)
    await log(`Section: ${snippet.sectionHeading}`)
    await log(`Langs:   ${snippet.languagesPresent.join(", ")}`)

    try {
      // Generate prompt
      const prompt = await generatePrompt(snippet.id, opts.golemRepo, opts.addLanguage)

      if (opts.dryRun) {
        await log("DRY RUN — prompt would be:")
        console.log("\n" + prompt + "\n")
        await log("(not modifying progress in dry-run mode)")
        continue
      }

      // Run Amp via stdin
      await log("Spawning Amp thread...")
      const startTime = Date.now()
      const { success, output } = await runAmp(prompt, opts.mode)
      const elapsed = ((Date.now() - startTime) / 1000).toFixed(1)

      if (success) {
        await log(`Amp completed in ${elapsed}s`)
        // Check if the snippet was marked as completed by Amp
        const updatedSnippet = await getSnippetStatus(snippet.id)
        if (updatedSnippet?.status === "completed") {
          await log(`✅ Snippet marked as completed by Amp`)
        } else {
          await log(
            `⚠️  Amp finished but snippet was NOT marked as completed. Leaving as pending for retry.`
          )
          failed++
          if (!opts.continueOnError) {
            await log("Stopping loop. Use --continue-on-error to skip and continue.")
            break
          }
        }
      } else {
        failed++
        await log(`❌ Amp failed after ${elapsed}s`)
        await log(`Error output: ${output.slice(0, 500)}`)

        if (opts.continueOnError) {
          await log("Skipping this snippet and continuing...")
          await runManageCmd("skip", snippet.id, "--note", `loop-failed: ${output.slice(0, 200)}`)
        } else {
          await log("Stopping loop due to error. Use --continue-on-error to keep going.")
          break
        }
      }
    } catch (e: any) {
      failed++
      await log(`💥 Unexpected error processing ${snippet.id}: ${e?.message ?? e}`)
      if (opts.continueOnError) {
        await log("Continuing to next snippet...")
      } else {
        await log("Stopping loop. Use --continue-on-error to keep going.")
        break
      }
    }
  }

  // Final status
  await log(`\n=== Loop finished ===`)
  await log(`Processed: ${processed}, Failed: ${failed}`)
  const status = await runManageCmd("status")
  await log(status)
}

main().catch(async e => {
  await log(`Fatal error: ${e}`)
  process.exit(1)
})
