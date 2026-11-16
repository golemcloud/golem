/**
 * The custom invocation context of `golem:durability/durability@1.6.0` that work belongs to.
 *
 * The runtime keeps one current token. Promise reactions (including `await`), timers,
 * `setImmediate`, `process.nextTick`, `queueMicrotask` and async imports run in the token that was
 * current when they were registered or called. Use this module instead of calling
 * `exchange-custom-invocation-context` directly.
 */
declare module 'wasm-rquickjs:custom-invocation-context' {
  /**
   * A scope: the root of the current exported call, or one live custom durable invocation.
   * A scope has one token object, so tokens can be compared by identity.
   */
  const tokenBrand: unique symbol;
  export interface Token {
    readonly [tokenBrand]: true;
  }

  /** A live custom durable invocation returned by asynchronous begin. */
  export interface LiveInvocation {
    enter(): void;
  }

  /** Returns the token of the current scope. Makes no host call. */
  export function capture(): Token;

  /**
   * Makes `next` current and returns the token that was current. Makes no host call when `next`
   * is already current.
   */
  export function swap(next: Token): Token;

  /** Calls `fn` with `token` current, then restores the previous token. */
  export function run<T>(token: Token, fn: () => T): T;

  /**
   * Opens the scope of a live invocation as a child of the current scope. Call it in the parent's
   * scope after asynchronous begin. It calls `live.enter()`, makes the new scope current and
   * returns its token.
   */
  export function enterLive(live: LiveInvocation): Token;

  /**
   * Closes a scope after its live invocation finished or was dropped. Works while another token
   * is current. Throws while the scope has open child scopes. Closing a closed token does nothing.
   * Work that later runs in a closed token runs in its nearest open ancestor, or in the root.
   */
  export function close(token: Token): void;
}
