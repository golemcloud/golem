package cloud.golem.ksp

import com.google.devtools.ksp.getDeclaredFunctions
import com.google.devtools.ksp.getDeclaredProperties
import com.google.devtools.ksp.processing.KSPLogger
import com.google.devtools.ksp.symbol.ClassKind
import com.google.devtools.ksp.symbol.KSAnnotated
import com.google.devtools.ksp.symbol.KSAnnotation
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSFunctionDeclaration
import com.google.devtools.ksp.symbol.KSType
import com.google.devtools.ksp.symbol.Modifier

internal const val TOOL_DEFINITION_FQN = "cloud.golem.annotations.ToolDefinition"
private const val TOOL_ERROR_FQN = "cloud.golem.annotations.ToolError"
private const val PRINCIPAL_FQN = "cloud.golem.Principal"
private const val EITHER_FQN = "cloud.golem.runtime.Either"

private val kebabName = Regex("^[a-z][a-z0-9]*(-[a-z0-9]+)*$")
private val identifier = Regex("^[A-Za-z_][A-Za-z0-9_]*$")
private val GENERATED_SUFFIXES = listOf("Definition", "Client", "Underlying", "Middleware", "Adapter")

/** A `@ToolDefinition` interface: its direct commands, its nested command groups, and the metadata of the tool. */
data class ToolDefinitionModel(
    val name: String,
    val version: String,
    val summary: String,
    val requiresFilesystem: Boolean,
    val packageName: String,
    val interfaceName: String,
    val qualifiedName: String,
    /** The commands declared directly on the interface (path = the kebab-case method name). */
    val commands: List<ToolCommandModel>,
    /** The no-parameter methods that return another `@ToolDefinition` interface. */
    val groups: List<ToolGroupModel>,
) {
    /** Every runnable command of the tool, nested groups included; paths and handler names are relative to this tool's root. */
    fun flatCommands(): List<ToolCommandModel> = commands + groups.flatMap { g ->
        g.child.flatCommands().map { c ->
            c.copy(
                path = listOf(g.kebabName) + c.path,
                handlerName = g.methodName + c.handlerName.replaceFirstChar { it.uppercase() },
                groupChain = listOf(g.methodName) + c.groupChain,
                globalArgs = g.canonicalArgs + c.globalArgs,
                groupArgs = listOf(g.args) + c.groupArgs,
            )
        }
    }

    /** The distinct error classes of the commands declared directly on this interface, by qualified name. */
    fun directErrorClasses(): List<ToolErrorClassModel> = commands.mapNotNull { it.errorClass }.distinctBy { it.qualifiedName }

    /** The name of the generated `object` holding this tool's definition and error codecs. */
    val definitionFqn: String get() = "$packageName.${interfaceName}Definition"
}

/** A group method; its [params] (options and flags only) are globals of every command below it. [args] are in declaration order. */
data class ToolGroupModel(
    val methodName: String,
    val kebabName: String,
    val child: ToolDefinitionModel,
    val params: List<ParamModel> = emptyList(),
    val paramKotlinTypes: List<String> = emptyList(),
    val args: List<ToolArgModel> = emptyList(),
    val description: String = "",
    val aliases: List<String> = emptyList(),
) {
    /** The globals the group contributes in canonical input order: options before flags. */
    val canonicalArgs: List<ToolArgModel> get() = args.filter { !it.isFlag } + args.filter { it.isFlag }
}

/**
 * One command. [params] are the positionals; the `Principal` parameter (if any) is not among them and sits at [principalIndex] of the method's
 * full parameter list.
 */
