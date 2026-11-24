import { expect, test } from "bun:test"
import { mkdir, mkdtemp, readFile, rm, utimes, writeFile } from "node:fs/promises"
import { join, resolve } from "node:path"
import { pathToFileURL } from "node:url"

const root = resolve(import.meta.dir, "../..")
const cargo = Bun.which("cargo")!

for (const scenario of ["selected", "wrong source", "features", "profile"] as const) {
  test(`APFS production task: ${scenario}`, async () => {
    const scratch = join(root, ".amp/in/orb-backup/apfs-tests")
    await mkdir(scratch, { recursive: true })
    const dir = await mkdtemp(join(scratch, "case-"))
    try {
      for (const path of ["bin", "integration-tests/scripts/apfs", "golem-worker-executor/src", "target/dev-ci/deps"]) {
        await mkdir(join(dir, path), { recursive: true })
      }
      const flow = await readFile(join(root, "Makefile.main-validation.toml"), "utf8")
      // Run the production Duckscript with real Cargo metadata and cargo-test-r;
      // replace only compilation and the platform-specific runner.
      const apfs = flow.slice(flow.indexOf("[tasks.main-validation-apfs]"))
      await writeFile(join(dir, "Makefile.toml"), apfs)
      await writeFile(
        join(dir, "Cargo.toml"),
        "[workspace]\nmembers=[\"golem-worker-executor\"]\nresolver=\"3\"\n[profile.dev-ci]\ninherits=\"dev\"\n"
      )
      await writeFile(
        join(dir, "golem-worker-executor/Cargo.toml"),
        "[package]\nname=\"golem-worker-executor\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[features]\ntest-utils=[]\n"
      )
      await writeFile(join(dir, "golem-worker-executor/src/lib.rs"), "")
      const lock = Bun.spawnSync([cargo, "generate-lockfile", "--offline"], { cwd: dir })
      expect(lock.exitCode, lock.stderr.toString()).toBe(0)
      const binary = join(dir, "target/dev-ci/deps/native test")
      await writeFile(binary, "#!/bin/sh\nexit 99\n", { mode: 0o755 })
      const decoy = join(dir, "target/dev-ci/deps/golem_worker_executor-stale-decoy")
      await writeFile(decoy, "#!/bin/sh\nexit 98\n", { mode: 0o755 })
      await utimes(decoy, new Date("2038-01-01"), new Date("2038-01-01"))
      const artifact = {
        reason: "compiler-artifact",
        package_id: `path+${pathToFileURL(join(dir, "golem-worker-executor"))}#0.0.0`,
        manifest_path: join(dir, "golem-worker-executor/Cargo.toml"),
        features: ["test-utils"],
        profile: { opt_level: "0", debuginfo: 2, debug_assertions: true, overflow_checks: true, test: true },
        target: {
          name: "golem_worker_executor",
          kind: ["lib"],
          crate_types: ["lib"],
          src_path: join(dir, "golem-worker-executor/src/lib.rs"),
          edition: "2024",
          doctest: true,
          test: true,
          doc: true
        },
        executable: binary,
        filenames: [binary],
        fresh: false
      }
      const messages = [artifact]
      if (scenario === "wrong source") {
        messages.push({ ...artifact, target: { ...artifact.target, src_path: join(dir, "other/src/lib.rs") } })
      } else if (scenario === "features") {
        messages.push({ ...artifact, features: ["test-utils", "extra"] })
      } else if (scenario === "profile") {
        messages.push({ ...artifact, profile: { ...artifact.profile, opt_level: "3" } })
      }
      await writeFile(join(dir, "artifact.json"), messages.map((message) => JSON.stringify(message)).join("\n") + "\n")
      await writeFile(
        join(dir, "bin/cargo"),
        `#!/bin/sh
case "$1" in
  test)
    [ "$*" = "test --locked -j 4 --profile dev-ci -p golem-worker-executor --features test-utils --lib --no-run --message-format=json-render-diagnostics" ] || exit 91
    cat "$APFS_FIXTURE/artifact.json" ;;
  metadata)
    printf "%s\\n" "$*" >> "$APFS_FIXTURE/metadata-invocations"
    exec "$REAL_CARGO" "$@" ;;
  *) exit 93 ;;
esac
`,
        { mode: 0o755 }
      )
      await writeFile(
        join(dir, "integration-tests/scripts/apfs/run-native-tests.sh"),
        "#!/bin/sh\nprintf \"%s\\n\" \"$@\" > \"$APFS_FIXTURE/native-invoked\"\n",
        { mode: 0o755 }
      )
      const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("CARGO_MAKE_")))
      const child = Bun.spawn(
        [cargo, "make", "--disable-check-for-updates", "--no-workspace", "main-validation-apfs"],
        {
          cwd: dir,
          env: {
            ...env,
            PATH: `${dir}/bin:${process.env.PATH}`,
            REAL_CARGO: cargo,
            APFS_FIXTURE: dir,
            MAIN_VALIDATION_REPORT_DIR: join(dir, "reports"),
            CARGO_TARGET_DIR: join(dir, "target")
          },
          stdout: "pipe",
          stderr: "pipe"
        }
      )
      const [code, stdout, stderr] = await Promise.all([
        child.exited,
        new Response(child.stdout).text(),
        new Response(child.stderr).text()
      ])
      const output = stdout + stderr
      expect(await Bun.file(join(dir, "reports/apfs-cargo-metadata.json")).exists(), output).toBe(true)
      expect((await readFile(join(dir, "metadata-invocations"), "utf8")).split("\n"), output).toContain(
        "metadata --locked --no-deps --format-version 1"
      )
      expect(await Bun.file(join(dir, "native-invoked")).exists(), output).toBe(scenario === "selected")
      if (scenario === "selected") {
        expect(code, output).toBe(0)
        expect(await readFile(join(dir, "native-invoked"), "utf8")).toBe([
          binary,
          ":tag:native_apfs",
          "--include-ignored",
          "--test-threads",
          "1",
          "--report-time",
          "--format",
          "ctrf",
          "--logfile",
          join(dir, "reports/ctrf-apfs.json"),
          ""
        ].join("\n"))
      } else {
        expect(code, output).not.toBe(0)
      }
    } finally {
      await rm(dir, { recursive: true, force: true })
    }
  }, 30000)
}
