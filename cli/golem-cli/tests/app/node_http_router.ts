import http, { createServer, IncomingMessage } from "node:http";
import bareHttp from "http";
import {
  defineHttpRouter,
  nodeHttpHandler,
} from "@golemcloud/golem-ts-sdk/http-router";
import bundled from "./node-http-dependency.cjs";

const server = createServer((req, res) => {
  res.writeHead(200, { "content-type": "text/plain" });
  res.end("Hello from Golem");
});

export const router = defineHttpRouter("Web")
  .mount("/")
  .implementRaw(nodeHttpHandler(server));

const checks = createServer((req, res) => {
  const path = req.url!.split("?")[0];
  if (path === "/checks/imports") {
    const same =
      http.createServer === bareHttp.createServer &&
      http.createServer === bundled.http.createServer &&
      http.createServer === bundled.nodeHttp.createServer &&
      http.get === bareHttp.get &&
      http.get === bundled.http.get &&
      req instanceof IncomingMessage;
    res.end(String(same));
  } else if (path === "/checks/outgoing") {
    const target = new URLSearchParams(req.url!.split("?")[1]).get("url")!;
    const outgoing = http.get(target, (incoming) => {
      void (async () => {
        if (!(incoming instanceof IncomingMessage))
          throw new Error("client identity lost");
        for await (const chunk of incoming) res.write(chunk);
        res.end();
      })().catch((error) => res.destroy(error));
    });
    outgoing.on("error", (error) => res.destroy(error));
  } else if (path === "/checks/echo") {
    res.writeHead(200, { "set-cookie": ["a=first", "a=second"] });
    req.pipe(res);
  } else if (path === "/checks/bodyless") {
    res.statusCode = Number(req.headers["x-status"] ?? 200);
    if (req.headers["x-zero"]) res.setHeader("content-length", "000");
    res.flushHeaders(); // The host must dispose without end() or an output reader.
  } else if (path === "/checks/before-head") {
    res.destroy(new Error("before head"));
  } else if (path === "/checks/after-head") {
    res.flushHeaders();
    req.once("data", () => res.destroy(new Error("after head")));
    req.resume();
  } else if (path === "/checks/disconnect-before") {
    req.resume(); // Await upload EOF without committing a response.
    http
      .get(String(req.headers["x-entered"]), (incoming) => incoming.resume())
      .on("error", (error) => res.destroy(error));
  } else if (path === "/checks/disconnect-after") {
    res.flushHeaders();
    req.pipe(res); // Await more input and output until the host cancels.
  } else {
    res.end("checks");
  }
});

export const checkRouter = defineHttpRouter("Checks")
  .mount("/checks")
  .implementRaw(nodeHttpHandler(checks));
