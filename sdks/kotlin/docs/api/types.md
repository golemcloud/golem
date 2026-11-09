# Types

> How Kotlin types map to Golem's WIT / schema value model for agent constructor parameters, method parameters, and return types — exactly what `TypeMapper.resolve` and `TypeDesc.toWit()` support. **Status:** Complete (per capability ledger).

## Overview

When KSP processes an [`@Agent`](agent-model.md) class, every constructor parameter, method
parameter, and method return type is resolved to a `TypeDesc` by
`cloud.golem.ksp.TypeMapper.resolve`. Each `TypeDesc` produces:

- a **WIT type string** via `TypeDesc.toWit()` — consumed by the runtime schema-graph builder
  and value lift, and
- recursive **`SchemaValue` ⟷ Kotlin converters** via `ConverterCodegen.encode` / `decode`,
  used by the generated registration and RPC clients.

The full composite set is modelled, and arbitrary nesting is supported: records inside
lists, options of maps, variants carrying records, tuples of enums, and so on. Field/case
**names** matter for the schema graph; at the value level everything is positional.

See [Agent Model](agent-model.md) for where these types appear, and the
[SDK README](../../README.md) for the build/deploy flow.

## Type mapping table

| Kotlin type | WIT type (`toWit()`) | `TypeDesc` | Notes |
|-------------|----------------------|------------|-------|
| `Int` | `s32` | `Prim` | |
| `Long` | `s64` | `Prim` | |
| `Short` | `s16` | `Prim` | |
| `Byte` | `s8` | `Prim` | |
| `UInt` | `u32` | `Prim` | |
| `ULong` | `u64` | `Prim` | |
| `UShort` | `u16` | `Prim` | |
| `UByte` | `u8` | `Prim` | |
| `Float` | `f32` | `Prim` | |
| `Double` | `f64` | `Prim` | |
| `Boolean` | `bool` | `Prim` | |
| `String` | `string` | `Prim` | |
| `Unit` | `()` | `UnitT` | A method with no return value. |
| `T?` | `option<T>` | `OptionT` | Wraps the non-null form; nesting recurses. |
| `List<T>` | `list<T>` | `ListT` | |
| `Map<K, V>` | `map<K,V>` | `MapT` | |
| `Pair<A, B>` | `tuple<A,B>` | `TupleT` | |
| `Triple<A, B, C>` | `tuple<A,B,C>` | `TupleT` | |
| `enum class` | `enum<CASE,...>` | `EnumT` | Entry names in declaration order. |
| `sealed class` / `interface` | `variant<Case:payloadOrUnit,...>` | `VariantT` | Object case = no payload (`_`); param case = record. |
| `data class` | `record<field:T,...>` | `Record` | Fields = primary-constructor params, recursed. |
| `cloud.golem.Datetime` | `datetime` | `DatetimeT` | `data class Datetime(seconds: Long, nanoseconds: Int)`. |
| `cloud.golem.runtime.Either<L, R>` | `result<R,L>` | `ResultT` | `Right` = ok, `Left` = err; a `Unit` arm becomes `_`. |
| `cloud.golem.runtime.Uuid` | `uuid` | `UuidT` | `data class Uuid(highBits: Long, lowBits: Long)` (two `u64` halves). Not the same class as `cloud.golem.Uuid` (`ULong` halves, used by `Principal.GolemUser`), which is not mapped. |
| `kotlin.time.Duration` | `duration` | `DurationT` | Signed nanoseconds (`s64`); an infinite duration saturates. |
| `ByteArray` | `list<u8>` | `BytesT` | |
| `cloud.golem.schema.UnstructuredText` | `unstructured-text` (`unstructured-text<en,de>` with an allow-list) | `UnstructuredT` | `variant { inline: text, url: url }` with the `unstructured-text` role; `Inline(text, languageCode?)` / `Url(url)`. |
| `cloud.golem.schema.UnstructuredBinary` | `unstructured-binary` (`unstructured-binary<image/png>` with an allow-list) | `UnstructuredT` | `variant { inline: binary, url: url }` with the `unstructured-binary` role; `Inline(bytes, mimeType?)` / `Url(url)`. |
| `cloud.golem.schema.GolemPath` | `path<direction,kind,mime,extensions>` | `PathT` | `value class GolemPath(value: String)`; the widest spec (`in-out`, `any`) by default, narrowed by `@Arg`. |
| `cloud.golem.schema.Url` | `url` (`url<<https>,_>` once narrowed) | `UrlT` | `value class Url(value: String)`; unrestricted by default, narrowed by `@Arg(schemes = [...])`. |
| `cloud.golem.schema.Quantity` | `quantity<baseUnit,<suffixes>,_,_>` | `QuantityT` | `data class Quantity(mantissa: Long, scale: Int, unit: String = "")`, the value being `mantissa * 10^-scale`. `@QuantityUnit` names the units; see below. |
| `cloud.golem.streams.AgentStream<T>` | `stream<T>` | `StreamT` | See [Streams](streams.md). |
| `cloud.golem.runtime.host.QuotaToken` | `quota-token` | `QuotaTokenT` | Owned resource handle; see [Quota](quota.md). |
| `cloud.golem.http.HttpRequest` / `HttpResponse` | canonical envelope record | `HttpT` | See [HTTP routers](http-routers.md). |
| `cloud.golem.permissions.PermissionCard` / `PolymorphicPermissionCard` | `permission-card` / `polymorphic-permission-card` | `CardT` | See [Permission cards](permissions.md). |

