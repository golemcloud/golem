import { NoParameters, universal } from "@golemcloud/effect-golem/middleware"
import { toolMiddlewareGuest } from "@golemcloud/effect-golem"

universal({
  name: "effect-standalone-audit",
  parameters: NoParameters,
  handler: (invocation, underlying) =>
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin),
})

const discovered = toolMiddlewareGuest.discoverToolMiddlewares()
if (
  discovered.length !== 1 ||
  discovered[0]?.name !== "effect-standalone-audit" ||
  discovered[0]?.scope.tag !== "universal"
) {
  throw new Error("embedded middleware guest did not discover the subpath registration")
}
