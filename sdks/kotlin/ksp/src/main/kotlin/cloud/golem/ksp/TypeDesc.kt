package cloud.golem.ksp

/**
 * A resolved agent-surface type, produced from a Kotlin `KSType` and used to drive BOTH the rich
 * witType string ([toWit], consumed by the runtime schema-graph builder + value lift) AND the
 * recursive SchemaValue<->Kotlin converter codegen in [NativeRegistrationEmitter].
 *
 * The full composite set is modelled here; [TypeMapper.resolve] and the emitter's converters are
 * extended kind-by-kind across increments.
 */
sealed class TypeDesc {
    abstract fun toWit(): String

    /** A WIT primitive (`s32`, `string`, `bool`, ...). */
    data class Prim(val wit: String) : TypeDesc() {
        override fun toWit(): String = wit
    }

    /** `kotlin.Unit` — a method with no return value. */
    object UnitT : TypeDesc() {
        override fun toWit(): String = "()"
    }

    /** A Kotlin data class -> WIT record. [kotlinFqn] is the fully-qualified class name (for ctor calls). */
    data class Record(val kotlinFqn: String, val fields: List<Field>) : TypeDesc() {
        override fun toWit(): String = "record<" + fields.joinToString(",") { "${it.name}:${it.type.toWit()}" } + ">"
    }

    /** Kotlin `List<T>` -> WIT list. */
    data class ListT(val elem: TypeDesc) : TypeDesc() {
        override fun toWit(): String = "list<${elem.toWit()}>"
    }

    /** Kotlin `T?` -> WIT option. */
    data class OptionT(val inner: TypeDesc) : TypeDesc() {
        override fun toWit(): String = "option<${inner.toWit()}>"
    }

    /** Kotlin enum class -> WIT enum. [cases] are the entry names in declaration order. */
    data class EnumT(val kotlinFqn: String, val cases: List<String>) : TypeDesc() {
        override fun toWit(): String = "enum<" + cases.joinToString(",") + ">"
    }

    /** Kotlin sealed class/interface -> WIT variant. */
    data class VariantT(val kotlinFqn: String, val cases: List<VariantCase>) : TypeDesc() {
        override fun toWit(): String = "variant<" + cases.joinToString(",") { "${it.name}:${it.payload?.toWit() ?: "_"}" } + ">"
    }

    /** Kotlin `Map<K,V>` -> WIT map. */
    data class MapT(val key: TypeDesc, val value: TypeDesc) : TypeDesc() {
        override fun toWit(): String = "map<${key.toWit()},${value.toWit()}>"
    }

    /** Kotlin `Pair`/`Triple` -> WIT tuple. [kotlinFqn] is the tuple class (for ctor calls). */
    data class TupleT(val kotlinFqn: String, val elems: List<TypeDesc>) : TypeDesc() {
        override fun toWit(): String = "tuple<" + elems.joinToString(",") { it.toWit() } + ">"
    }

    /**
     * Kotlin `Either<L,R>` -> WIT `result<R,L>` (`Right` = ok, `Left` = err). A `Unit` arm becomes
     * `_` (WIT's unit ok/err marker).
     */
    data class ResultT(val ok: TypeDesc, val err: TypeDesc) : TypeDesc() {
        override fun toWit(): String = "result<${arm(ok)},${arm(err)}>"
        private fun arm(t: TypeDesc): String = if (t is UnitT) "_" else t.toWit()
    }

    /** Kotlin `cloud.golem.Datetime` -> WIT `datetime`. */
    object DatetimeT : TypeDesc() {
        override fun toWit(): String = "datetime"
    }
}

/** A record field: its name + type. */
data class Field(val name: String, val type: TypeDesc)

/** A variant case: its name, the Kotlin subclass FQN, and its payload type (null for a no-payload case). */
data class VariantCase(val name: String, val kotlinFqn: String, val payload: TypeDesc?)