**Quantity units:** put `@QuantityUnit("m")` on the type, or `@QuantityUnit("m", "m", "km")` to accept kilometres as well (`fun f(d: @QuantityUnit("m", "m", "km") Quantity)`; it also works on
return types and fields). The first argument is the base unit and the rest are the allowed suffixes; they reach the schema as the `quantity-spec`'s `base-unit` and `allowed-suffixes`, hex-encoded in
the witType grammar. **The host's rule decides a value's unit:** with no allowed suffix it must be the base unit, and with suffixes it must be one of them — the base unit is *not* implicitly allowed
beside them, so name it among the suffixes when a value may use it (`check_quantity` in `golem-common/src/schema/validation/value.rs`). The generated codecs apply the same rule, so a bad unit fails
in the guest rather than as a rejected invocation. An empty `unit` on a value means the base unit, as Go's `EncodeQuantity` reads one. A `Quantity` without the annotation is rejected: the unit cannot
be read from the type, which is one class whatever it measures. The Scala SDK takes the spec from an implicit `QuantityUnit[U]` and the Go SDK from a type parameter; a Kotlin type parameter is erased
and the SDK does no runtime reflection, so the spec is written where the type is. No SDK authors the spec's `min` or `max`.

**Allow-lists:** put `@Languages("en", "de")` or `@MimeTypes("image/png")` on the type (`fun f(t: @Languages("en", "de") UnstructuredText)`; it also works on return types, fields and
generated RPC client signatures). The codes go into the schema as the text type's `languages` / the binary type's `mime-types` restriction, so the host rejects a mismatching inline value
before the call, and the generated decoder rejects one with no language / MIME type at all (an `invalid-input` error, which the host treats as a failed invocation). The decoder also rejects a variant case other than `inline` / `url` and a payload of the wrong type. A URL value is always
accepted. Without the annotation every language code / MIME type, or none, is accepted. Codes must not contain `,`, `<` or `>`.
`Char` outside the Basic Multilingual Plane is not representable (a Kotlin `Char` is one UTF-16 unit).

Anything else raises `Unsupported Kotlin type for WIT mapping: <fqn> (composite kind not yet supported)`.

## `TypeDesc` and `toWit()`

`TypeDesc` is the resolved agent-surface type. Each case's `toWit()` output:

