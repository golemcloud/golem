// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { sdkOverflowControl } from "golem-tool-process-sdk-control";
import { runJavaScript } from "wasm-rquickjs:execution";
import {
  getReflectedToolType,
  ok,
  processExitStatusSchema,
  toolDefinition,
  z,
} from "golem-tool-process-sdk-control";

export {
  golemTool010Guest,
  golemTool010Guest as guest,
} from "golem-tool-process-sdk-control";
export {
  golemAgent200Guest,
  toolMiddlewareGuest,
  toolMiddlewareGuest as golemTool010ToolMiddlewareGuest,
  saveSnapshot,
  loadSnapshot,
} from "golem-tool-process-sdk-control";

toolDefinition("node-tool-probe", { requiresFilesystem: true })
  .version("0.0.1")
  .body((body) =>
    body
      .option("cwd", z.string(), { default: "/workspace" })
      .tail("args", z.string(), { separator: "--", verbatim: true })
      .stdout({ required: true })
      .stderr({ required: true })
      .returns(processExitStatusSchema),
  )
  .implement({
    async "node-tool-probe"(input, context) {
      const [mode, effectUrl, execution = "all"] = input.args;
      let value;
      if (execution === "fresh-sdk-overflow") {
        if (effectUrl) {
          const response = await fetch(`${effectUrl}/${mode}-main`, {
            method: "POST",
          });
          await response.arrayBuffer();
        }
        value = await sdkOverflowControl();
        const job = await runJavaScript({
          source: `
            const { sdkOverflowControl } = await import('golem-tool-process-sdk-control');
            if (${JSON.stringify(effectUrl)}) {
              const response = await fetch(${JSON.stringify(effectUrl)} + '/' + ${JSON.stringify(mode)} + '-job', { method: 'POST' });
              await response.arrayBuffer();
            }
            return await sdkOverflowControl();
          `,
          cwd: "/",
          timeoutMs: 10000,
          maxBytes: 4096,
        });
        if (job.value !== value || job.stdout || job.stderr || job.overflowed)
          throw new Error("SDK-only fresh runtime differs");
      } else {
        if (execution !== "all" || mode !== "streams-collect")
          throw new Error("Unsupported SDK-only control");
        const call = getReflectedToolType("streaming")
          .client.command(["dual-reconstruct"])
          .startJson({ mode: "redaction-terminals" });
        const result = await call.collect();
        if (
          [result.result, result.stdout, result.stderr].some(
            (part) => part.status !== "fulfilled",
          )
        )
          throw new Error("SDK-only follow-up failed");
        const sum = (bytes) => [...bytes].reduce((a, b) => a + b, 0);
        value = `${result.stdout.value.length}:${sum(result.stdout.value)}:${result.stderr.value.length}:${sum(result.stderr.value)}`;
      }
      const label = execution === "all" ? mode : `${mode}[${execution}]`;
      const writer = context.stdout.getWriter();
      try {
        await writer.write(new TextEncoder().encode(`${label}:${value}\n`));
      } finally {
        writer.releaseLock();
      }
      return ok({ exit_code: 0 });
    },
  });
