import { readFileSync } from "node:fs";

const [artifactPath] = process.argv.slice(2);
if (!artifactPath) {
  throw new Error("usage: node check.mjs <linked-main.js>");
}

const source = readFileSync(artifactPath, "utf8");
const linked = await import(
  `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`
);
const guest = linked.golemTool010ToolMiddlewareGuest;

if (!guest) {
  throw new Error("missing golemTool010ToolMiddlewareGuest export");
}
for (const name of [
  "discoverToolMiddlewares",
  "getToolMiddleware",
  "invokeToolMiddleware",
]) {
  if (typeof guest[name] !== "function") {
    throw new Error(`missing middleware guest method: ${name}`);
  }
}
for (const name of ["golemTool010Guest", "golemAgent200Guest", "guest"]) {
  if (name in linked) {
    throw new Error(`pure middleware link unexpectedly exports ${name}`);
  }
}
if (linked.golemTool010ToolMiddlewareGuestLinkRoot !== guest) {
  throw new Error("fixture root does not preserve middleware guest identity");
}
if (linked.toolMiddlewareGuest !== guest) {
  throw new Error("generated wrapper namespace does not preserve middleware guest identity");
}
for (const symbol of ["ToolHostApi", "golem_tool_0_1_0_host"]) {
  if (source.includes(symbol)) {
    throw new Error(`pure middleware link contains ambient tool host reference: ${symbol}`);
  }
}

console.log("middleware guest pure-link exports verified");
