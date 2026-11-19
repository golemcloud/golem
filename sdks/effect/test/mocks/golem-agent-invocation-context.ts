/**
 * In-memory mock for `golem:agent/invocation-context@2.0.0`.
 *
 * Tests call `__setCaller` to choose the principals that `caller` reports for the next
 * agent, tool, or middleware invocation. The default caller is anonymous.
 */
const anonymous = { tag: "anonymous" }
let current: { original: unknown; immediate: unknown; tokens: Array<unknown> } = {
  original: anonymous,
  immediate: anonymous,
  tokens: [],
}

let reads = 0

export const caller = () => {
  reads += 1
  return current
}

/** How many times the guest asked the host for the caller. */
export const callerReads = (): number => reads

export const invocationId = () => undefined

export const __setCaller = (
  immediate: unknown,
  original: unknown = immediate,
  tokens: Array<unknown> = [],
): void => {
  current = { original, immediate, tokens }
}

export const __resetCaller = (): void => {
  current = { original: anonymous, immediate: anonymous, tokens: [] }
}

/** Report `immediate` as the caller, then start `run`. */
export const withCaller = <A>(immediate: unknown, run: () => A): A => {
  __setCaller(immediate)
  return run()
}
