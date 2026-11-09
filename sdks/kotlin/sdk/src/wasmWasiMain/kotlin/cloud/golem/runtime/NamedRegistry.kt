package cloud.golem.runtime

/** A registry of named entries: a name is registered once, looked up by name, and listed in registration order or sorted by name. */
internal class NamedRegistry<T>(private val kind: String, private val nameOf: (T) -> String, private val sorted: Boolean = false) {
    private val entries = LinkedHashMap<String, T>()

    fun register(entry: T) {
        val name = nameOf(entry)
        check(name !in entries) { "duplicate $kind registration '$name'" }
        entries[name] = entry
    }

    fun lookup(name: String): T? = entries[name]

    fun all(): List<T> = if (sorted) entries.entries.sortedBy { it.key }.map { it.value } else entries.values.toList()
}