```kotlin
sealed class TypeDesc {
    abstract fun toWit(): String

    // Prim("s32").toWit()      == "s32"
    data class Prim(val wit: String) : TypeDesc()

    // UnitT.toWit()            == "()"
    object UnitT : TypeDesc()

    // Record("com.acme.Point", [x:s32, y:s32]).toWit() == "record<x:s32,y:s32>"
    data class Record(val kotlinFqn: String, val fields: List<Field>) : TypeDesc()

    // ListT(Prim("string")).toWit()   == "list<string>"
    data class ListT(val elem: TypeDesc) : TypeDesc()

    // OptionT(Prim("s32")).toWit()    == "option<s32>"
    data class OptionT(val inner: TypeDesc) : TypeDesc()

    // EnumT("com.acme.Color", ["Red","Green"]).toWit() == "enum<Red,Green>"
    data class EnumT(val kotlinFqn: String, val cases: List<String>) : TypeDesc()

    // VariantT("com.acme.Shape", [Circle:record<r:f64>, Unknown:_]).toWit()
    //   == "variant<Circle:record<r:f64>,Unknown:_>"
    data class VariantT(val kotlinFqn: String, val cases: List<VariantCase>) : TypeDesc()

    // MapT(Prim("string"), Prim("s32")).toWit()  == "map<string,s32>"
    data class MapT(val key: TypeDesc, val value: TypeDesc) : TypeDesc()

    // TupleT("kotlin.Pair", [Prim("string"), Prim("s32")]).toWit() == "tuple<string,s32>"
    data class TupleT(val kotlinFqn: String, val elems: List<TypeDesc>) : TypeDesc()

    // ResultT(ok, err).toWit() == "result<ok,err>" (a Unit arm prints as "_")
    data class ResultT(val ok: TypeDesc, val err: TypeDesc) : TypeDesc()

    // Scalar/handle cases (see the mapping table): DatetimeT "datetime", BytesT "list<u8>", UuidT "uuid",
    // DurationT "duration", QuotaTokenT "quota-token", UnstructuredT(binary, allowed) "unstructured-text[<..>]" /
    // "unstructured-binary[<..>]", PathT(spec) "path<...>", UrlT(schemes, hosts) "url[<..>]",
    // QuantityT(baseUnit, suffixes) "quantity<..>", CardT(polymorphic), StreamT(elem) "stream<T>", HttpT(response).
    object DatetimeT : TypeDesc()
    object BytesT : TypeDesc()
    object UuidT : TypeDesc()
    object DurationT : TypeDesc()
    object QuotaTokenT : TypeDesc()
    data class UnstructuredT(val binary: Boolean, val allowed: List<String>? = null) : TypeDesc()
    data class CardT(val polymorphic: Boolean) : TypeDesc()
    data class StreamT(val elem: TypeDesc) : TypeDesc()
    data class HttpT(val response: Boolean) : TypeDesc()
}

data class Field(val name: String, val type: TypeDesc)
data class VariantCase(val name: String, val kotlinFqn: String, val payload: TypeDesc?)
```

The WIT-string grammar these produce (shared by the value lift and schema-graph builder):

```
primitives: bool, s8, s16, s32, s64, u8, u16, u32, u64, f32, f64, char, string
record<name0:T0,name1:T1,...>      (field names; body is positional at the value level)
variant<c0:T0,c1:_,...>            (case names; `_` = no payload)
enum  or  enum<c0,c1,...>          (case names; the value carries only a case index)
list<T>   option<T>   tuple<T0,T1,...>   map<K,V>   result<T,E>   (`_` = unit ok/err)
```

## How resolution works (`TypeMapper.resolve`)

- **Nullable first:** `T?` resolves to `OptionT(resolve(T))` before anything else.
- **Primitives / `Unit`:** looked up directly.
- **`List<T>`:** single type argument recursed.
- **`Map<K, V>`:** both type arguments recursed.
- **`Pair` / `Triple`:** every type argument recursed into a `TupleT`.
- **`enum class`:** cases are the enum entries in declaration order.
- **`sealed` class/interface:** each **direct** subclass is a variant case. An `object`
  subclass (or one with no primary-constructor params) has **no payload** (`_`); a subclass
  **with** primary-constructor params carries a **record** of those params. Case order is
  fixed at resolution and reused for encode/decode. A sealed type with no subclasses errors.
- **`data class`:** a record whose fields are the primary-constructor parameters, recursed.
- **`cloud.golem.Datetime`:** maps to the `datetime` WIT type (a `DatetimeT`).
- **`cloud.golem.runtime.Either<L, R>`:** maps to `result<R,L>` — `Right` is the ok arm, `Left`
  the err arm; a `Unit` arm becomes `_`.
