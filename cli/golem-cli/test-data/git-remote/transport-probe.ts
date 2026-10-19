// Run in a disposable CLI-test subprocess because released parsers can stall.
import { readFile } from "node:fs/promises";
import { repository } from "../../../../builtin-tools/git/src/git.ts";
import { fetchCommand } from "../../../../builtin-tools/git/src/transfer.ts";
import { pushCommand } from "../../../../builtin-tools/git/src/push.ts";

const [dir, mode, oid, packFile] = process.argv.slice(2);
const repo = await repository([dir!]);
const pkt = (value: Buffer | string) => {
  const bytes = Buffer.from(value);
  return Buffer.concat([
    Buffer.from((bytes.length + 4).toString(16).padStart(4, "0")),
    bytes,
  ]);
};
const push = mode === "open-push";
const service = push ? "git-receive-pack" : "git-upload-pack";
const url = "https://example.org/repo.git";
const send: typeof fetch = async (_input, options) => {
  if (options?.method !== "POST")
    return new Response(
      Buffer.concat([
        pkt(`# service=${service}\n`),
        Buffer.from("0000"),
        pkt(
          `${oid} refs/heads/main\0${push ? "report-status delete-refs side-band-64k" : "multi_ack_detailed ofs-delta"}\n`,
        ),
        Buffer.from("0000"),
      ]),
      { headers: { "content-type": `application/x-${service}-advertisement` } },
    );
  process.stdout.write("READY\n");
  if (push)
    return new Response(
      new ReadableStream({
        start(controller) {
          controller.enqueue(
            Buffer.concat([
              pkt(
                Buffer.concat([Buffer.from([1]), pkt("unpack ok\0invalid\n")]),
              ),
              Buffer.from("0000"),
            ]),
          );
        },
      }),
      { headers: { "content-type": "application/x-git-receive-pack-result" } },
    );
  return new Response(
    Buffer.concat([pkt("NAK\n"), await readFile(packFile!)]),
    {
      headers: { "content-type": "application/x-git-upload-pack-result" },
    },
  );
};
// Keep a pending asynchronous parser observable until the supervising test stops it.
const keepAlive = setInterval(() => {}, 1000);
try {
  if (push) await pushCommand(repo, url, ["main"], {}, send);
  else
    await fetchCommand(
      repo,
      url,
      ["refs/heads/main:refs/remotes/origin/main"],
      {},
      send,
    );
  process.stdout.write("COMPLETE\n");
} finally {
  clearInterval(keepAlive);
}