data class ToolCommandModel(
    val path: List<String>,
    /** The method name on the interface that declares the command. */
    val methodName: String,
    /** The name of the middleware handler: [methodName], or for a command inside a group (see [ToolDefinitionModel.flatCommands]) the camel-case join of the path. */
    val handlerName: String,
    /** The method names of the nested groups leading to the command (empty for a direct command). */
    val groupChain: List<String> = emptyList(),
    /** The generated definition `object` that declares the command and holds its error codecs. */
    val definitionFqn: String,
    val params: List<ParamModel>,
    /** Rendered Kotlin type of each of [params]. */
    val paramKotlinTypes: List<String>,
    val principalName: String?,
    /** Rendered Kotlin type of the `Principal` parameter, nullability included. */
    val principalType: String?,
    val principalIndex: Int,
    val outputTypeDesc: TypeDesc,
    /** Rendered Kotlin type of the success value (`kotlin.Unit` for none). */
    val outputKotlinType: String,
    val errorClass: ToolErrorClassModel?,
    /** One entry per [params], in declaration order. */
    val args: List<ToolArgModel> = emptyList(),
    /** The globals inherited from enclosing command groups, in canonical order (outermost group first; options before flags per group). */
    val globalArgs: List<ToolArgModel> = emptyList(),
    /** The arguments of each enclosing group, outermost first and in declaration order; parallel to [groupChain]. */
    val groupArgs: List<List<ToolArgModel>> = emptyList(),
    val description: String = "",
    val aliases: List<String> = emptyList(),
    val formatters: List<String> = emptyList(),
    val defaultFormatter: String = "default",
    val examples: List<Pair<String, String>> = emptyList(),
    val hints: ToolHintsModel? = null,
    val constraints: List<ToolConstraintModel> = emptyList(),
    val stdin: Boolean = false,
    val stdout: Boolean = false,
    val stderr: Boolean = false,
) {
    /** True when a call returns its value together with the bytes the command wrote (`WithOutputs`). */
    val hasOutputs: Boolean get() = stdout || stderr

    /**
     * Every argument of the invocation input record in the order the host and every client agree on: inherited globals, fixed positionals, the
     * tail, options, flags.
     */
    val canonicalArgs: List<ToolArgModel> get() = globalArgs + canonicalOwn()

    /** The command's own arguments in canonical order, without the inherited globals. */
    fun canonicalOwn(): List<ToolArgModel> = args.filter { it.kind == ToolArgKind.POSITIONAL } + args.filter { it.kind == ToolArgKind.TAIL } +
        args.filter { it.kind == ToolArgKind.OPTION } + args.filter { it.isFlag }

    val inputWitType: String get() = TypeDesc.Record("", canonicalArgs.map { Field(it.name, it.typeDesc) }).toWit()

    /** The handler's parameters: the arguments of the enclosing groups, then the method's parameters in declaration order, `Principal` included. */
    fun signature(): List<Pair<String, String>> = groupArgs.flatten().map { it.paramName to it.kotlinType } + ownSignature()

    /** The method's own parameters in declaration order as (name, rendered type), `Principal` included. */
    fun ownSignature(): List<Pair<String, String>> {
        val principal = principalName?.let { name ->
            name to checkNotNull(principalType) { "a principal parameter carries a rendered type" }
        }
        val out = ArrayList<Pair<String, String>>()
        var next = 0
        for (i in 0 until params.size + (if (principal != null) 1 else 0)) {
            if (principal != null && i == principalIndex) {
                out.add(principal)
            } else {
                out.add(params[next].name to paramKotlinTypes[next])
                next++
            }
        }
        return out
    }
}

/** The sealed class of declared errors of a command; every direct subclass is a case. */
data class ToolErrorClassModel(val qualifiedName: String, val simpleName: String, val cases: List<ToolErrorCaseModel>)

