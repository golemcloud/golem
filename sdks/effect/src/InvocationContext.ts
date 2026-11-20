/**
 * The caller of an invocation, and the service that carries it.
 *
 * An agent constructor, a method handler, a tool and a tool middleware read who caused the running
 * invocation from the {@link InvocationContext} service that the dispatcher provides for each call.
 * A {@link Caller}, a {@link Principal} and a {@link TokenProvenance} are live values, not data: they
 * have no schema and cannot be a method input, a result or part of an agent id. Store
 * `caller.original.ref` (a {@link PrincipalRef}) when an identity has to be kept.
 *
 * @since 2.0.0
 */
import { Context, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import { witPrincipalRefAnnotationKey } from "./WitTypes.js"

/**
 * The identity of one principal: an issuer and a subject.
 *
 * A reference is data: holding one grants nothing. Two references name the same principal exactly
 * when both strings are equal, byte for byte. The text form is `("<issuer>","<subject>")` and
 * {@link PrincipalRef.parse} refuses every other spelling.
 *
 * @since 2.0.0
 * @category models
 */
export class PrincipalRef {
  readonly issuer: string
  readonly subject: string

  constructor(issuer: string, subject: string) {
    this.issuer = issuer
    this.subject = subject
  }

  /** Reads the canonical text form `("<issuer>","<subject>")`. Throws on any other spelling. */
  static parse(text: string): PrincipalRef {
    const parser = new TextFormParser(text)
    parser.expect("(")
    const issuer = parser.string()
    parser.expect(",")
    const subject = parser.string()
    parser.expect(")")
    parser.end()
    return new PrincipalRef(issuer, subject)
  }

  /** The canonical text form `("<issuer>","<subject>")`. */
  toString(): string {
    return `(${printString(this.issuer)},${printString(this.subject)})`
  }

  /** Byte equality of both strings. */
  equals(other: PrincipalRef): boolean {
    return this.issuer === other.issuer && this.subject === other.subject
  }

  /** Serializes as the record `{ issuer, subject }`. */
  toJSON(): { readonly issuer: string; readonly subject: string } {
    return { issuer: this.issuer, subject: this.subject }
  }
}

/**
 * Schema for a {@link PrincipalRef} as data: the named record `principal-ref { issuer, subject }`.
 * Unlike a {@link Principal} or a {@link Caller}, a reference can be a method parameter, a result,
 * part of an agent id and a field of stored state.
 *
 * @since 2.0.0
 * @category schemas
 */
export const PrincipalRefSchema: Schema.Schema<PrincipalRef> = Schema.declare(
  (value): value is PrincipalRef => value instanceof PrincipalRef,
).pipe(Schema.annotate({ [witPrincipalRefAnnotationKey]: true }))

/** Prints a string the way RFC 8785 serializes JSON strings. */
const printString = (value: string): string => {
  let out = '"'
  for (const character of value) {
    const code = character.codePointAt(0)!
    switch (character) {
      case '"':
        out += '\\"'
        break
      case "\\":
        out += "\\\\"
        break
      case "\b":
        out += "\\b"
        break
      case "\t":
        out += "\\t"
        break
      case "\n":
        out += "\\n"
        break
      case "\f":
        out += "\\f"
        break
      case "\r":
        out += "\\r"
        break
      default:
        out += code < 0x20 ? `\\u${code.toString(16).padStart(4, "0")}` : character
    }
  }
  return out + '"'
}

class TextFormParser {
  private position = 0
  private readonly characters: ReadonlyArray<string>

  constructor(text: string) {
    this.characters = Array.from(text)
  }

  private fail(message: string): never {
    throw new SyntaxError(
      `invalid principal reference text: ${message} at character ${this.position}`,
    )
  }

  expect(expected: string): void {
    if (this.characters[this.position] !== expected) this.fail(`expected \`${expected}\``)
    this.position += 1
  }

  end(): void {
    if (this.position < this.characters.length) this.fail("trailing text")
  }

  string(): string {
    this.expect('"')
    let value = ""
    for (;;) {
      const character = this.characters[this.position]
      if (character === undefined) this.fail("unterminated string")
      this.position += 1
      if (character === '"') return value
      if (character === "\\") {
        value += this.escape()
      } else if (character.codePointAt(0)! < 0x20) {
        this.fail("unescaped control character")
      } else {
        value += character
      }
    }
  }

  /** Reads the escape after a backslash. Only the escapes that the printer writes are accepted. */
  private escape(): string {
    const character = this.characters[this.position]
    if (character === undefined) this.fail("non-canonical escape")
    this.position += 1
    switch (character) {
      case '"':
        return '"'
      case "\\":
        return "\\"
      case "b":
        return "\b"
      case "t":
        return "\t"
      case "n":
        return "\n"
      case "f":
        return "\f"
      case "r":
        return "\r"
      case "u": {
        const digits = this.characters.slice(this.position, this.position + 4).join("")
        if (!/^[0-9a-f]{4}$/.test(digits)) this.fail("non-canonical escape")
        const code = parseInt(digits, 16)
        // A control character has the short escape when it has one, else `\u00xx`; every other
        // character is written as itself.
        if (code >= 0x20 || [0x08, 0x09, 0x0a, 0x0c, 0x0d].includes(code))
          this.fail("non-canonical escape")
        this.position += 4
        return String.fromCharCode(code)
      }
      default:
        return this.fail("non-canonical escape")
    }
  }
}

/**
 * The five kinds of principal.
 *
 * @since 2.0.0
 * @category models
 */
export type PrincipalKind = "user" | "account" | "agent" | "tool" | "anonymous"

const liveValueSerialization =
  "a Caller, a Principal and a TokenProvenance are live values and cannot be serialized; store ctx.caller.original.ref instead"

abstract class BasePrincipal {
  // Prevents structural compatibility between the kinds.
  protected readonly _brand!: void

  /** A principal is a live value: serializing it throws. Store its `ref` instead. */
  toJSON(): never {
    throw new Error(liveValueSerialization)
  }

  abstract readonly kind: PrincipalKind
  abstract readonly ref: PrincipalRef | undefined

  /** Whether this principal is the one `other` names. An anonymous principal equals nothing. */
  equals(other: Principal | PrincipalRef): boolean {
    const mine = this.ref
    if (mine === undefined) return false
    const theirs = other instanceof PrincipalRef ? other : other.ref
    return theirs !== undefined && mine.equals(theirs)
  }

  /** `"<kind> <ref text>"`, or `"anonymous"`. It never includes claims. */
  toString(): string {
    const ref = this.ref
    return ref === undefined ? "anonymous" : `${this.kind} ${ref.toString()}`
  }
}

const refFromHost = (ref: AgentCommon.PrincipalRef): PrincipalRef =>
  new PrincipalRef(ref.issuer, ref.subject)

/**
 * A person who signed in through an identity provider.
 *
 * @since 2.0.0
 * @category models
 */
export class UserPrincipal extends BasePrincipal {
  readonly kind = "user" as const
  readonly ref: PrincipalRef
  /** The claims of the sign-in, as the exact JSON object text the host recorded. */
  readonly claimsJson: string
  private readonly authenticatedAtWire: AgentCommon.Datetime
  private parsedClaims: Record<string, unknown> | undefined

  constructor(inner: AgentCommon.UserPrincipal) {
    super()
    this.ref = refFromHost(inner.pair)
    this.claimsJson = inner.claims
    this.authenticatedAtWire = inner.authenticatedAt
  }

  get issuer(): string {
    return this.ref.issuer
  }

  get subject(): string {
    return this.ref.subject
  }

  /** The claims, parsed on first use. */
  get claims(): Record<string, unknown> {
    this.parsedClaims ??= JSON.parse(this.claimsJson) as Record<string, unknown>
    return this.parsedClaims
  }

  /** When the user signed in. */
  get authenticatedAt(): Date {
    const { seconds, nanoseconds } = this.authenticatedAtWire
    return new Date(Number(seconds) * 1000 + Math.floor(nanoseconds / 1_000_000))
  }
}

/**
 * A Golem account, as seen through the CLI, the REST API or a token.
 *
 * @since 2.0.0
 * @category models
 */
export class AccountPrincipal extends BasePrincipal {
  readonly kind = "account" as const
  readonly ref: PrincipalRef
  readonly accountId: AgentCommon.AccountId
  readonly authentication: AgentCommon.AccountAuthentication

  constructor(inner: AgentCommon.AccountPrincipal) {
    super()
    this.ref = refFromHost(inner.pair)
    this.accountId = inner.accountId
    this.authentication = inner.authentication
  }
}

/**
 * A running agent that made a call.
 *
 * @since 2.0.0
 * @category models
 */
export class AgentPrincipal extends BasePrincipal {
  readonly kind = "agent" as const
  readonly ref: PrincipalRef
  readonly agentId: AgentCommon.AgentId
  readonly agentType: string
  readonly environmentId: AgentCommon.EnvironmentId

  constructor(inner: AgentCommon.AgentPrincipal) {
    super()
    this.ref = refFromHost(inner.pair)
    this.agentId = inner.agentId
    this.agentType = inner.agentType
    this.environmentId = inner.environmentId
  }
}

/**
 * A tool that made a call.
 *
 * @since 2.0.0
 * @category models
 */
export class ToolPrincipal extends BasePrincipal {
  readonly kind = "tool" as const
  readonly ref: PrincipalRef
  readonly environmentId: AgentCommon.EnvironmentId
  readonly toolName: string

  constructor(inner: AgentCommon.ToolPrincipal) {
    super()
    this.ref = refFromHost(inner.pair)
    this.environmentId = inner.environmentId
    this.toolName = inner.toolName
  }
}

/**
 * Nobody: an unauthenticated caller. It has no reference and equals nothing.
 *
 * @since 2.0.0
 * @category models
 */
export class AnonymousPrincipal extends BasePrincipal {
  readonly kind = "anonymous" as const
  readonly ref = undefined
}

/**
 * One side of a call: a user, an account, an agent, a tool or nobody.
 *
 * A principal is a live value, not data. Store `principal.ref` instead.
 *
 * @since 2.0.0
 * @category models
 */
export type Principal =
  | UserPrincipal
  | AccountPrincipal
  | AgentPrincipal
  | ToolPrincipal
  | AnonymousPrincipal

/**
 * Who made a token and which token it is. The id is the token's own, never its secret.
 *
 * @since 2.0.0
 * @category models
 */
export class TokenProvenance {
  // Prevents structural compatibility with plain records.
  protected readonly _brand!: void

  readonly id: AgentCommon.Uuid
  readonly maker: PrincipalRef | undefined

  constructor(inner: AgentCommon.TokenProvenance) {
    this.id = inner.id
    this.maker = inner.maker === undefined ? undefined : refFromHost(inner.maker)
  }

  /** A token provenance is a live value: serializing it throws. */
  toJSON(): never {
    throw new Error(liveValueSerialization)
  }
}

/**
 * Who caused the running invocation: the `original` principal that started the chain, the
 * `immediate` principal that made this call, and the provenance of the tokens on the way.
 *
 * @since 2.0.0
 * @category models
 */
export class Caller {
  // Prevents structural compatibility with plain records.
  protected readonly _brand!: void

  /** A caller is a live value: serializing it throws. Store `original.ref` instead. */
  toJSON(): never {
    throw new Error(liveValueSerialization)
  }

  readonly original: Principal
  readonly immediate: Principal
  readonly tokens: ReadonlyArray<TokenProvenance>

  constructor(
    original: Principal,
    immediate: Principal,
    tokens: ReadonlyArray<TokenProvenance> = [],
  ) {
    this.original = original
    this.immediate = immediate
    this.tokens = tokens
  }

  /** A caller with no identity: anonymous in both roles and no tokens. */
  static anonymous(): Caller {
    const anonymous = new AnonymousPrincipal()
    return new Caller(anonymous, anonymous)
  }
}

/**
 * What the host tells an invocation about itself.
 *
 * @since 2.0.0
 * @category models
 */
export interface InvocationContextShape {
  /** Who caused the running invocation. Read from the host on first use. */
  readonly caller: Caller
}

/**
 * Effect service exposing the {@link Caller} of the running invocation.
 *
 * The dispatcher provides this service for every constructor, method, snapshot, tool and tool
 * middleware call, read-only handlers included, so depending on it never leaks into the public
 * `R` slot of a method signature. The host is asked for the caller on first use only. In tests that
 * bypass the dispatcher, provide it with `Effect.provideService(InvocationContext, { caller })`.
 *
 * **Example**
 *
 * ```ts
 * whoCalled: () =>
 *   Effect.gen(function* () {
 *     const context = yield* InvocationContext.InvocationContext
 *     return context.caller.immediate.toString()
 *   })
 * ```
 *
 * @since 2.0.0
 * @category host services
 */
export class InvocationContext extends Context.Service<InvocationContext, InvocationContextShape>()(
  "effect-golem/InvocationContext",
) {}
