import {
  AgentStream,
  defineAgent,
  defineHttpRouter,
  http,
  method,
  withRawHeaders,
} from "@golemcloud/golem-ts-sdk";
import { z } from "zod";
import * as fs from "node:fs";

export const WebRouter = defineHttpRouter("WebRouter")
  .mount("/web")
  .openApi(() => ({
    openapi: "3.1.0",
    info: { title: "Router", version: "1" },
    paths: {
      "/echo": {
        post: {
          operationId: "echo",
          responses: { "200": { description: "Streaming echo" } },
        },
      },
    },
  }))
  .implement((request) => {
    const path = new URL(request.url).pathname;
    if (path.endsWith("/early")) return new Response("early", { status: 202 });
    if (path.endsWith("/head"))
      return new Response(
        new ReadableStream(
          {
            pull() {
              throw new Error("HEAD producer polled");
            },
          },
          { highWaterMark: 0 },
        ),
      );
    return withRawHeaders(new Response(request.body), [
      { name: "set-cookie", value: new TextEncoder().encode("a=first") },
      { name: "set-cookie", value: new TextEncoder().encode("a=second") },
    ]);
  });

export const RawRouter = defineHttpRouter("RawRouter")
  .mount("/raw")
  .implementRaw((request) => ({
    status: 200,
    headers: [],
    body: AgentStream.from([
      new TextEncoder().encode(
        JSON.stringify({
          method: request.method,
          path: request.path,
          query: request.query ?? null,
        }),
      ),
    ]),
  }));

export const StaticRouter = defineHttpRouter("StaticRouter")
  .mount("/static")
  .static("/*", "/assets/$1")
  .implement();
export const ProviderRouter = defineHttpRouter("ProviderRouter")
  .mount("/provider")
  .openApi(() => ({
    openapi: "3.1.0",
    info: { title: "Provider", version: "1" },
    paths: {},
  }))
  .implement();

const Files = defineAgent({
  name: "Files",
  id: { name: z.string() },
  methods: {
    update: method({ input: {}, returns: z.string() }),
  },
  http: http.mount("/files/{name}", {
    exposeFiles: [{ route: "/value", path: "/value.txt" }],
  }),
});
export const FilesImpl = Files.implement({
  init({ id }) {
    fs.writeFileSync("/value.txt", `initial:${id.name}`);
    return {};
  },
  methods: {
    update() {
      fs.writeFileSync("/value.txt", "updated");
      return "updated";
    },
  },
});
