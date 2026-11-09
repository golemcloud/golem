package cloud.golem.wasm

// The witType-string grammar shared by the value lift (Lift.kt) and the schema-graph builder
// (cloud.golem.runtime.AgentTypeModel / ToolModel). A witType is one of:
//   primitives: bool, s8, s16, s32, s64, u8, u16, u32, u64, f32, f64, char, string
//   record<name0:T0,name1:T1,...>      (field names; body is positional at the value level)
//   variant<c0:T0,c1:_,...>            (case names; `_` = no payload)
//   enum  or  enum<c0,c1,...>          (case names; the value carries only a case index)
//   list<T>   option<T>   tuple<T0,T1,...>   map<K,V>   result<T,E>   (`_` = unit ok/err)
//   uuid  duration  text  binary  url  (scalar schema types)   unstructured-text  unstructured-binary  (a variant of text/binary | url, with a role)
//   stream<T>   (a schema-value-stream of T values)
//   secret<T>   (an own<secret> handle whose revealed value has type T; bare `secret` is the same handle, type unspecified)
// Field/case NAMES matter only for the schema-graph; the value tree is positional, so lift drops
// them. Kotlin identifiers can't contain ',' '<' '>' or ':', so the split helpers are unambiguous.

/** `variant { inline: text, url: url }` with the `unstructured-text` role: free-form text or a URL referring to it. */
internal const val UNSTRUCTURED_TEXT = "unstructured-text"

/** `variant { inline: binary, url: url }` with the `unstructured-binary` role. */
internal const val UNSTRUCTURED_BINARY = "unstructured-binary"

/** The plain variant an unstructured witType is lifted and lowered as (the role is only schema metadata). */
internal fun expandUnstructured(wit: String): String = when {
    wit == UNSTRUCTURED_TEXT -> "variant<inline:text,url:url>"
    wit == UNSTRUCTURED_BINARY -> "variant<inline:binary,url:url>"
    // `unstructured-text<en,de>` / `unstructured-binary<image/png>`: the allow-list rides on the inline branch's `text<...>` / `binary<...>`.
    wit.startsWith("$UNSTRUCTURED_TEXT<") && wit.endsWith(">") -> "variant<inline:text<${innerOf(wit, "$UNSTRUCTURED_TEXT<")}>,url:url>"
    wit.startsWith("$UNSTRUCTURED_BINARY<") && wit.endsWith(">") -> "variant<inline:binary<${innerOf(wit, "$UNSTRUCTURED_BINARY<")}>,url:url>"
    else -> wit
}

/** True for `unstructured-text`, with or without a language allow-list. */
internal fun isUnstructuredText(wit: String): Boolean = wit == UNSTRUCTURED_TEXT || (wit.startsWith("$UNSTRUCTURED_TEXT<") && wit.endsWith(">"))

/** True for `unstructured-binary`, with or without a MIME allow-list. */
internal fun isUnstructuredBinary(wit: String): Boolean = wit == UNSTRUCTURED_BINARY || (wit.startsWith("$UNSTRUCTURED_BINARY<") && wit.endsWith(">"))

/**
 * True for `path`, with or without its spec (`path<in-out,any,_,_>`). The value grammar needs only the bare
 * tag, because a `path-value` is a string and carries no spec; the schema grammar needs the spec. So both
 * spellings reach code that handles a path, and only the schema builder reads what is inside the brackets.
 */
internal fun isPathWit(wit: String): Boolean = wit == "path" || (wit.startsWith("path<") && wit.endsWith(">"))

/** True for `url`, with or without its restrictions (`url<<https>,_>`). Same split as [isPathWit]. */
internal fun isUrlWit(wit: String): Boolean = wit == "url" || (wit.startsWith("url<") && wit.endsWith(">"))

/**
 * True for `quantity`, with or without its spec (`quantity<6d,<>,_,_>`). Same split as [isPathWit]: a
 * `quantity-value` carries its own unit string, so the value grammar needs only the bare tag.
 */
internal fun isQuantityWit(wit: String): Boolean = wit == "quantity" || (wit.startsWith("quantity<") && wit.endsWith(">"))

/** `url<schemesListOrUnderscore,hostsListOrUnderscore>` -- the `url-restrictions` of a `url` schema type. */
internal data class ParsedUrlRestrictions(val allowedSchemes: List<String>?, val allowedHosts: List<String>?)

internal fun parseUrlRestrictions(wit: String): ParsedUrlRestrictions {
    if (wit == "url") return ParsedUrlRestrictions(null, null)
    val parts = splitTopLevelCommas(innerOf(wit, "url<"))
    return ParsedUrlRestrictions(unescapeOptionalLeafList(parts[0]), unescapeOptionalLeafList(parts[1]))
}