/** One error case; [fields] are the constructor properties of the subclass (empty for an object or a class without properties). A case with one property carries that property's value as the payload. */
data class ToolErrorCaseModel(
    val kebabName: String,
    val className: String,
    val isObject: Boolean,
    val fields: List<ParamModel>,
    val usage: Boolean,
    val exitCode: Int,
    val description: String,
) {
    /**
     * `null` for a payloadless case; the property's own type for a case with one property (the convention of the Rust SDK, so a Kotlin
     * definition and a Rust implementation of one tool agree); a record of the properties otherwise.
     */
    val payloadTypeDesc: TypeDesc?
        get() = when (fields.size) {
            0 -> null
            1 -> fields[0].typeDesc
            else -> TypeDesc.Record(className, fields.map { Field(it.name, it.typeDesc) })
        }
    val payloadWitType: String? get() = payloadTypeDesc?.toWit()
}

private fun contains(td: TypeDesc, test: (TypeDesc) -> Boolean): Boolean = test(td) ||
    when (td) {
        is TypeDesc.Record -> td.fields.any { contains(it.type, test) }
        is TypeDesc.ListT -> contains(td.elem, test)
        is TypeDesc.OptionT -> contains(td.inner, test)
        is TypeDesc.MapT -> contains(td.key, test) || contains(td.value, test)
        is TypeDesc.TupleT -> td.elems.any { contains(it, test) }
        is TypeDesc.ResultT -> contains(td.ok, test) || contains(td.err, test)
        else -> false
    }

private fun containsVariant(td: TypeDesc): Boolean = contains(td) { it is TypeDesc.VariantT }

private fun containsUnit(td: TypeDesc): Boolean = contains(td) { it is TypeDesc.UnitT }

internal fun KSAnnotated.annotationByFqn(fqn: String): KSAnnotation? = annotations.firstOrNull {
    it.annotationType.resolve().declaration.qualifiedName?.asString() == fqn
}

internal fun KSAnnotation.arg(name: String): Any? = arguments.firstOrNull { it.name?.asString() == name }?.value

internal fun KSClassDeclaration.isToolDefinition(): Boolean = annotationByFqn(TOOL_DEFINITION_FQN) != null

/** Packages that generated code names by their fully qualified path; a parameter of the same name would shadow them. */
private val GENERATED_PACKAGE_ROOTS = setOf("cloud", "kotlin")

/**
 * Builds the model of a `@ToolDefinition` interface, from source or from a compiled dependency. Reports problems through [logger] with a
 * `GolemKSP:` prefix and returns `null` when the definition is unusable.
 */
fun buildToolDefinitionModel(
    decl: KSClassDeclaration,
    logger: KSPLogger,
    declarationExists: (qualifiedName: String) -> Boolean = { false },
): ToolDefinitionModel? = ToolDefinitionBuilder(logger, declarationExists).build(decl, emptySet(), emptyList())

private class ToolDefinitionBuilder(val logger: KSPLogger, val declarationExists: (String) -> Boolean) {
    var failed = false
    val arguments = ToolArgumentParser { message, at -> fail(message, at) }

    fun fail(message: String, at: KSAnnotated?): Nothing? {
        failed = true
        logger.error("GolemKSP: $message", at)
        return null
    }

