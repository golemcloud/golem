/**
 * Invocation context support
 */
declare module 'golem:api/context@1.3.0' {
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  /**
   * Starts a new `span` with the given name, as a child of the current invocation context
   */
  export function startSpan(name: string): Span;
  /**
   * Gets the current invocation context
   * The function call captures the current context; if new spans are started, the returned `invocation-context` instance will not
   * reflect that.
   */
  export function currentContext(): InvocationContext;
  /**
   * Allows or disallows forwarding of trace context headers in outgoing HTTP requests
   * Returns the previous value of the setting
   */
  export function allowForwardingTraceContextHeaders(allow: boolean): boolean;
  export class Span {
    /**
     * Gets the starting time of the span
     */
    startedAt(): Datetime;
    /**
     * Set an attribute on the span
     */
    setAttribute(name: string, value: AttributeValue): void;
    /**
     * Set multiple attributes on the span
     */
    setAttributes(attributes: Attribute[]): void;
    /**
     * Early finishes the span; otherwise it will be finished when the resource is dropped
     */
    finish(): void;
  }
  export class InvocationContext {
    /**
     * Gets the current trace id
     */
    traceId(): TraceId;
    /**
     * Gets the current span id
     */
    spanId(): SpanId;
    /**
     * Gets the parent context, if any; allows recursive processing of the invocation context.
     * Alternatively, the attribute query methods can return inherited values without having to
     * traverse the stack manually.
     */
    parent(): InvocationContext | undefined;
    /**
     * Gets the value of an attribute `key`. If `inherited` is true, the value is searched in the stack of spans,
     * otherwise only in the current span.
     */
    getAttribute(key: string, inherited: boolean): AttributeValue | undefined;
    /**
     * Gets all attributes of the current invocation context. If `inherited` is true, it returns the merged set of attributes, each
     * key associated with the latest value found in the stack of spans.
     */
    getAttributes(inherited: boolean): Attribute[];
    /**
     * Gets the chain of attribute values associated with the given `key`. If the key does not exist in any of the
     * spans in the invocation context, the list is empty. The chain's first element contains the most recent (innermost) value.
     */
    getAttributeChain(key: string): AttributeValue[];
    /**
     * Gets all values of all attributes of the current invocation context.
     */
    getAttributeChains(): AttributeChain[];
    /**
     * Gets the W3C Trace Context headers associated with the current invocation context
     */
    traceContextHeaders(): [string, string][];
  }
  export type Datetime = wasiClocks023WallClock.Datetime;
  /**
   * Possible span attribute value types
   */
  export type AttributeValue = 
  /** A string value */
  {
    tag: 'string'
    val: string
  };
  /**
   * An attribute of a span
   */
  export type Attribute = {
    key: string;
    value: AttributeValue;
  };
  /**
   * A chain of attribute values, the first element representing the most recent value
   */
  export type AttributeChain = {
    key: string;
    values: AttributeValue[];
  };
  /**
   * The trace represented by a 16 bytes hexadecimal string
   */
  export type TraceId = string;
  /**
   * The span represented by a 8 bytes hexadecimal string
   */
  export type SpanId = string;
}