/** True for `text` / `binary` (scalar schema types), with or without an allow-list (`text<en,de>`). */
internal fun isTextType(wit: String): Boolean = wit == "text" || (wit.startsWith("text<") && wit.endsWith(">"))

internal fun isBinaryType(wit: String): Boolean = wit == "binary" || (wit.startsWith("binary<") && wit.endsWith(">"))

/** The allow-list of a restricted `text<...>` / `binary<...>`, or `null` when unrestricted. */
internal fun allowListOf(wit: String): List<String>? = when {
    wit.startsWith("text<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "text<"))
    wit.startsWith("binary<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "binary<"))
    else -> null
}

/**
 * Splits [s] on top-level commas, ignoring commas nested inside `<...>`. An empty string yields an
 * empty list (NOT `[""]`) so an empty composite -- `record<>` / `tuple<>` / `variant<>`, e.g. a
 * no-argument method's `function-input` -- has zero fields/cases rather than one phantom `""` field.
 * The phantom field previously made the value lift read past a zero-length child list, producing a
 * garbage value-node index and an out-of-bounds dereference on live oplog/tsv data.
 */
internal fun splitTopLevelCommas(s: String): List<String> {
    if (s.isEmpty()) return emptyList()
    val parts = mutableListOf<String>()
    var depth = 0
    var start = 0
    for (i in s.indices) {
        when (s[i]) {
            '<' -> depth++
            '>' -> depth--
            ',' -> if (depth == 0) {
                parts.add(s.substring(start, i))
                start = i + 1
            }
        }
    }
    parts.add(s.substring(start))
    return parts
}

/**
 * Hex-encodes a string so it can be embedded as one leaf of the grammar -- a path's mime type, a
 * quantity's unit, a union discriminator's literal -- without its own `,` / `<` / `>` ever being
 * mistaken for structure. The same convention `cloud.golem.runtime.ToolRefinement` already uses
 * for a `text` type's `regex` refinement (`text~regex=5e5b612d7a5d2b24`), reused here rather than
 * invented afresh: hex output is only `[0-9a-f]`, so `splitTopLevelCommas` needs no change at all
 * to carry it.
 */
internal fun escapeGrammarLeaf(s: String): String = s.encodeToByteArray().joinToString("") { b -> (b.toInt() and 0xFF).toString(16).padStart(2, '0') }

/** Inverse of [escapeGrammarLeaf]: reuses `ToolRefinement`'s decoder for the identical hex format. */
internal fun unescapeGrammarLeaf(s: String): String = cloud.golem.runtime.unhex(s)

/** A wrapped, escaped leaf, `_` for `null` (the existing "no value" sentinel, as in `result<ok,_>`). */
internal fun optionalEscapedLeaf(value: String?): String = value?.let(::escapeGrammarLeaf) ?: "_"

/** Inverse of [optionalEscapedLeaf]. */
internal fun unescapeOptionalLeaf(s: String): String? = if (s == "_") null else unescapeGrammarLeaf(s)

/** A `list<string>` of escaped leaves (never optional), wrapped in its own `<...>` so top-level commas inside it stay local. */
internal fun escapedLeafList(values: List<String>): String = values.joinToString(",", "<", ">") { escapeGrammarLeaf(it) }

/** Inverse of [escapedLeafList]. */
internal fun unescapeLeafList(s: String): List<String> = splitTopLevelCommas(innerOf(s, "<")).map { unescapeGrammarLeaf(it) }

/** A list of escaped leaves, wrapped in its own `<...>` so top-level commas inside it stay local -- `_` for `null` (no restriction). */
internal fun optionalEscapedLeafList(values: List<String>?): String = values?.joinToString(",", "<", ">") { escapeGrammarLeaf(it) } ?: "_"

/** Inverse of [optionalEscapedLeafList]. */
internal fun unescapeOptionalLeafList(s: String): List<String>? = if (s == "_") null else splitTopLevelCommas(innerOf(s, "<")).map { unescapeGrammarLeaf(it) }

/** For a `prefix<INNER>` witType, returns `INNER`. */
internal fun innerOf(witType: String, prefix: String): String = witType.substring(prefix.length, witType.length - 1)

/**
 * The immediate child type strings of a composite witType (for recursive type-node registration).
 * Primitives and enums have no children. `_` (unit ok/err/payload) is not a real child type.
 */
internal fun childWitTypes(wit: String): List<String> = when {
    wit.startsWith("record<") && wit.endsWith(">") ->
        splitTopLevelCommas(innerOf(wit, "record<")).map { it.substringAfter(':') }
    wit.startsWith("variant<") && wit.endsWith(">") ->
        splitTopLevelCommas(innerOf(wit, "variant<")).map { it.substringAfter(':') }.filter { it != "_" }
    wit.startsWith("list<") && wit.endsWith(">") -> listOf(innerOf(wit, "list<"))
    wit.startsWith("fixed-list<") && wit.endsWith(">") -> listOf(fixedListElement(wit))
    wit.startsWith("union<") && wit.endsWith(">") -> loweredUnionBranches(wit).map { it.type }
    wit.startsWith("option<") && wit.endsWith(">") -> listOf(innerOf(wit, "option<"))
    wit.startsWith("secret<") && wit.endsWith(">") -> listOf(innerOf(wit, "secret<"))
    wit == UNSTRUCTURED_TEXT -> listOf("text", "url")
    wit == UNSTRUCTURED_BINARY -> listOf("binary", "url")
    isUnstructuredText(wit) -> listOf("text<${innerOf(wit, "$UNSTRUCTURED_TEXT<")}>", "url")
    isUnstructuredBinary(wit) -> listOf("binary<${innerOf(wit, "$UNSTRUCTURED_BINARY<")}>", "url")
    wit.startsWith("stream<") && wit.endsWith(">") -> listOf(innerOf(wit, "stream<"))
    wit.startsWith("tuple<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "tuple<"))
    wit.startsWith("map<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "map<"))
    wit.startsWith("result<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "result<")).filter { it != "_" }
    else -> emptyList()
}

/**
 * `fixed-list<T,N>` -> `T`. The length is the last top-level part, so the element type may itself
 * hold commas (`fixed-list<map<string,s32>,3>`).
 */
internal fun fixedListElement(wit: String): String = splitTopLevelCommas(innerOf(wit, "fixed-list<")).dropLast(1).joinToString(",")

/** `fixed-list<T,N>` -> `N`. */
internal fun fixedListLength(wit: String): Int {
    val last = splitTopLevelCommas(innerOf(wit, "fixed-list<")).last()
    return last.toIntOrNull() ?: error("native fixed-list: '$wit' does not end in a length")
}

/** `flags<a,b>` -> [a, b]; a bare `flags` has no names and cannot be lowered into a schema graph. */
internal fun flagNames(wit: String): List<String> = if (wit == "flags") emptyList() else splitTopLevelCommas(innerOf(wit, "flags<"))

/** `record<name:type,...>` -> [(name, type)]. Each field is `name:type` (name required). */
internal fun recordFields(wit: String): List<Pair<String, String>> = splitTopLevelCommas(innerOf(wit, "record<")).map {
    val c = it.indexOf(':')
    it.substring(0, c) to it.substring(c + 1)
}

/** `variant<name:type,name:_,...>` -> [(name, type-or-null)]. `_` payload -> null. */
internal fun variantCases(wit: String): List<Pair<String, String?>> = splitTopLevelCommas(innerOf(wit, "variant<")).map {
    val c = it.indexOf(':')
    val name = it.substring(0, c)
    val t = it.substring(c + 1)
    name to (if (t == "_") null else t)
}

/** `enum` or `enum<c0,c1,...>` -> [c0,c1,...]. */
internal fun enumCases(wit: String): List<String> = if (wit == "enum") emptyList() else splitTopLevelCommas(innerOf(wit, "enum<"))

// --- path / quantity / union on the lowered path ----------------------------------------------
//
// These three are refused by Lift.kt's grammar for the lift direction's purposes (it only ever
// needs the bare tag -- "path"/"quantity" -- or, for union, tag:type with no discriminator,
// because a decoded VALUE already carries its branch/shape and never needs the schema-level
// spec). The schema-graph BUILDER, however, writes the real WIT record
// (path-spec/quantity-spec/union-branch), which does need these fields, so the LOWERED grammar
// below is richer than the lift grammar and is used only by cloud.golem.runtime.AgentTypeModel.

/** `path<direction,kind,mimeListOrUnderscore,extListOrUnderscore>`. */
internal data class ParsedPathSpec(
    val direction: String,
    val kind: String,
    val allowedMimeTypes: List<String>?,
    val allowedExtensions: List<String>?,
)

internal fun parsePathSpec(wit: String): ParsedPathSpec {
    // A bare `path` carries no spec, so it means the widest one: the same default `cloud.golem.schema.GolemPath`
    // maps to, and the same as Scala's `GolemPath.defaultSpec`.
    if (wit == "path") return ParsedPathSpec("in-out", "any", null, null)
    val parts = splitTopLevelCommas(innerOf(wit, "path<"))
    return ParsedPathSpec(parts[0], parts[1], unescapeOptionalLeafList(parts[2]), unescapeOptionalLeafList(parts[3]))
}

/** A `quantity-value { mantissa, scale, unit }`, as `<mantissa,scale,unit>`. */
internal data class ParsedQuantityValue(val mantissa: Long, val scale: Int, val unit: String)

internal fun parseQuantityValue(s: String): ParsedQuantityValue {
    val parts = splitTopLevelCommas(innerOf(s, "<"))
    return ParsedQuantityValue(parts[0].toLong(), parts[1].toInt(), unescapeGrammarLeaf(parts[2]))
}

/** `quantity<baseUnit,<suffix,suffix>,minOrUnderscore,maxOrUnderscore>`. */
internal data class ParsedQuantitySpec(
    val baseUnit: String,
    val allowedSuffixes: List<String>,
    val min: ParsedQuantityValue?,
    val max: ParsedQuantityValue?,
)

internal fun parseQuantitySpec(wit: String): ParsedQuantitySpec {
    val parts = splitTopLevelCommas(innerOf(wit, "quantity<"))
    return ParsedQuantitySpec(
        unescapeGrammarLeaf(parts[0]),
        unescapeLeafList(parts[1]),
        if (parts[2] == "_") null else parseQuantityValue(parts[2]),
        if (parts[3] == "_") null else parseQuantityValue(parts[3]),
    )
}

/** One of `discriminator-rule`'s six closed cases (`wit/deps/golem-core-v2/golem-core-v2.wit:1219`). */
internal sealed class ParsedDiscriminatorRule {
    data class Prefix(val text: String) : ParsedDiscriminatorRule()
    data class Suffix(val text: String) : ParsedDiscriminatorRule()
    data class Contains(val text: String) : ParsedDiscriminatorRule()
    data class Regex(val text: String) : ParsedDiscriminatorRule()
    data class FieldEquals(val fieldName: String, val literal: String?) : ParsedDiscriminatorRule()
    data class FieldAbsent(val fieldName: String) : ParsedDiscriminatorRule()
}

/** `prefix<text>` / `suffix<text>` / `contains<text>` / `regex<text>` / `field-equals<name,literalOrUnderscore>` / `field-absent<name>`. */
internal fun parseDiscriminatorRule(rule: String): ParsedDiscriminatorRule = when {
    rule.startsWith("prefix<") -> ParsedDiscriminatorRule.Prefix(unescapeGrammarLeaf(innerOf(rule, "prefix<")))
    rule.startsWith("suffix<") -> ParsedDiscriminatorRule.Suffix(unescapeGrammarLeaf(innerOf(rule, "suffix<")))
    rule.startsWith("contains<") -> ParsedDiscriminatorRule.Contains(unescapeGrammarLeaf(innerOf(rule, "contains<")))
    rule.startsWith("regex<") -> ParsedDiscriminatorRule.Regex(unescapeGrammarLeaf(innerOf(rule, "regex<")))
    rule.startsWith("field-equals<") -> {
        val parts = splitTopLevelCommas(innerOf(rule, "field-equals<"))
        ParsedDiscriminatorRule.FieldEquals(unescapeGrammarLeaf(parts[0]), unescapeOptionalLeaf(parts[1]))
    }
    rule.startsWith("field-absent<") -> ParsedDiscriminatorRule.FieldAbsent(unescapeGrammarLeaf(innerOf(rule, "field-absent<")))
    else -> error("native lowerSchemaTypeBody: unrecognised discriminator rule '$rule'")
}

/** One lowered-union branch: `<tag,type,rule>`, wrapped so its own top-level commas stay local to the branch. */
internal data class ParsedUnionBranch(val tag: String, val type: String, val rule: String)

/** `union<<tag,type,rule>,...>` -> the branches, in declaration order. */
internal fun loweredUnionBranches(wit: String): List<ParsedUnionBranch> = splitTopLevelCommas(innerOf(wit, "union<")).map { branch ->
    val parts = splitTopLevelCommas(innerOf(branch, "<"))
    // The tag is a leaf that may hold any string (unlike the checked-identifier tag the lift
    // grammar's union<tag:type,...> carries), so it is escaped on the wire and must be
    // unescaped here; type and rule are returned raw for their own parsers to handle.
    ParsedUnionBranch(unescapeGrammarLeaf(parts[0]), parts[1], parts[2])
}
