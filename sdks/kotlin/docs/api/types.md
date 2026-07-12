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

Anything else raises `Unsupported Kotlin type for WIT mapping: <fqn>`.

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

> **Other utility types.** The caller's [`Principal`](agent-model.md#baseagent) and the
> [`Uuid`](agent-model.md#baseagent) it may carry are runtime identity types (read via
> `BaseAgent.principal`), not agent-surface parameter types — see the [agent model](agent-model.md).

## Value model (`SchemaValue` converters)

`ConverterCodegen` generates the recursive mapping between the lifted `SchemaValue` tree and
your Kotlin values. The correspondence:

| `TypeDesc` | `SchemaValue` variant(s) | Encode / decode |
|-----------|--------------------------|-----------------|
| `Prim(wit)` | `Bool`, `S8`..`S64`, `U8`..`U64`, `F32`/`F64`, `Chr`, `Str` | `.v` field |
| `UnitT` | `Unit_` | decode is not supported (a Unit return has no value to read) |
| `Record` | `Record(fields: List<SchemaValue>)` | positional by field index; rebuilt via the class constructor |
| `ListT` | `ListVal(items)` | `map` over items |
| `OptionT` | `OptionVal(inner: SchemaValue?)` | `?.let` over `inner` |
| `EnumT` | `EnumVal(caseIndex)` | `entries[caseIndex]` / `.ordinal` |
| `VariantT` | `VariantVal(caseIndex, payload)` | `when` over `caseIndex`; payload is a `Record` for payload cases |
| `MapT` | `MapVal(entries: List<Pair<SchemaValue,SchemaValue>>)` | `associate` / `entries.map` |
| `TupleT` | `TupleVal(items)` | positional by element index; rebuilt via `Pair`/`Triple` ctor |

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

- **Integer widths.** The forward direction preserves each width the user writes
  (`Int`→`s32`, `Long`→`s64`, `UInt`→`u32`, …). Per the `TypeMapper` source note, a WIT width
  other than `s32` may not survive a full Kotlin → WIT → Kotlin round-trip through the
  earlier Kotlin/JS binding, which collapsed every integer width to `Int`; `Int ⟷ s32` is
  always exact.
- **`char`.** The grammar and `SchemaValue` (`Chr`) include `char`, but Kotlin `Char` is not
  in `TypeMapper`'s primitive table — use `String`.
- **`result<T,E>`.** The witType grammar recognizes `result<T,E>`, but `TypeMapper.resolve`
  does not produce it from a Kotlin type; model fallible returns with a sealed class
  (→ variant) instead.
- **Object vs param variant cases.** A sealed subclass with no primary-constructor params
  (including `object`) becomes a payloadless case (`_`); one with params becomes a record.
- **Case/field order is fixed** at resolution time and reused for both encode and decode, so
  reordering enum entries or sealed subclasses changes the wire encoding.
- **Names are schema-only.** Field and case names live in the schema graph; the value tree is
  positional, so lift matches by index.
- **Unsupported types error at compile time** with `Unsupported Kotlin type for WIT mapping`.
