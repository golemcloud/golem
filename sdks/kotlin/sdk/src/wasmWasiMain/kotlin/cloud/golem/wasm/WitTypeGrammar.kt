package cloud.golem.wasm

// The witType-string grammar shared by the value lift (Lift.kt) and the schema-graph builder
// (cloud.golem.runtime.AgentTypeModel / ToolModel). A witType is one of:
//   primitives: bool, s8, s16, s32, s64, u8, u16, u32, u64, f32, f64, char, string
//   record<name0:T0,name1:T1,...>      (field names; body is positional at the value level)
//   variant<c0:T0,c1:_,...>            (case names; `_` = no payload)
//   enum  or  enum<c0,c1,...>          (case names; the value carries only a case index)
//   list<T>   option<T>   tuple<T0,T1,...>   map<K,V>   result<T,E>   (`_` = unit ok/err)
// Field/case NAMES matter only for the schema-graph; the value tree is positional, so lift drops
// them. Kotlin identifiers can't contain ',' '<' '>' or ':', so the split helpers are unambiguous.

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
    wit.startsWith("option<") && wit.endsWith(">") -> listOf(innerOf(wit, "option<"))
    wit.startsWith("tuple<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "tuple<"))
    wit.startsWith("map<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "map<"))
    wit.startsWith("result<") && wit.endsWith(">") -> splitTopLevelCommas(innerOf(wit, "result<")).filter { it != "_" }
    else -> emptyList()
}

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