    fun build(decl: KSClassDeclaration, visiting: Set<String>, inherited: List<ToolArgModel>): ToolDefinitionModel? {
        val fqn = decl.qualifiedName?.asString() ?: return fail("a @ToolDefinition cannot be anonymous", decl)
        val simple = decl.simpleName.asString()
        val annotation = decl.annotationByFqn(TOOL_DEFINITION_FQN) ?: return fail("$simple is not annotated with @ToolDefinition", decl)
        if (decl.classKind != ClassKind.INTERFACE) return fail("@ToolDefinition $simple must be an interface", decl)
        if (fqn in visiting) return fail("$simple is part of a cycle of nested command groups", decl)
        val name = annotation.arg("name") as? String ?: ""
        if (name.isEmpty()) return fail("@ToolDefinition on $simple is missing a name", decl)
        if (!kebabName.matches(name)) return fail("@ToolDefinition name '$name' on $simple must be lowercase kebab-case (e.g. 'public-echo')", decl)
        if ((annotation.arg("version") as? String ?: "0.1.0").isEmpty()) return fail("@ToolDefinition on $simple has an empty version", decl)

        val packageName = decl.packageName.asString()
        if (packageName.isEmpty()) return fail("@ToolDefinition $simple must be declared in a package", decl)
        if (decl.parentDeclaration != null) return fail("@ToolDefinition $simple must be a top-level interface", decl)
        if (visiting.isEmpty()) {
            for (suffix in GENERATED_SUFFIXES) {
                val clash = "$simple$suffix"
                if (declarationExists("$packageName.$clash")) {
                    return fail("package '$packageName' already declares '$clash', which is generated for @ToolDefinition $simple", decl)
                }
            }
        }
        if (decl.getDeclaredProperties().any()) return fail("@ToolDefinition $simple declares properties; properties are not supported, only functions", decl)
        if (decl.superTypes.any { it.resolve().declaration.qualifiedName?.asString() != "kotlin.Any" }) {
            return fail("@ToolDefinition $simple must not extend other interfaces; declare every command on $simple itself", decl)
        }

        val commands = ArrayList<ToolCommandModel>()
        val groups = ArrayList<ToolGroupModel>()
        val seenKebab = HashMap<String, String>()
        var ok = true
        for (fn in decl.getDeclaredFunctions()) {
            val method = fn.simpleName.asString()
            val unsupported = when {
                !fn.isAbstract -> "has a default body (every method of a tool is a command and must be abstract)"
                Modifier.SUSPEND in fn.modifiers -> "is a suspend function (commands are synchronous)"
                fn.typeParameters.isNotEmpty() -> "declares type parameters"
                fn.extensionReceiver != null -> "is an extension function"
                !identifier.matches(method) -> "is not a plain identifier"
                else -> null
            }
            if (unsupported != null) {
                fail("$simple.$method $unsupported", fn)
                ok = false
                continue
            }
            // `@ToolCommand(name = ...)` names the command, as Scala's `command(name = ...)` does; without it the
            // method name in kebab-case is the name. On a group method this also names the grafted child root,
            // because the resolved name is what buildGroup grafts under.
            val declaredName = fn.annotations
                .firstOrNull { it.annotationType.resolve().declaration.qualifiedName?.asString() == "cloud.golem.annotations.ToolCommand" }
                ?.arg("name") as? String
            val kebab = declaredName?.takeIf { it.isNotEmpty() } ?: toKebabCase(method)
            if (!kebabName.matches(kebab)) {
                fail("$simple.$method does not map to a valid command name ('$kebab' is not lowercase kebab-case)", fn)
                ok = false
                continue
            }
            seenKebab.put(kebab, method)?.let { first ->
                fail("$simple.$method and $simple.$first both map to the command name '$kebab'", fn)
                ok = false
                continue
            }
            val returnType = fn.returnType?.resolve()
            if (returnType == null) {
                fail("$simple.$method has no return type", fn)
                ok = false
                continue
            }
            val returnDecl = returnType.declaration as? KSClassDeclaration
            if (returnDecl != null && returnDecl.isToolDefinition() && !returnType.isMarkedNullable) {
                val group = buildGroup(simple, fn, kebab, inherited) { globals -> build(returnDecl, visiting + fqn, inherited + globals) }
                if (group == null) {
                    ok = false
                    continue
                }
                groups.add(group)
            } else {
                val command = buildCommand(simple, fn, kebab, returnType, "${decl.packageName.asString()}.${simple}Definition", inherited)
                if (command == null) {
                    ok = false
                    continue
                }
                commands.add(command)
            }
        }
        if (!ok || failed) return null
        for (c in commands) {
            val clash = commands.firstOrNull { "start" + it.methodName.replaceFirstChar { ch -> ch.uppercase() } == c.methodName }
            if (clash != null) return fail("$simple.${c.methodName} clashes with the generated start method of $simple.${clash.methodName}", decl)
        }
        commands.mapNotNull { it.errorClass }.distinctBy { it.qualifiedName }.groupBy { it.simpleName }.values.firstOrNull { it.size > 1 }?.let {
            return fail("$simple has commands whose error classes share the simple name '${it.first().simpleName}'", decl)
        }
        val result = ToolDefinitionModel(
            name = name,
            version = annotation.arg("version") as? String ?: "0.1.0",
            summary = annotation.arg("summary") as? String ?: "",
            requiresFilesystem = annotation.arg("requiresFilesystem") as? Boolean ?: false,
            packageName = decl.packageName.asString(),
            interfaceName = simple,
            qualifiedName = fqn,
            commands = commands,
            groups = groups,
        )
        result.flatCommands().groupBy { it.handlerName }.values.firstOrNull { it.size > 1 }?.let {
            return fail("$simple has commands ${it.joinToString { c -> c.path.joinToString("/") }} that map to the same handler name '${it.first().handlerName}'", decl)
        }
        return result
    }

