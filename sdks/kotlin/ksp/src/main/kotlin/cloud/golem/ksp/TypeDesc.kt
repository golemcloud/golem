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

    /**
     * A `list<string>` of grammar leaves, or `_` for none. Each leaf is hex-encoded UTF-8, because that is
     * what the SDK's `unescapeOptionalLeafList` decodes on the other side of `path<...>` and `url<...>`.
     * The `text<...>` / `binary<...>` allow-lists use a different, unescaped convention -- do not copy this
     * there, or `allowListOf` will read the hex as the value.
     */
    protected fun leafList(values: List<String>?): String = values?.joinToString(",", "<", ">") { leaf(it) } ?: "_"

    /** One grammar leaf, hex-encoded UTF-8, as [leafList] encodes each of its items. */
    protected fun leaf(value: String): String = value.toByteArray(Charsets.UTF_8).joinToString("") { b -> (b.toInt() and 0xFF).toString(16).padStart(2, '0') }

    /**
     * A Kotlin `String` that a tool argument restricts with a regular expression or a length: a `text` in the tool's schema, whose values are
     * text values. Not produced by [TypeMapper].
     */
    data class TextT(val regex: String?, val minLength: Int?, val maxLength: Int?) : TypeDesc() {
        override fun toWit(): String = "text"
    }

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

    /**
     * `cloud.golem.permissions.PermissionCard` (concrete) / `PolymorphicPermissionCard` (template) -> a `permission-card`
     * handle; the schema spec records whether the card is polymorphic.
     */
    data class CardT(val polymorphic: Boolean) : TypeDesc() {
        override fun toWit(): String = if (polymorphic) "polymorphic-permission-card" else "permission-card"
        val kotlinFqn: String get() = if (polymorphic) "cloud.golem.permissions.PolymorphicPermissionCard" else "cloud.golem.permissions.PermissionCard"
    }

    /** `cloud.golem.runtime.host.QuotaToken` -> an owned `quota-token` handle. */
    object QuotaTokenT : TypeDesc() {
        override fun toWit(): String = "quota-token"
    }

    /** `kotlin.ByteArray` -> `list<u8>`. */
    object BytesT : TypeDesc() {
        override fun toWit(): String = "list<u8>"
    }

    /** `cloud.golem.runtime.Uuid` -> `uuid`. */
    object UuidT : TypeDesc() {
        override fun toWit(): String = "uuid"
    }

    /**
     * `cloud.golem.schema.GolemPath` -> `path<direction,kind,mime,extensions>`. The widest spec is the default,
     * as in Scala's `GolemPath.defaultSpec`; `@Arg` narrows it on a tool argument. Not produced by [TypeMapper]
     * with anything other than the default -- the narrowing is applied where the annotation is read.
     */
    data class PathT(
        val direction: String = "in-out",
        val kind: String = "any",
        val mimeTypes: List<String>? = null,
        val extensions: List<String>? = null,
    ) : TypeDesc() {
        override fun toWit(): String = "path<$direction,$kind,${leafList(mimeTypes)},${leafList(extensions)}>"
    }

    /** `cloud.golem.schema.Url` -> `url`, or `url<schemes,hosts>` once `@Arg(schemes = ...)` narrows it. */
    data class UrlT(val schemes: List<String>? = null, val hosts: List<String>? = null) : TypeDesc() {
        override fun toWit(): String = if (schemes == null && hosts == null) "url" else "url<${leafList(schemes)},${leafList(hosts)}>"
    }

    /**
     * `cloud.golem.schema.Quantity` -> `quantity<baseUnit,<suffixes>,_,_>`, the base unit and the suffixes
     * hex-encoded as grammar leaves. `@QuantityUnit` on the declared type names them. The spec's `min` and `max`
     * stay `_`: no SDK authors them, in Kotlin or in Scala.
     */
    data class QuantityT(val baseUnit: String, val suffixes: List<String>) : TypeDesc() {
        override fun toWit(): String = "quantity<${leaf(baseUnit)},${leafList(suffixes)},_,_>"
    }

    /** `kotlin.time.Duration` -> `duration` (signed nanoseconds). */
    object DurationT : TypeDesc() {
        override fun toWit(): String = "duration"
    }

    /** `cloud.golem.schema.UnstructuredText` / `UnstructuredBinary` -> a `variant { inline, url }` with an unstructured role. */
    data class UnstructuredT(val binary: Boolean, val allowed: List<String>? = null) : TypeDesc() {
        override fun toWit(): String {
            val base = if (binary) "unstructured-binary" else "unstructured-text"
            return if (allowed == null) base else "$base<${allowed.joinToString(",")}>"
        }
    }

    /** `cloud.golem.http.HttpRequest` / `HttpResponse`: the canonical HTTP router envelopes (a record with a `stream<list<u8>>` body). */
    data class HttpT(val response: Boolean) : TypeDesc() {
        override fun toWit(): String = if (response) RESPONSE_WIT else REQUEST_WIT

        companion object {
            private const val HEADERS = "headers:list<record<name:string,value:list<u8>>>"
            const val REQUEST_WIT = "record<method:string,scheme:string,authority:string,path:string,query:option<string>,$HEADERS,body:stream<list<u8>>>"
            const val RESPONSE_WIT = "record<status:u16,$HEADERS,body:stream<list<u8>>>"
        }
    }

    /** `cloud.golem.streams.AgentStream<T>` -> a `schema-value-stream` of [elem]. */
    data class StreamT(val elem: TypeDesc) : TypeDesc() {
        override fun toWit(): String = "stream<${elem.toWit()}>"
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
