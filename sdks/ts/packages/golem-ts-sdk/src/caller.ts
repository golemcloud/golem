// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import {
  Caller as HostCaller,
  Principal as HostPrincipal,
  PrincipalRef as HostPrincipalRef,
  TokenProvenance as HostTokenProvenance,
} from 'golem:agent/common@2.0.0';
import { caller as hostCaller } from 'golem:agent/invocation-context@2.0.0';
import { AccountId, EnvironmentId } from './ids';
import { Uuid } from './uuid';

/**
 * Who caused the running invocation.
 *
 * An agent method handler, a constructor, a tool or a tool middleware reads its caller from the
 * {@link InvocationContext} that the SDK passes in. A {@link Caller} is a live value, not data:
 * it has no schema and cannot be a method input or result. Store
 * `ctx.caller.original.ref` (a {@link PrincipalRef}) instead.
 */

/**
 * The identity of one principal: an issuer and a subject.
 *
 * A reference is data: holding one grants nothing. Two references name the same principal
 * exactly when both strings are equal, byte for byte. The text form is
 * `("<issuer>","<subject>")` and {@link PrincipalRef.parse} refuses every other spelling.
 */
export class PrincipalRef {
  readonly issuer: string;
  readonly subject: string;

  constructor(issuer: string, subject: string) {
    this.issuer = issuer;
    this.subject = subject;
  }

  /** Reads the canonical text form `("<issuer>","<subject>")`. Throws on any other spelling. */
  static parse(text: string): PrincipalRef {
    const parser = new TextFormParser(text);
    parser.expect('(');
    const issuer = parser.string();
    parser.expect(',');
    const subject = parser.string();
    parser.expect(')');
    parser.end();
    return new PrincipalRef(issuer, subject);
  }

  /** The canonical text form `("<issuer>","<subject>")`. */
  toString(): string {
    return `(${printString(this.issuer)},${printString(this.subject)})`;
  }

  /** Byte equality of both strings. */
  equals(other: PrincipalRef): boolean {
    return this.issuer === other.issuer && this.subject === other.subject;
  }

  /** Serializes as the record `{ issuer, subject }`. */
  toJSON(): { issuer: string; subject: string } {
    return { issuer: this.issuer, subject: this.subject };
  }
}

/** Prints a string the way RFC 8785 serializes JSON strings. */
function printString(value: string): string {
  let out = '"';
  for (const character of value) {
    const code = character.codePointAt(0)!;
    switch (character) {
      case '"':
        out += '\\"';
        break;
      case '\\':
        out += '\\\\';
        break;
      case '\b':
        out += '\\b';
        break;
      case '\t':
        out += '\\t';
        break;
      case '\n':
        out += '\\n';
        break;
      case '\f':
        out += '\\f';
        break;
      case '\r':
        out += '\\r';
        break;
      default:
        out += code < 0x20 ? `\\u${code.toString(16).padStart(4, '0')}` : character;
    }
  }
  return out + '"';
}

class TextFormParser {
  private position = 0;
  private readonly characters: string[];

  constructor(private readonly text: string) {
    this.characters = Array.from(text);
  }

  private fail(message: string): never {
    throw new SyntaxError(
      `invalid principal reference text: ${message} at character ${this.position}`,
    );
  }

  expect(expected: string): void {
    if (this.characters[this.position] !== expected) this.fail(`expected \`${expected}\``);
    this.position += 1;
  }

  end(): void {
    if (this.position < this.characters.length) this.fail('trailing text');
  }

  string(): string {
    this.expect('"');
    let value = '';
    for (;;) {
      const character = this.characters[this.position];
      if (character === undefined) this.fail('unterminated string');
      this.position += 1;
      if (character === '"') return value;
      if (character === '\\') {
        value += this.escape();
      } else if (character.codePointAt(0)! < 0x20) {
        this.fail('unescaped control character');
      } else {
        value += character;
      }
    }
  }

