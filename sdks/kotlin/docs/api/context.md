# Context API

> `ContextApi` — the native Kotlin/Wasm binding to Golem's invocation-context / tracing interface `golem:api/context@1.5.0`. **Status:** 🟢 Available.

## Overview

`ContextApi` (`cloud.golem.runtime.host.ContextApi`) exposes Golem's tracing model: **spans**
(units of work you open and close) and the **invocation context** (the stack of attributes
accumulated by automatic and user-defined spans, plus trace/span ids and trace-context
headers). It mirrors OpenTelemetry-style tracing but is backed entirely by the Golem host.

Both `span` and `invocation-context` are component-model **resources**, so `Span` and
`InvocationContext` wrap raw host handles that are **not** tied to Kotlin/Wasm GC — each must be
`close()`d. For `Span`, dropping without an explicit `finish()` is the *normal* lifecycle: the
host finishes the span automatically at drop time, so `finish()` is only for an early finish.

Use `ContextApi` from inside an [`@Agent`](agent-model.md) method to add structured tracing
around operations, or to read trace ids for correlating logs. See the SDK overview in
[`../../README.md`](../../README.md).

## API reference

### Types

```kotlin
/** wasi:clocks/wall-clock@0.2.3's datetime record. */
data class ContextDateTime(val seconds: Long, val nanoseconds: Int)

/** golem:api/context@1.5.0's attribute-value variant (currently one case: a string). */
sealed class AttributeValue {
    data class StringValue(val value: String) : AttributeValue()
}

data class Attribute(val key: String, val value: AttributeValue)
data class AttributeChain(val key: String, val values: List<AttributeValue>)
```

### Entry point

```kotlin
object ContextApi {
    /** Start a new span with the given name, as a child of the current invocation context. */
    fun startSpan(name: String): Span

    /** The current invocation context. */
    fun currentContext(): InvocationContext

    /** Allow/disallow forwarding trace-context headers in outgoing HTTP; returns the previous value. */
    fun allowForwardingTraceContextHeaders(allow: Boolean): Boolean
}
```

### `Span`

```kotlin
/**
 * A unit of work (golem:api/context@1.5.0's span resource). MUST be close()d when done.
 * Dropping without finish() is the normal lifecycle — the host finishes it at drop time.
 */
class Span {
    /** When the span was started. */
    fun startedAt(): ContextDateTime

    /** Set a single attribute on the span. */
    fun setAttribute(name: String, value: AttributeValue)

    /** Set several attributes at once. */
    fun setAttributes(attributes: List<Attribute>)

    /** Early-finish the span; otherwise it finishes automatically when close()d. */
    fun finish()

    fun close()
}
```

### `InvocationContext`

```kotlin
/**
 * Query the stack of attributes created by automatic and user-defined spans
 * (golem:api/context@1.5.0's invocation-context resource). MUST be close()d when done.
 */
class InvocationContext {
    fun traceId(): String
    fun spanId(): String

    /** The parent context, if any. The returned InvocationContext MUST also be close()d. */
    fun parent(): InvocationContext?

    fun getAttribute(key: String, inherited: Boolean): AttributeValue?
    fun getAttributes(inherited: Boolean): List<Attribute>

    fun getAttributeChain(key: String): List<AttributeValue>
    fun getAttributeChains(): List<AttributeChain>

    /** W3C trace-context headers to forward on outgoing requests. */
    fun traceContextHeaders(): List<Pair<String, String>>

    fun close()
}
```

- `getAttribute` / `getAttributes` take `inherited: Boolean` — when `true`, attributes from
  parent spans are included, not only those set on the current context.
- `getAttributeChain(key)` returns every value recorded for `key` up the span stack;
  `getAttributeChains()` returns all keys' chains at once.

## Examples

### Tracing an operation with a span

```kotlin
@Agent
class OrderAgent : BaseAgent() {

    @Endpoint
    fun placeOrder(sku: String, qty: Int): String {
        val span = ContextApi.startSpan("place-order")
        try {
            span.setAttributes(
                listOf(
                    Attribute("sku", AttributeValue.StringValue(sku)),
                    Attribute("qty", AttributeValue.StringValue(qty.toString())),
                )
            )

            val result = reserveInventory(sku, qty) // ... real work ...

            span.setAttribute("result", AttributeValue.StringValue(result))
            return result
        } finally {
            // close() finishes the span (no explicit finish() needed for the normal path).
            span.close()
        }
    }
}
```

### Reading trace ids and inherited attributes

```kotlin
val ctx = ContextApi.currentContext()
try {
    val traceId = ctx.traceId()
    val spanId = ctx.spanId()
    val tenant = ctx.getAttribute("tenant", inherited = true)
    log("trace=$traceId span=$spanId tenant=$tenant")
} finally {
    ctx.close()
}
```

### Forwarding trace-context headers downstream

```kotlin
val previous = ContextApi.allowForwardingTraceContextHeaders(true)
// ... make outgoing HTTP calls; W3C traceparent/tracestate headers are now propagated ...
ContextApi.allowForwardingTraceContextHeaders(previous) // restore
```

## Notes

- Binds `golem:api/context@1.5.0` (as declared verbatim in the source `@WasmImport` bindings).
- `Span` and `InvocationContext` are resource-backed and **must** be `close()`d — an unclosed
  handle leaks in the host's resource table until the whole component instance tears down.
  `parent()` returns a fresh `InvocationContext` that must be closed too.
- Every `Span` / `InvocationContext` method throws if called after `close()`.
- `attribute-value` currently models a single WIT case (`StringValue`); the `sealed class`
  leaves room for future cases without a breaking change.
- Unlike `golem:api/host`'s `get-agents`, `span` and `invocation-context` have no WIT
  `constructor`: handles are obtained only via `startSpan` / `currentContext`.
- See also the runtime primitives on [`HostApi`](host-api.md) and the durability model in
  [Durability API](durability.md).
