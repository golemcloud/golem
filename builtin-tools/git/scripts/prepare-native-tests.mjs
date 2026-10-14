import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";

if (process.platform === "win32") process.exit(0);
const root = path.resolve(import.meta.dirname, "../../..");
const prefix = path.join(root, "tmp/git-reference-2.55.0");
const git = path.join(prefix, "bin/git");
function exec(command, args, cwd = root) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.error || result.status !== 0)
    throw result.error ?? new Error(`${command} exited ${result.status}`);
}
if (
  spawnSync(git, ["--version"], { encoding: "utf8" }).stdout?.trim() !==
  "git version 2.55.0"
) {
  await mkdir(prefix, { recursive: true });
  const archive = path.join(prefix, "git-2.55.0.tar.xz");
  let bytes = await readFile(archive).catch(() => undefined);
  if (!bytes) {
    const response = await fetch(
      "https://www.kernel.org/pub/software/scm/git/git-2.55.0.tar.xz",
    );
    if (!response.ok)
      throw new Error(`Git reference download failed: ${response.status}`);
    bytes = Buffer.from(await response.arrayBuffer());
  }
  if (
    createHash("sha256").update(bytes).digest("hex") !==
    "457fdb04dc8728e007d4688695e6912e6f680727920f2a40bf11eacc17505357"
  )
    throw new Error("Git reference source checksum mismatch");
  await writeFile(archive, bytes);
  exec("tar", ["-xJf", archive, "-C", prefix]);
  exec(
    "make",
    [
      "-j2",
      `prefix=${prefix}`,
      "PROFILE=",
      "NO_RUST=1",
      "NO_GETTEXT=1",
      "NO_TCLTK=1",
      "install",
    ],
    path.join(prefix, "git-2.55.0"),
  );
}
exec(git, ["--version"]);
