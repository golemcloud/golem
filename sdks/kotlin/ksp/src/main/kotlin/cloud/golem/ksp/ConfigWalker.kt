package cloud.golem.ksp

import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSType
import com.google.devtools.ksp.symbol.Modifier

/**
 * Walks the type argument of an agent's `Config<T>` parameter into config declarations plus the Kotlin expression that builds
 * the value. A (non-null) data class contributes a path segment per field and is never a leaf; a `Secret<T>` is a secret leaf;
 * everything else the type mapper understands (primitives, lists, nullable types, enums, ...) is a local leaf.
 */
object ConfigWalker {
    private const val SECRET = "cloud.golem.config.Secret"

    fun walk(type: KSType, path: List<String>, declarations: MutableList<ConfigDeclModel>): String {
        val fqn = type.declaration.qualifiedName?.asString()
        if (fqn == SECRET) {
            val innerType = type.arguments.firstOrNull()?.type?.resolve() ?: throw UnsupportedTypeException("Secret needs a type argument (at ${path.joinToString(".")})")
            val inner = TypeMapper.resolve(innerType)
            declarations += ConfigDeclModel("secret", path, "secret<${inner.toWit()}>")
            return "cloud.golem.config.Secret(${listLit(path)}, ${lit(inner.toWit())}) { golemSv -> ${ConverterCodegen.decode(inner, "golemSv")} }"
        }
        val decl = type.declaration as? KSClassDeclaration
        if (decl != null && isConfigRecord(decl, fqn, type)) {
            val ctor = decl.primaryConstructor ?: throw UnsupportedTypeException("$fqn has no primary constructor")
            val args = ctor.parameters.joinToString(", ") { p ->
                val name = p.name?.asString() ?: throw UnsupportedTypeException("unnamed field in $fqn")
                "$name = ${walk(p.type.resolve(), path + name, declarations)}"
            }
            return "$fqn($args)"
        }
        val td = TypeMapper.resolve(type)
        declarations += ConfigDeclModel("local", path, td.toWit())
        return "cloud.golem.config.ConfigRuntime.local(${listLit(path)}, ${lit(td.toWit())}).let { golemSv -> ${ConverterCodegen.decode(td, "golemSv")} }"
    }

    private fun isConfigRecord(decl: KSClassDeclaration, fqn: String?, type: KSType): Boolean = Modifier.DATA in decl.modifiers &&
        !type.isMarkedNullable &&
        fqn != null &&
        !fqn.startsWith("kotlin.") &&
        !fqn.startsWith("cloud.golem.")

    private fun lit(s: String): String = kotlinStringLiteral(s)

    private fun listLit(path: List<String>): String = if (path.isEmpty()) "emptyList<String>()" else "listOf(${path.joinToString(", ") { lit(it) }})"
}