    fun buildGroup(owner: String, fn: KSFunctionDeclaration, kebab: String, inherited: List<ToolArgModel>, buildChild: (List<ToolArgModel>) -> ToolDefinitionModel?): ToolGroupModel? {
        val method = fn.simpleName.asString()
        val params = ArrayList<ParamModel>()
        val types = ArrayList<String>()
        val args = ArrayList<ToolArgModel>()
        for (p in fn.parameters) {
            val pName = p.name?.asString() ?: return fail("$owner.$method has an unnamed parameter", fn)
            val pType = p.type.resolve()
            if (pName.startsWith("golem")) return fail("$owner.$method parameter '$pName' uses the reserved prefix 'golem' of generated names", fn)
            if (pName == "underlying") return fail("$owner.$method parameter 'underlying' clashes with the generated handler parameter of that name", fn)
            if (pName in GENERATED_PACKAGE_ROOTS) return fail("$owner.$method parameter '$pName' shadows the package '$pName' that generated code refers to", fn)
            if (!kebabName.matches(toKebabCase(pName))) return fail("$owner.$method parameter '$pName' does not map to a valid option name ('${toKebabCase(pName)}')", fn)
            if (pType.declaration.qualifiedName?.asString() == PRINCIPAL_FQN) return fail("$owner.$method: a command group cannot take a Principal", fn)
            val desc = try {
                TypeMapper.resolve(pType)
            } catch (e: UnsupportedTypeException) {
                return fail("$owner.$method parameter '$pName' has an unsupported type (${e.message})", fn)
            }
            if (containsUnit(desc) || containsVariant(desc)) return fail("$owner.$method parameter '$pName' has a type that cannot be a tool argument", fn)
            val arg = arguments.parseArg("$owner.$method", p, pName, desc, renderKotlinType(pType), fn, allowedInGroup = true) ?: return null
            params.add(ParamModel(pName, desc))
            types.add(renderKotlinType(pType))
            args.add(arg)
        }
        if (!arguments.validateSet("$owner.$method", args, inherited, fn)) return null
        val canonical = args.filter { !it.isFlag } + args.filter { it.isFlag }
        val child = buildChild(canonical) ?: return null
        val command = fn.annotations.firstOrNull { it.annotationType.resolve().declaration.qualifiedName?.asString() == "cloud.golem.annotations.ToolCommand" }
        return ToolGroupModel(
            method,
            kebab,
            child,
            params,
            types,
            args,
            description = command?.arg("description") as? String ?: "",
            aliases = (command?.arg("aliases") as? List<*>)?.map { it.toString() } ?: emptyList(),
        )
    }