  /** Reads the escape after a backslash. Only the escapes that the printer writes are accepted. */
  private escape(): string {
    const character = this.characters[this.position];
    if (character === undefined) this.fail('non-canonical escape');
    this.position += 1;
    switch (character) {
      case '"':
        return '"';
      case '\\':
        return '\\';
      case 'b':
        return '\b';
      case 't':
        return '\t';
      case 'n':
        return '\n';
      case 'f':
        return '\f';
      case 'r':
        return '\r';
      case 'u': {
        const digits = this.characters.slice(this.position, this.position + 4).join('');
        if (!/^[0-9a-f]{4}$/.test(digits)) this.fail('non-canonical escape');
        const code = parseInt(digits, 16);
        // A control character has the short escape when it has one, else `\u00xx`; every
        // other character is written as itself.
        if (code >= 0x20 || [0x08, 0x09, 0x0a, 0x0c, 0x0d].includes(code))
          this.fail('non-canonical escape');
        this.position += 4;
        return String.fromCharCode(code);
      }
      default:
        return this.fail('non-canonical escape');
    }
  }
}

export type PrincipalKind = 'user' | 'account' | 'agent' | 'tool' | 'anonymous';

abstract class BasePrincipal {
  // Prevents structural compatibility between the kinds.
  private readonly _brand!: void;

  abstract readonly kind: PrincipalKind;
  abstract readonly ref: PrincipalRef | undefined;

  /** Whether this principal is the one `other` names. An anonymous principal equals nothing. */
  equals(other: Principal | PrincipalRef): boolean {
    const mine = this.ref;
    if (mine === undefined) return false;
    const theirs = other instanceof PrincipalRef ? other : other.ref;
    return theirs !== undefined && mine.equals(theirs);
  }

  /** `"<kind> <ref text>"`, or `"anonymous"`. It never includes claims. */
  toString(): string {
    const ref = this.ref;
    return ref === undefined ? 'anonymous' : `${this.kind} ${ref.toString()}`;
  }
}

/** A person who signed in through an identity provider. */
export class UserPrincipal extends BasePrincipal {
  readonly kind = 'user' as const;
  readonly ref: PrincipalRef;
  /** The claims of the sign-in, as the exact JSON object text the host recorded. */
  readonly claimsJson: string;
  private readonly authenticatedAtWire: { seconds: bigint; nanoseconds: number };
  private parsedClaims: Record<string, unknown> | undefined;

  constructor(inner: Extract<HostPrincipal, { tag: 'user' }>['val']) {
    super();
    this.ref = fromHostRef(inner.pair);
    this.claimsJson = inner.claims;
    this.authenticatedAtWire = inner.authenticatedAt;
  }

  get issuer(): string {
    return this.ref.issuer;
  }

  get subject(): string {
    return this.ref.subject;
  }

  /** The claims, parsed on first use. */
  get claims(): Record<string, unknown> {
    this.parsedClaims ??= JSON.parse(this.claimsJson) as Record<string, unknown>;
    return this.parsedClaims;
  }

  /** When the user signed in. */
  get authenticatedAt(): Date {
    const { seconds, nanoseconds } = this.authenticatedAtWire;
    return new Date(Number(seconds) * 1000 + Math.floor(nanoseconds / 1_000_000));
  }
}

export type AccountAuthentication = 'api-token' | 'bootstrap-token' | 'sign-in';

/** A Golem account, as seen through the CLI, the REST API or a token. */
export class AccountPrincipal extends BasePrincipal {
  readonly kind = 'account' as const;
  readonly ref: PrincipalRef;
  readonly accountId: AccountId;
  readonly authentication: AccountAuthentication;

  constructor(inner: Extract<HostPrincipal, { tag: 'account' }>['val']) {
    super();
    this.ref = fromHostRef(inner.pair);
    this.accountId = AccountId.from(inner.accountId);
    this.authentication = inner.authentication;
  }
}

/** A running agent that made a call. */
export class AgentPrincipal extends BasePrincipal {
  readonly kind = 'agent' as const;
  readonly ref: PrincipalRef;
  readonly agentId: Extract<HostPrincipal, { tag: 'agent' }>['val']['agentId'];
  readonly agentType: string;
  readonly environmentId: EnvironmentId;

  constructor(inner: Extract<HostPrincipal, { tag: 'agent' }>['val']) {
    super();
    this.ref = fromHostRef(inner.pair);
    this.agentId = inner.agentId;
    this.agentType = inner.agentType;
    this.environmentId = EnvironmentId.from(inner.environmentId);
  }
}

/** A tool that made a call. */
export class ToolPrincipal extends BasePrincipal {
  readonly kind = 'tool' as const;
  readonly ref: PrincipalRef;
  readonly environmentId: EnvironmentId;
  readonly toolName: string;

  constructor(inner: Extract<HostPrincipal, { tag: 'tool' }>['val']) {
    super();
    this.ref = fromHostRef(inner.pair);
    this.environmentId = EnvironmentId.from(inner.environmentId);
    this.toolName = inner.toolName;
  }
}