- **Scalar and handle types:** `ByteArray`, `cloud.golem.runtime.Uuid`, `kotlin.time.Duration`, `UnstructuredText` /
  `UnstructuredBinary` (honouring `@Languages` / `@MimeTypes`), `QuotaToken`, `AgentStream<T>`, `HttpRequest` /
  `HttpResponse`, and `PermissionCard` / `PolymorphicPermissionCard` are matched by fully-qualified name before the
  class-kind checks.

> **Other utility types.** The caller's [`Principal`](agent-model.md#baseagent) and the
> `cloud.golem.Uuid` it may carry (`Principal.GolemUser.accountId`) are runtime identity types (read via
> `BaseAgent.principal`), not agent-surface parameter types — see the [agent model](agent-model.md).

## Value model (`SchemaValue` converters)

`ConverterCodegen` generates the recursive mapping between the lifted `SchemaValue` tree and
your Kotlin values. The correspondence:

| `TypeDesc` | `SchemaValue` variant(s) | Encode / decode |
|-----------|--------------------------|-----------------|
| `Prim(wit)` | `Bool`, `S8`..`S64`, `U8`..`U64`, `F32`/`F64`, `Chr`, `Str` | `.v` field |
| `UnitT` | `Unit_` | decode is not supported (a Unit return has no value to read) |
| `Record` | `Record(fields: List<SchemaValue>)` | positional by field index; rebuilt via the class constructor |
| `ListT` | `ListVal(items)`; the items are converted one at a time, when the value is lowered | `map` over items, read from the arguments one at a time |
| (a fixed-length list parameter of another agent; no Kotlin type maps to it) | `FixedListVal(items)` — sent as a `fixed-list-value` node; a received one lifts to `ListVal` | — |
| `OptionT` | `OptionVal(inner: SchemaValue?)` | `?.let` over `inner` |
| `EnumT` | `EnumVal(caseIndex)` | `entries[caseIndex]` / `.ordinal` |
| `VariantT` | `VariantVal(caseIndex, payload)` | `when` over `caseIndex`; payload is a `Record` for payload cases |
| `MapT` | `MapVal(entries: List<Pair<SchemaValue,SchemaValue>>)` | `associate` / `entries.map` |
| `TupleT` | `TupleVal(items)` | positional by element index; rebuilt via `Pair`/`Triple` ctor |
| `ResultT` | `ResultVal(ok, inner)` | `Either.Right` (ok) / `Either.Left` (err); a `Unit` arm has no `inner` |
| `DatetimeT` | `DatetimeVal(seconds, nanoseconds)` | `cloud.golem.Datetime` |
| `BytesT` / `UuidT` / `DurationT` / `UnstructuredT` | via `SchemaCodecs` helpers | see the mapping table |
| `QuotaTokenT` / `CardT` / `StreamT` | `QuotaTokenVal` / `PermissionCardVal` / `StreamVal` (owned handles) | `fromHandle(...)` |
| `HttpT` | record envelope | `HttpCodec.decodeRequest` / `decodeResponse` |

## The cost of large values

A value that crosses the agent boundary is lowered into the agent's linear memory and lifted again, and every byte of that is memory the executor
reserves for the agent until the agent is unloaded (see [`MEMORY.md`](../../MEMORY.md)). Measured on 2026-11-08 for a 256 KiB payload, as the growth of
the memory the executor granted the agent:

| Payload | As a result | As an argument |
|---|---|---|
| `String` | 6 MiB | 4 MiB |
| `ByteArray` (a `list<u8>`) | 9.6 MiB | 9.6 MiB |

A `ByteArray`, a `List<Int>`, a `List<Long>`, a `List<Double>` and a `List<String>` are lowered and lifted without an object per element,
and any other list is converted one element at a time, so a list of records never exists as a graph of schema values. The host still reads
one schema node per element, so every element costs at least 36 bytes of arena. Measured as the growth of the memory the executor granted:

| List | Size | As a result | As an argument |
|---|---|---|---|
| `List<Int>` | 100,000 | 13.4 MiB | 4.3 MiB |
| `List<String>` | 100,000 | 24.9 MiB | 24.9 MiB |
| `List<Record>` of three fields | 50,000 | 22.9 MiB | 22.9 MiB |