    fun buildCommand(owner: String, fn: KSFunctionDeclaration, kebab: String, returnType: KSType, definitionFqn: String, inherited: List<ToolArgModel>): ToolCommandModel? {
        val method = fn.simpleName.asString()
        val params = ArrayList<ParamModel>()
        val types = ArrayList<String>()
        val args = ArrayList<ToolArgModel>()
        var principalName: String? = null
        var principalType: String? = null
        var principalIndex = -1
        for ((i, p) in fn.parameters.withIndex()) {
            val pName = p.name?.asString() ?: return fail("$owner.$method has an unnamed parameter", fn)
            val pType = p.type.resolve()
            if (pName.startsWith("golem")) return fail("$owner.$method parameter '$pName' uses the reserved prefix 'golem' of generated names", fn)
            if (pName == "underlying") return fail("$owner.$method parameter 'underlying' clashes with the generated handler parameter of that name", fn)
            if (pName in GENERATED_PACKAGE_ROOTS) return fail("$owner.$method parameter '$pName' shadows the package '$pName' that generated code refers to", fn)
            if (pType.declaration.qualifiedName?.asString() == PRINCIPAL_FQN) {
                if (principalName != null) return fail("$owner.$method declares more than one Principal parameter", fn)
                principalName = pName
                principalType = renderKotlinType(pType)
                principalIndex = i
                continue
            }
            val desc = try {
                TypeMapper.resolve(pType)
            } catch (e: UnsupportedTypeException) {
                val why = if (pType.declaration.qualifiedName == null || pType.declaration.simpleName.asString().startsWith("Function")) {
                    "function types cannot be tool arguments"
                } else {
                    e.message
                }
                return fail("$owner.$method parameter '$pName' has an unsupported type ($why)", fn)
            }
            if (containsUnit(desc)) return fail("$owner.$method parameter '$pName': Unit cannot be a tool argument", fn)
            if (!kebabName.matches(toKebabCase(pName))) {
                return fail("$owner.$method parameter '$pName' does not map to a valid positional name ('${toKebabCase(pName)}')", fn)
            }
            if (containsVariant(desc)) {
                return fail("$owner.$method parameter '$pName' contains a sealed type (variants cannot appear in tool input)", fn)
            }
            if (params.any { toKebabCase(it.name) == toKebabCase(pName) }) {
                return fail("$owner.$method parameters '${params.first { toKebabCase(it.name) == toKebabCase(pName) }.name}' and '$pName' both map to the positional name '${toKebabCase(pName)}'", fn)
            }
            args.add(arguments.parseArg("$owner.$method", p, pName, desc, renderKotlinType(pType), fn, allowedInGroup = false) ?: return null)
            params.add(ParamModel(pName, desc))
            types.add(renderKotlinType(pType))
        }
        if (!arguments.validateSet("$owner.$method", args, inherited, fn)) return null

        var successType = returnType
        var errorClass: ToolErrorClassModel? = null
        if (returnType.declaration.qualifiedName?.asString() == EITHER_FQN) {
            val left = returnType.arguments[0].type?.resolve() ?: return fail("$owner.$method has an unresolvable Either error type", fn)
            successType = returnType.arguments[1].type?.resolve() ?: return fail("$owner.$method has an unresolvable Either result type", fn)
            errorClass = buildErrorClass(owner, method, left, fn) ?: return null
        }
        val outputDesc = try {
            TypeMapper.resolve(successType)
        } catch (e: UnsupportedTypeException) {
            return fail("$owner.$method has an unsupported result type (${e.message})", fn)
        }
        val metadata = arguments.parseMetadata("$owner.$method", fn, args, inherited) ?: return null
        if (metadata.stdin && (params.any { it.name == "stdin" } || inherited.any { it.paramName == "stdin" })) {
            return fail("$owner.$method parameter 'stdin' clashes with the generated stdin parameter of a command that declares stdin", fn)
        }
        return ToolCommandModel(
            path = listOf(kebab),
            methodName = method,
            handlerName = method,
            definitionFqn = definitionFqn,
            params = params,
            paramKotlinTypes = types,
            principalName = principalName,
            principalType = principalType,
            principalIndex = principalIndex,
            outputTypeDesc = outputDesc,
            outputKotlinType = renderKotlinType(successType),
            errorClass = errorClass,
            args = args,
            description = metadata.description,
            aliases = metadata.aliases,
            formatters = metadata.formatters,
            defaultFormatter = metadata.defaultFormatter,
            examples = metadata.examples,
            hints = metadata.hints,
            constraints = metadata.constraints,
            stdin = metadata.stdin,
            stdout = metadata.stdout,
            stderr = metadata.stderr,
        )
    }

