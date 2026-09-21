import { NoParameters, universal } from "@golemcloud/effect-golem/middleware"

universal({
  name: "effect-standalone-audit",
  parameters: NoParameters,
  handler: (invocation, underlying) =>
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin),
})
