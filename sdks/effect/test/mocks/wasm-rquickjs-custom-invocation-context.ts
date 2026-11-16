/**
 * Model of the runtime's custom invocation context cell. A token names an open or closed
 * scope; installing a closed token installs its nearest open ancestor. `__exchanges` counts
 * the host context switches the runtime would perform.
 */
import { __record, type LiveCustomDurableInvocation } from "./golem-durability.js"

export class Token {
  closed = false
  constructor(
    readonly label: string,
    readonly parent: Token | undefined,
  ) {}
}

let root = new Token("root", undefined)
let current = root
let exchanges = 0
let open = new Set<Token>()
let entered: Token[] = []

const effective = (token: Token): Token => {
  let resolved = token
  while (resolved.closed) resolved = resolved.parent ?? root
  return resolved
}

let closedInstalls = 0

const install = (token: Token): void => {
  if (token.closed) closedInstalls++
  const next = effective(token)
  if (next !== current) {
    exchanges++
    current = next
  }
}

const isDescendant = (token: Token, ancestor: Token): boolean => {
  for (let at = token.parent; at !== undefined; at = at.parent) if (at === ancestor) return true
  return false
}

export const capture = (): Token => current

export const swap = (next: Token): Token => {
  const previous = current
  install(next)
  return previous
}

export const run = <A>(token: Token, fn: () => A): A => {
  const previous = swap(token)
  try {
    return fn()
  } finally {
    swap(previous)
  }
}

export const enterLive = (live: LiveCustomDurableInvocation): Token => {
  live.enter()
  const child = new Token(live.functionName, current)
  open.add(child)
  entered.push(child)
  exchanges++
  current = child
  return child
}

export const close = (token: Token): void => {
  if ([...open].some((other) => isDescendant(other, token)))
    throw new Error(`${token.label} has open descendants`)
  token.closed = true
  open.delete(token)
  __record(`close:${token.label}`)
  if (current === token) install(token)
}

/**
 * Models the runtime's promise hook for one host promise: its reactions run in the scope that is
 * current now, when the import is called.
 */
export const __inCurrentScope = <A>(promise: Promise<A>): Promise<A> => {
  const token = current
  const inScope =
    <X, Y>(f: ((x: X) => Y) | null | undefined) =>
    (x: X) =>
      run(token, () => f!(x))
  return {
    then: (onFulfilled, onRejected) =>
      promise.then(
        onFulfilled ? inScope(onFulfilled) : undefined,
        onRejected ? inScope(onRejected) : undefined,
      ),
  } as Promise<A>
}

/** The label of the scope that host calls made now are attributed to. */
export const __current = (): string => current.label
export const __exchanges = (): number => exchanges
/** How often a closed token was installed; the runtime reports each as escaped work. */
export const __closedInstalls = (): number => closedInstalls
/** Every scope entered so far, with the label of the scope it was entered in. */
export const __entered = (): Array<[string, string]> =>
  entered.map((t) => [t.label, t.parent!.label])
export const __open = (): number => open.size
export const __reset = (): void => {
  root = new Token("root", undefined)
  current = root
  exchanges = 0
  closedInstalls = 0
  open = new Set()
  entered = []
}