    fun buildErrorClass(owner: String, method: String, type: KSType, at: KSFunctionDeclaration): ToolErrorClassModel? {
        val decl = type.declaration as? KSClassDeclaration
        if (decl == null || Modifier.SEALED !in decl.modifiers) {
            return fail("$owner.$method error type ${renderKotlinType(type)} must be a sealed class", at)
        }
        val fqn = decl.qualifiedName?.asString() ?: return fail("$owner.$method error type has no qualified name", at)
        val subclasses = decl.getSealedSubclasses().toList()
        if (subclasses.isEmpty()) return fail("error class ${decl.simpleName.asString()} of $owner.$method has no cases", at)
        val cases = ArrayList<ToolErrorCaseModel>()
        val seen = HashSet<String>()
        for (sub in subclasses) {
            val subFqn = sub.qualifiedName?.asString() ?: return fail("error case of ${decl.simpleName.asString()} has no qualified name", at)
            val kebab = toKebabCase(sub.simpleName.asString())
            if (!kebabName.matches(kebab)) return fail("error case ${sub.simpleName.asString()} does not map to a valid error name ('$kebab')", at)
            if (!seen.add(kebab)) return fail("error class ${decl.simpleName.asString()} has two cases named '$kebab'", at)
            val annotation = sub.annotationByFqn(TOOL_ERROR_FQN)
            val exitCode = annotation?.arg("exitCode") as? Int ?: 1
            if (exitCode !in 0..255) return fail("error case ${sub.simpleName.asString()} exitCode $exitCode is outside 0..255", at)
            val isObject = sub.classKind == ClassKind.OBJECT
            if (Modifier.SEALED in sub.modifiers) return fail("error case ${sub.simpleName.asString()} of ${decl.simpleName.asString()} must not be a sealed class", at)
            if (!isObject && sub.primaryConstructor?.parameters?.any { !it.isVal && !it.isVar } == true) {
                return fail("error case ${sub.simpleName.asString()} must declare its constructor parameters as val properties", at)
            }
            val fields = if (isObject) {
                emptyList()
            } else {
                (sub.primaryConstructor?.parameters ?: emptyList()).map { p ->
                    val pName = p.name?.asString() ?: return fail("error case ${sub.simpleName.asString()} has an unnamed property", at)
                    val desc = try {
                        TypeMapper.resolve(p.type.resolve())
                    } catch (e: UnsupportedTypeException) {
                        return fail("error case ${sub.simpleName.asString()} property '$pName' has an unsupported type (${e.message})", at)
                    }
                    ParamModel(pName, desc)
                }
            }
            cases.add(
                ToolErrorCaseModel(
                    kebabName = kebab,
                    className = subFqn,
                    isObject = isObject,
                    fields = fields,
                    usage = enumName(annotation?.arg("kind")) == "USAGE",
                    exitCode = exitCode,
                    description = annotation?.arg("description") as? String ?: "",
                ),
            )
        }
        return ToolErrorClassModel(fqn, decl.simpleName.asString(), cases)
    }

    /** The entry name of an enum annotation argument (KSP delivers a type or a declaration, depending on where the annotation was read). */
    fun enumName(value: Any?): String? = when (value) {
        is KSType -> value.declaration.simpleName.asString()
        is KSClassDeclaration -> value.simpleName.asString()
        null -> null
        else -> value.toString().substringAfterLast('.')
    }
}