/** Nobody: an unauthenticated caller. It has no reference and equals nothing. */
export class AnonymousPrincipal extends BasePrincipal {
  readonly kind = 'anonymous' as const;
  readonly ref = undefined;
}

/**
 * One side of a call: a user, an account, an agent, a tool or nobody.
 *
 * A principal is a live value, not data. Store `principal.ref` instead.
 */
export type Principal =
  | UserPrincipal
  | AccountPrincipal
  | AgentPrincipal
  | ToolPrincipal
  | AnonymousPrincipal;

/** Who made a token and which token it is. The id is the token's own, never its secret. */
export class TokenProvenance {
  // Prevents structural compatibility with plain records.
  private readonly _brand!: void;

  readonly id: Uuid;
  readonly maker: PrincipalRef | undefined;

  constructor(inner: HostTokenProvenance) {
    this.id = Uuid.from(inner.id);
    this.maker = inner.maker === undefined ? undefined : fromHostRef(inner.maker);
  }
}

/**
 * Who caused the running invocation: the `original` principal that started the chain, the
 * `immediate` principal that made this call, and the provenance of the tokens on the way.
 */
export class Caller {
  // Prevents structural compatibility with plain records.
  private readonly _brand!: void;

  readonly original: Principal;
  readonly immediate: Principal;
  readonly tokens: readonly TokenProvenance[];

  constructor(original: Principal, immediate: Principal, tokens: readonly TokenProvenance[] = []) {
    this.original = original;
    this.immediate = immediate;
    this.tokens = tokens;
  }

  /** A caller with no identity: anonymous in both roles and no tokens. */
  static anonymous(): Caller {
    const anonymous = new AnonymousPrincipal();
    return new Caller(anonymous, anonymous);
  }
}

/**
 * What the host tells an invocation about itself. The SDK passes it as the second argument of
 * every agent method handler, in the `ctx` of a constructor, and in the context of tools and
 * tool middleware.
 */
export interface InvocationContext {
  /** Who caused the running invocation. Read from the host on first use, during the call: a context whose call has ended refuses a first read. */
  readonly caller: Caller;
}

function fromHostRef(ref: HostPrincipalRef): PrincipalRef {
  return new PrincipalRef(ref.issuer, ref.subject);
}

function fromHostPrincipal(principal: HostPrincipal): Principal {
  switch (principal.tag) {
    case 'user':
      return new UserPrincipal(principal.val);
    case 'account':
      return new AccountPrincipal(principal.val);
    case 'agent':
      return new AgentPrincipal(principal.val);
    case 'tool':
      return new ToolPrincipal(principal.val);
    case 'anonymous':
      return new AnonymousPrincipal();
  }
}

/** Wraps the host's answer to `golem:agent/invocation-context.caller`. */
export function callerFromHost(host: HostCaller): Caller {
  return new Caller(
    fromHostPrincipal(host.original),
    fromHostPrincipal(host.immediate),
    host.tokens.map((token) => new TokenProvenance(token)),
  );
}

/** An invocation context that the SDK ends when its call returns. */
export interface EndableInvocationContext extends InvocationContext {
  /**
   * Marks the call as over. A first read of the caller after this throws, unless the call
   * returned a stream: its producer runs after the return, so the caller is read from the host
   * now, while the call still holds it, and kept for the producer.
   */
  end(returnedStream?: boolean): void;
}

/**
 * The context of the running invocation. The host is asked for the caller on first use, and the
 * answer is kept for the rest of the invocation. Once the call has ended the host holds another
 * invocation's caller, so a context that has not read it by then refuses to, and a context that
 * has keeps the value it read.
 */
export function currentInvocationContext(): EndableInvocationContext {
  let caller: Caller | undefined;
  let ended = false;
  return {
    get caller(): Caller {
      if (caller === undefined) {
        if (ended) {
          throw new Error(
            'this InvocationContext belongs to a call that has ended; store ctx.caller.original.ref instead',
          );
        }
        caller = callerFromHost(hostCaller());
      }
      return caller;
    },
    end(returnedStream = false): void {
      if (returnedStream && caller === undefined && !ended) {
        caller = callerFromHost(hostCaller());
      }
      ended = true;
    },
  };
}

/** The context of an invocation made by `caller`. */
export function invocationContextOf(caller: Caller): InvocationContext {
  return { caller };
}
