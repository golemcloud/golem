package cloud.golem.ksp

import com.google.devtools.ksp.getConstructors
import com.google.devtools.ksp.getDeclaredFunctions
import com.google.devtools.ksp.processing.KSPLogger
import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.symbol.ClassKind
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSType
import com.google.devtools.ksp.symbol.Modifier

private const val TYPED_BASE_FQN = "cloud.golem.runtime.TypedToolMiddleware"

/** Works out which tool a `@ToolMiddleware` class presents and wraps from its generated `XMiddleware` / `XAdapter<Y>` superclass. */
class TypedMiddlewareResolver(
    private val resolver: Resolver,
    private val logger: KSPLogger,
    private val definitionModel: (KSClassDeclaration) -> ToolDefinitionModel? = { buildToolDefinitionModel(it, logger) },
) {

    private fun fail(message: String, at: KSClassDeclaration): Nothing? {
        logger.error("GolemKSP: $message", at)
        return null
    }

    /** The binding of [decl], or `null` when it is not a typed middleware or its definition is unusable (the latter is reported). */
    fun resolve(decl: KSClassDeclaration): TypedMiddlewareBinding? {
        val superType = typedSuperType(decl) ?: return null
        if (decl.packageName.asString().isEmpty()) return fail("typed middleware ${decl.simpleName.asString()} must be declared in a package", decl)
        if (decl.typeParameters.isNotEmpty()) return fail("typed middleware '${decl.simpleName.asString()}' must not declare type parameters", decl)
        if (decl.getConstructors().none { c -> c.parameters.all { it.hasDefault } }) {
            return fail("typed middleware '${decl.simpleName.asString()}' must have a constructor that takes no arguments", decl)
        }
        val superDecl = superType.declaration as KSClassDeclaration
        val superName = superDecl.simpleName.asString()
        val className = decl.simpleName.asString()
        val adapter = superName.endsWith("Adapter")
        if (!adapter && !superName.endsWith("Middleware")) {
            return fail("$className extends $superName, which is not a generated <Tool>Middleware or <Tool>Adapter", decl)
        }
        val presentedDecl = definitionNamed(superDecl.packageName.asString(), superName.removeSuffix(if (adapter) "Adapter" else "Middleware"))
            ?: return fail("$className extends $superName, which is not generated from a @ToolDefinition interface", decl)
        val presented = definitionModel(presentedDecl) ?: return null
        if (!adapter) return TypedMiddlewareBinding(presented, null)

        val underlyingArg = superType.arguments.firstOrNull()?.type?.resolve()?.declaration as? KSClassDeclaration
        val underlyingName = underlyingArg?.simpleName?.asString()
        val expectedDecl = (
            if (underlyingArg != null && underlyingName != null && underlyingName.endsWith("Underlying")) {
                definitionNamed(underlyingArg.packageName.asString(), underlyingName.removeSuffix("Underlying"))
            } else {
                null
            }
            ) ?: return fail("adapter '$className' must name a generated <Tool>Underlying as the type argument of $superName", decl)
        val expected = definitionModel(expectedDecl) ?: return null

        val overrides = decl.getDeclaredFunctions().filter { Modifier.OVERRIDE in it.modifiers }.map { it.simpleName.asString() to it.parameters.size }.toSet()
        for (c in presented.flatCommands()) {
            if ((c.handlerName to (c.signature().size + 1)) !in overrides) {
                return fail("adapter '$className' does not override command '${c.handlerName}' of ${presented.interfaceName}", decl)
            }
        }
        val expectedPayloads = payloadsByError(expected)
        for ((name, payloads) in payloadsByError(presented)) {
            val inner = expectedPayloads[name] ?: continue
            if (payloads.intersect(inner).isEmpty()) {
                return fail(
                    "error '$name' of '${presented.name}' has payload ${payloads.joinToString(" | ")} but '${expected.name}' declares ${inner.joinToString(" | ")}",
                    decl,
                )
            }
        }
        return TypedMiddlewareBinding(presented, expected)
    }

    /** For each error name a tool declares, the payload types it is declared with across all its commands (`none` for no payload). */
    private fun payloadsByError(tool: ToolDefinitionModel): Map<String, Set<String>> = tool.flatCommands().flatMap { it.errorClass?.cases.orEmpty() }.groupBy({ it.kebabName }, { it.payloadWitType ?: "none" }).mapValues { it.value.toSet() }

    private fun definitionNamed(packageName: String, simpleName: String): KSClassDeclaration? {
        val name = if (packageName.isEmpty()) simpleName else "$packageName.$simpleName"
        return resolver.getClassDeclarationByName(resolver.getKSNameFromString(name))?.takeIf { it.isToolDefinition() }
    }

    companion object {
        /** The direct superclass of [decl] when it derives from the typed middleware base, else `null`. */
        fun typedSuperType(decl: KSClassDeclaration): KSType? = decl.superTypes.map { it.resolve() }.firstOrNull { t ->
            val d = t.declaration as? KSClassDeclaration ?: return@firstOrNull false
            d.classKind == ClassKind.CLASS &&
                Modifier.ABSTRACT in d.modifiers &&
                d.superTypes.any { it.resolve().declaration.qualifiedName?.asString() == TYPED_BASE_FQN }
        }

        fun isTyped(decl: KSClassDeclaration): Boolean = typedSuperType(decl) != null
    }
}