A `String` costs 1 byte per character, so use a `String`, or a stream, for the largest payloads. The elements of a list are converted at
least twice when the list is lowered (once to count the nodes, once to write them), so the conversion has to be a pure function of the
element, and the list must not change between the end of the call and its lowering. A list lifted from an argument reads the arena as it is
used, so it is valid for the call that received it and fails when read after the call ends; copy it (`toList()`) if it has to outlive the call.

## Examples

### Records (data classes) as parameters and return types

```kotlin
data class GeoPoint(val lat: Double, val lon: Double)   // record<lat:f64,lon:f64>
data class Place(val name: String, val at: GeoPoint)    // record<name:string,at:record<lat:f64,lon:f64>>

@Endpoint(post = "/places")
fun addPlace(place: Place): String = place.name
```

### Lists, options, and maps

```kotlin
// param:  list<record<lat:f64,lon:f64>>
// return: option<record<name:string,at:record<lat:f64,lon:f64>>>
@Endpoint(post = "/nearest")
fun nearest(points: List<GeoPoint>, to: GeoPoint): Place? { /* ... */ }

// return: map<string,s32>
@Endpoint(get = "/counts")
fun counts(): Map<String, Int> = mapOf("a" to 1, "b" to 2)
```

### Tuples

```kotlin
// return: tuple<string,s32>
@Endpoint(get = "/top")
fun top(): Pair<String, Int> = "alice" to 42

// param: tuple<f64,f64,f64>
@Endpoint(post = "/vec")
fun addVec(v: Triple<Double, Double, Double>): Double = v.first + v.second + v.third
```

### Enums

```kotlin
enum class Priority { Low, Medium, High }   // enum<Low,Medium,High>

@Endpoint(post = "/prioritize")
fun prioritize(p: Priority): Priority = if (p == Priority.Low) Priority.Medium else p
```

### Sealed classes → variants (object cases and record cases)

```kotlin
sealed class Shape {
    object Unknown : Shape()                              // Unknown:_        (no payload)
    data class Circle(val radius: Double) : Shape()       // Circle:record<radius:f64>
    data class Rect(val w: Double, val h: Double) : Shape() // Rect:record<w:f64,h:f64>
}
// Shape.toWit() == variant<Unknown:_,Circle:record<radius:f64>,Rect:record<w:f64,h:f64>>

@Endpoint(post = "/area")
fun area(shape: Shape): Double = when (shape) {
    is Shape.Circle -> Math.PI * shape.radius * shape.radius
    is Shape.Rect -> shape.w * shape.h
    Shape.Unknown -> 0.0
}
```

### Arbitrary nesting

```kotlin
data class Order(
    val id: String,
    val lines: List<Pair<String, Int>>,   // list<tuple<string,s32>>
    val coupon: String?,                  // option<string>
    val ship: Shape                       // nested variant
)
// record<id:string,lines:list<tuple<string,s32>>,coupon:option<string>,ship:variant<...>>
```

## Notes

- **Integer widths.** Each width round-trips exactly (`Int`↔`s32`, `Long`↔`s64`, `UInt`↔`u32`, …);
  `TypeMapper.fqnToWit` / `witToFqn` are exact inverses over the primitives.
- **`char`.** The grammar and `SchemaValue` (`Chr`) include `char`, but Kotlin `Char` is not
  in `TypeMapper`'s primitive table — use `String`.
- **`result<T,E>`.** Produced from `cloud.golem.runtime.Either<L, R>` (`result<R,L>`); there is no mapping for
  `kotlin.Result`, so use `Either` or a sealed class (→ variant) for fallible returns.
- **Object vs param variant cases.** A sealed subclass with no primary-constructor params
  (including `object`) becomes a payloadless case (`_`); one with params becomes a record.
- **Case/field order is fixed** at resolution time and reused for both encode and decode, so
  reordering enum entries or sealed subclasses changes the wire encoding.
- **Names are schema-only.** Field and case names live in the schema graph; the value tree is
  positional, so lift matches by index.
- **Unsupported types error at compile time** with `Unsupported Kotlin type for WIT mapping`.
