import { universal } from "@golemcloud/effect-golem/middleware"

universal({
  name: "effect-standalone-audit",
  handler: (invocation, underlying) =>
    underlying.invoke(invocation.commandPath, invocation.input, invocation.stdin),
})
