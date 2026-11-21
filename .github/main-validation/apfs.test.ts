import { expect, test } from "bun:test"
import { copyFile, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"

const root = resolve(import.meta.dir, "../..")
const cargo = Bun.which("cargo")!

test("APFS production task reaches the native runner with its selected executable", async () => {
  const scratch = join(root, ".amp/in/orb-backup/apfs-tests")
  await mkdir(scratch, { recursive: true })
  const dir = await mkdtemp(join(scratch, "case-"))
  try {
    for (
      const path of [
        "bin",
        "src",
        ".github/main-validation",
        "integration-tests/scripts/apfs",
        "golem-worker-executor/src",
        "golem-worker-executor-tests/tests"
      ]
    ) {
      await mkdir(join(dir, path), { recursive: true })
    }
    const file = "Makefile.main-validation.toml"
    const flow = await readFile(join(root, file), "utf8")
    // Execute the production Duckscript unchanged, only replacing the expensive
    // compiler and the platform-specific native test runner with local sinks.
    const apfs = flow.slice(flow.indexOf("[tasks.main-validation-apfs]"))
    await writeFile(join(dir, "Makefile.toml"), apfs)
    await writeFile(join(dir, "Cargo.toml"), "[package]\nname=\"apfs-fixture\"\nversion=\"0.0.0\"\n[workspace]\n")
    await writeFile(join(dir, "src/lib.rs"), "")
    await writeFile(join(dir, "golem-worker-executor/src/lib.rs"), "")
    await writeFile(join(dir, "golem-worker-executor-tests/tests/lib.rs"), "")
    await writeFile(join(dir, "native-test"), "#!/bin/sh\nexit 0\n", { mode: 0o755 })
    await writeFile(
      join(dir, "artifact.json"),
      JSON.stringify({
        reason: "compiler-artifact",
        profile: { test: true },
        target: {
          name: "golem_worker_executor",
          kind: ["lib"],
          src_path: join(dir, "golem-worker-executor/src/lib.rs")
        },
        executable: join(dir, "native-test")
      }) + "\n"
    )
    await writeFile(join(dir, "bin/cargo"), "#!/bin/sh\ncat \"$APFS_FIXTURE/artifact.json\"\n", { mode: 0o755 })
    await writeFile(
      join(dir, "integration-tests/scripts/apfs/run-native-tests.sh"),
      "#!/bin/sh\nprintf \"%s\\n\" \"$1\" > \"$APFS_FIXTURE/native-invoked\"\n",
      { mode: 0o755 }
    )
    for (const name of ["select-test-executables.ts", "select-test-executables.test.ts", "tooling-io.ts"]) {
      const path = `.github/main-validation/${name}`
      await copyFile(join(root, path), join(dir, path))
    }
    await symlink(join(import.meta.dir, "node_modules"), join(dir, ".github/main-validation/node_modules"))
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("CARGO_MAKE_")))
    const child = Bun.spawn([cargo, "make", "--disable-check-for-updates", "--no-workspace", "main-validation-apfs"], {
      cwd: dir,
      env: {
        ...env,
        PATH: `${dir}/bin:${process.env.PATH}`,
        APFS_FIXTURE: dir,
        MAIN_VALIDATION_REPORT_DIR: join(dir, "reports"),
        CARGO_TARGET_DIR: join(dir, "target")
      },
      stdout: "pipe",
      stderr: "pipe"
    })
    const [code, stdout, stderr] = await Promise.all([
      child.exited,
      new Response(child.stdout).text(),
      new Response(child.stderr).text()
    ])
    expect(await Bun.file(join(dir, "native-invoked")).exists(), stdout + stderr).toBe(true)
    expect(code, stdout + stderr).toBe(0)
    expect((await readFile(join(dir, "native-invoked"), "utf8")).trim()).toBe(join(dir, "native-test"))
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
}, 30000)
