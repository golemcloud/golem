/**
 * In-memory mock for `golem:api/context@1.5.0`. Maintains a stack of
 * spans so tests can simulate the host's invocation-context model:
 * `startSpan(name)` becomes the new current span (a child of the
 * previous one), and `Span.finish()` pops it back off the stack.
 *
 * Each span gets a deterministic id so assertions can rely on stable
 * trace/span-id ordering.
 */

export type AttributeValue = { tag: "string"; val: string }

export interface Attribute {
  readonly key: string
  readonly value: AttributeValue
}

export interface AttributeChain {
  readonly key: string
  readonly values: AttributeValue[]
}

export type TraceId = string
export type SpanId = string

export type Datetime = { seconds: bigint; nanoseconds: number }

let nextSpanId = 1
let nextTraceId = 1
let traceContextHeaderForwarding = false

interface MockSpanState {
  readonly traceId: string
  readonly spanId: string
  readonly name: string
  readonly parent: MockSpanState | null
  readonly attributes: Map<string, AttributeValue>
  finished: boolean
}

const stack: Array<MockSpanState> = []

const top = (): MockSpanState | null => (stack.length === 0 ? null : stack[stack.length - 1]!)

export class Span {
  /** @internal */
  readonly state: MockSpanState

  constructor(state: MockSpanState) {
    this.state = state
  }

  startedAt(): Datetime {
    return { seconds: 0n, nanoseconds: 0 }
  }

  setAttribute(name: string, value: AttributeValue): void {
    if (this.state.finished) {
      throw new Error(`setAttribute after finish on span '${this.state.name}'`)
    }
    this.state.attributes.set(name, value)
  }

  setAttributes(attributes: Attribute[]): void {
    for (const a of attributes) {
      this.setAttribute(a.key, a.value)
    }
  }

  finish(): void {
    if (this.state.finished) return
    this.state.finished = true
    const idx = stack.lastIndexOf(this.state)
    if (idx >= 0) stack.splice(idx, 1)
  }
}

export class InvocationContext {
  /** @internal */
  readonly state: MockSpanState | null

  constructor(state: MockSpanState | null) {
    this.state = state
  }

  traceId(): TraceId {
    return this.state?.traceId ?? "00000000000000000000000000000000"
  }

  spanId(): SpanId {
    return this.state?.spanId ?? "0000000000000000"
  }

  parent(): InvocationContext | undefined {
    if (!this.state || !this.state.parent) return undefined
    return new InvocationContext(this.state.parent)
  }

  getAttribute(key: string, inherited: boolean): AttributeValue | undefined {
    let cur: MockSpanState | null = this.state
    while (cur !== null) {
      const v = cur.attributes.get(key)
      if (v !== undefined) return v
      if (!inherited) return undefined
      cur = cur.parent
    }
    return undefined
  }

  getAttributes(inherited: boolean): Attribute[] {
    const out: Attribute[] = []
    if (!this.state) return out
    if (!inherited) {
      for (const [k, v] of this.state.attributes) out.push({ key: k, value: v })
      return out
    }
    const seen = new Set<string>()
    let cur: MockSpanState | null = this.state
    while (cur !== null) {
      for (const [k, v] of cur.attributes) {
        if (!seen.has(k)) {
          seen.add(k)
          out.push({ key: k, value: v })
        }
      }
      cur = cur.parent
    }
    return out
  }

  getAttributeChain(key: string): AttributeValue[] {
    const out: AttributeValue[] = []
    let cur: MockSpanState | null = this.state
    while (cur !== null) {
      const v = cur.attributes.get(key)
      if (v !== undefined) out.push(v)
      cur = cur.parent
    }
    return out
  }

  getAttributeChains(): AttributeChain[] {
    const map = new Map<string, AttributeValue[]>()
    let cur: MockSpanState | null = this.state
    while (cur !== null) {
      for (const [k, v] of cur.attributes) {
        const arr = map.get(k) ?? []
        arr.push(v)
        map.set(k, arr)
      }
      cur = cur.parent
    }
    return Array.from(map.entries()).map(([k, vs]) => ({ key: k, values: vs }))
  }

  traceContextHeaders(): [string, string][] {
    return [["traceparent", `00-${this.traceId()}-${this.spanId()}-01`]]
  }
}

const padHex = (n: number, len: number): string => n.toString(16).padStart(len, "0")

export const startSpan = (name: string): Span => {
  const parent = top()
  const traceId = parent === null ? padHex(nextTraceId++, 32) : parent.traceId
  const spanId = padHex(nextSpanId++, 16)
  const state: MockSpanState = {
    traceId,
    spanId,
    name,
    parent,
    attributes: new Map(),
    finished: false,
  }
  stack.push(state)
  return new Span(state)
}

export const currentContext = (): InvocationContext => new InvocationContext(top())

export const allowForwardingTraceContextHeaders = (allow: boolean): boolean => {
  const previous = traceContextHeaderForwarding
  traceContextHeaderForwarding = allow
  return previous
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

export const __reset = (): void => {
  nextSpanId = 1
  nextTraceId = 1
  stack.length = 0
  traceContextHeaderForwarding = false
}

/** Pre-seed an outer host span (e.g. simulating an in-flight invocation context). */
export const __pushSpan = (name: string): Span => startSpan(name)

export const __getStack = (): ReadonlyArray<{ name: string; spanId: string; traceId: string }> =>
  stack.map((s) => ({ name: s.name, spanId: s.spanId, traceId: s.traceId }))

export const __getAttributesOf = (span: Span): ReadonlyArray<Attribute> =>
  Array.from(span.state.attributes.entries()).map(([k, v]) => ({ key: k, value: v }))

export const __isFinished = (span: Span): boolean => span.state.finished

export const __getForwardingFlag = (): boolean => traceContextHeaderForwarding
