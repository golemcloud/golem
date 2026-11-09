package cloud.golem.ksp

import com.google.devtools.ksp.symbol.KSDeclaration
import com.google.devtools.ksp.symbol.KSType

/** "CounterAgent" -> "counter-agent", "get_x" -> "get-x". */
internal fun toKebabCase(name: String): String = name.replace(Regex("([a-z])([A-Z])"), "$1-$2").replace('_', '-').lowercase()

/** Renders a [KSType] back to fully-qualified Kotlin source (handles generics + nullability). */
internal fun renderKotlinType(type: KSType): String {
    val fqn = type.declaration.qualifiedName?.asString() ?: type.declaration.simpleName.asString()
    val args = if (type.arguments.isEmpty()) {
        ""
    } else {
        "<" + type.arguments.joinToString(", ") { arg -> arg.type?.resolve()?.let { renderKotlinType(it) } ?: "*" } + ">"
    }
    return fqn + args + (if (type.isMarkedNullable) "?" else "")
}

/**
 * The prose of a declaration's KDoc, or null when it has none. Used as the last fallback for an agent's
 * or a method's description, as the Scala SDK uses the scaladoc (`Scaladoc.clean` in
 * `sdks/scala/macros/src/main/scala-3/golem/runtime/macros/Scaladoc.scala`).
 *
 * The rule is Scala's, so that the two SDKs describe the same source the same way: strip the comment
 * delimiters and each line's leading `*` margin, keep the prose up to the first tag line (`@param`,
 * `@return`, ...), drop blank lines at both ends, and join what is left with newlines. Every paragraph
 * is kept, not only the first sentence. A comment that holds nothing but tags yields null.
 */
internal fun kdocDescription(declaration: KSDeclaration): String? {
    val raw = declaration.docString ?: return null
    val body = raw.trim().removePrefix("/**").removeSuffix("*/")
    val prose = body.lines()
        .map { line -> line.trim().removePrefix("*").trim() }
        .takeWhile { line -> !isKdocTagLine(line) }
        .dropWhile { it.isEmpty() }
        .dropLastWhile { it.isEmpty() }
    return prose.joinToString("\n").takeIf { it.isNotEmpty() }
}

private fun isKdocTagLine(line: String): Boolean = line.length >= 2 && line.startsWith("@") && line[1].isLetter()
