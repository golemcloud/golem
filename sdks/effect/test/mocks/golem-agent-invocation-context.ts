/**
 * In-memory mock for `golem:agent/invocation-context@2.0.0`.
 *
 * Tests call `__setCaller` to choose the principals that `caller` reports for the next
 * agent, tool, or middleware invocation. The default caller is anonymous.
 */
const anonymous = { tag: "anonymous" }
let current: { original: unknown; immediate: unknown } = {
  original: anonymous,
  immediate: anonymous,
}

export const caller = () => current

export const invocationId = () => undefined

export const __setCaller = (immediate: unknown, original: unknown = immediate): void => {
  current = { original, immediate }
}

export const __resetCaller = (): void => {
  current = { original: anonymous, immediate: anonymous }
}

/** Report `immediate` as the caller, then start `run`. */
export const withCaller = <A>(immediate: unknown, run: () => A): A => {
  __setCaller(immediate)
  return run()
}
