package cloud.golem.ksp

import com.google.devtools.ksp.getConstructors
import com.google.devtools.ksp.processing.CodeGenerator
import com.google.devtools.ksp.processing.KSPLogger
import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.processing.SymbolProcessor
import com.google.devtools.ksp.symbol.ClassKind
import com.google.devtools.ksp.symbol.KSAnnotated
import com.google.devtools.ksp.symbol.KSAnnotation
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSFunctionDeclaration
import com.google.devtools.ksp.symbol.KSType
import com.google.devtools.ksp.symbol.KSValueParameter
import com.google.devtools.ksp.symbol.Modifier
import com.google.devtools.ksp.symbol.Origin
import com.google.devtools.ksp.validate

private const val ARENA_HEADROOM_OPTION = "golem.arenaHeadroomKiB"
private const val MIN_HEADROOM_KIB = 64
private const val MAX_HEADROOM_KIB = 16384
private const val BYTES_PER_KIB = 1024

class GolemAgentProcessor(
    private val codeGenerator: CodeGenerator,
    private val logger: KSPLogger,
    private val options: Map<String, String> = emptyMap(),
) : SymbolProcessor {

    private val agentFqn = "cloud.golem.annotations.Agent"
    private val remoteAgentFqn = "cloud.golem.annotations.RemoteAgent"
    private val toolFqn = "cloud.golem.annotations.Tool"
    private val routerFqn = "cloud.golem.annotations.HttpRouter"
    private val middlewareFqn = "cloud.golem.annotations.ToolMiddleware"
    private val implementationFqn = "cloud.golem.annotations.ToolImplementation"

    private val collectedModels = mutableListOf<AgentModel>()
    private val collectedTools = mutableListOf<ToolModel>()
    private val collectedMiddlewares = mutableListOf<ToolMiddlewareModel>()
    private val collectedProviders = mutableListOf<ToolProviderModel>()

    // A definition is built, and its problems reported, once, whichever of the definition or a middleware reaches it first.
    private val definitionModels = HashMap<String, ToolDefinitionModel?>()

    private fun definitionModel(decl: KSClassDeclaration, declarationExists: (String) -> Boolean = { false }): ToolDefinitionModel? {
        val key = decl.qualifiedName?.asString() ?: decl.simpleName.asString()
        if (!definitionModels.containsKey(key)) definitionModels[key] = buildToolDefinitionModel(decl, logger, declarationExists)
        return definitionModels[key]
    }

    /**
     * The arena headroom the component asked for with the `golem.arenaHeadroomKiB` option, in bytes, or null for the SDK's default.
     * It has to be a whole number of 64 KiB pages from 64 KiB to 16 MiB, as `configureArenaHeadroom` requires.
     */
    private fun arenaHeadroomBytes(): Int? = options[ARENA_HEADROOM_OPTION]?.let { raw ->
        val kib = raw.trim().toIntOrNull()
        if (kib != null && kib in MIN_HEADROOM_KIB..MAX_HEADROOM_KIB && kib % MIN_HEADROOM_KIB == 0) {
            kib * BYTES_PER_KIB
        } else {
            logger.error("GolemKSP: $ARENA_HEADROOM_OPTION must be a multiple of $MIN_HEADROOM_KIB from $MIN_HEADROOM_KIB to $MAX_HEADROOM_KIB, got '$raw'")
            null
        }
    }

    override fun finish() {
        val headroom = arenaHeadroomBytes()
        val models = collectedModels
        val tools = collectedTools
        val middlewares = collectedMiddlewares
        val providers = collectedProviders
        if (models.isEmpty() && tools.isEmpty() && middlewares.isEmpty() && providers.isEmpty()) return
        val duplicateProvider = providers.groupBy { it.definition.name }.entries.firstOrNull { it.value.size > 1 }
        if (duplicateProvider != null) logger.error("GolemKSP: duplicate @ToolImplementation of tool '${duplicateProvider.key}'")
        val clashing = providers.map { it.definition.name }.intersect(tools.map { it.name }.toSet()).firstOrNull()
        if (clashing != null) logger.error("GolemKSP: tool '$clashing' is both a @Tool function and a @ToolImplementation")
        val duplicateTool = tools.groupBy { it.name }.entries.firstOrNull { it.value.size > 1 }
        if (duplicateTool != null) logger.error("GolemKSP: duplicate @Tool name '${duplicateTool.key}'")
        // Mirrors the JS-path processor: all @Agent classes and @Tool functions must currently share one package so the
        // generated entry point can call every register<Class>() via a single fully-qualified reference set.
        val distinctPackages = (models.map { it.packageName } + tools.map { it.packageName } + middlewares.map { it.packageName } + providers.map { it.packageName }).distinct()
        if (distinctPackages.size > 1) {
            logger.error(
                "GolemKSP: All @Agent classes and @Tool functions must currently share one package so the " +
                    "generated native registration entry point can see them; " +
                    "multi-package support is not yet available. " +
                    "Found packages: ${distinctPackages.joinToString()}",
            )
        } else {
            // A single entry point that registers every @Agent and declares the real @WasmExport golem:agent/guest@2.0.0
            // functions (see NativeRegistrationEmitter's doc comment for why registration is triggered from here).
            NativeRegistrationEmitter(codeGenerator).emitEntryPoint(models, tools, middlewares, providers, headroom)
        }
    }

    override fun process(resolver: Resolver): List<KSAnnotated> {
        val (valid, deferred) = resolver
            .getSymbolsWithAnnotation(agentFqn)
            .partition { it.validate() }

        // @RemoteAgent interfaces -> typed RPC clients (independent of @Agent registration).
        val (rpcValid, rpcDeferred) = resolver
            .getSymbolsWithAnnotation(remoteAgentFqn)
            .partition { it.validate() }
        rpcValid.filterIsInstance<KSClassDeclaration>().forEach { iface ->
            val typeName = iface.annotation("RemoteAgent")?.arg("typeName") as? String
                ?: error("@RemoteAgent on ${iface.simpleName.asString()} is missing typeName")
            logger.info("GolemKSP: generating RPC client for ${iface.qualifiedName?.asString()}")
            RemoteAgentEmitter(codeGenerator).emit(iface, typeName)
        }

        // @ToolDefinition interfaces -> typed clients, underlying handles and middleware base classes.
        val (definitionValid, definitionDeferred) = resolver
            .getSymbolsWithAnnotation(TOOL_DEFINITION_FQN)
            .partition { it.validate() }
        definitionValid.filterIsInstance<KSClassDeclaration>().forEach { iface ->
            definitionModel(iface) { resolver.getClassDeclarationByName(resolver.getKSNameFromString(it)) != null }?.let { model ->
                logger.info("GolemKSP: generating typed tool surface for ${model.qualifiedName}")
                ToolDefinitionEmitter(codeGenerator).emit(model)
            }
        }

        val (toolValid, toolDeferred) = resolver
            .getSymbolsWithAnnotation(toolFqn)
            .partition { it.validate() }
        val tools = toolValid
            .filterIsInstance<KSFunctionDeclaration>()
            .mapNotNull { fn -> buildToolModel(fn) }
            .toList()
        val (middlewareValid, middlewareDeferred) = resolver
            .getSymbolsWithAnnotation(middlewareFqn)
            .partition { it.validate() }
        collectedMiddlewares += middlewareValid.filterIsInstance<KSClassDeclaration>().mapNotNull { decl -> buildMiddlewareModel(decl, resolver) }
        reportUnannotatedMiddlewares(resolver)
        val (implementationValid, implementationDeferred) = resolver
            .getSymbolsWithAnnotation(implementationFqn)
            .partition { it.validate() }
        collectedProviders += implementationValid.filterIsInstance<KSClassDeclaration>().mapNotNull { decl -> buildProviderModel(decl) }
        val duplicateMiddleware = collectedMiddlewares.groupBy { it.name }.entries.firstOrNull { it.value.size > 1 }
        if (duplicateMiddleware != null) logger.error("GolemKSP: duplicate @ToolMiddleware name '${duplicateMiddleware.key}'")

        val (routerValid, routerDeferred) = resolver
            .getSymbolsWithAnnotation(routerFqn)
            .partition { it.validate() }

        val models = (valid.filterIsInstance<KSClassDeclaration>() + routerValid.filterIsInstance<KSClassDeclaration>())
            .map { classDecl ->
                val model = if (classDecl.annotationByFqn(routerFqn) != null) buildRouterModel(classDecl) else buildAgentModel(classDecl)
                validateHttpAgent(model, classDecl)
                logger.info("GolemKSP: processing ${model.qualifiedName}")
                HttpValidation.validate(model)?.let { logger.error(it, classDecl) }
                if (model.mode == "ephemeral" && model.methods.any { it.readOnlyCache != null }) {
                    logger.error(
                        "${model.className}: read-only methods have no effect on ephemeral agents (no shared state to read). Remove @ReadOnly or make the agent durable.",
                        classDecl,
                    )
                }
                NativeRegistrationEmitter(codeGenerator).emit(model)
                WitEmitter(codeGenerator).emit(model)
                model
            }
            .toList()

        // An agent that uses a generated RPC client (or a tool) is only resolvable in a later KSP round, so models arrive over
        // several rounds. The single entry point is emitted once, from finish(), with everything collected.
        collectedModels += models
        collectedTools += tools

        return deferred + rpcDeferred + definitionDeferred + toolDeferred + routerDeferred + middlewareDeferred + implementationDeferred
    }

    // -------------------------------------------------------------------------
    // Model building
    // -------------------------------------------------------------------------

    private fun buildAgentModel(classDecl: KSClassDeclaration): AgentModel {
        val agent = classDecl.annotation("Agent")
            ?: error("Class ${classDecl.simpleName.asString()} is missing @Agent")
        val mount = agent.arg("mount") as? String ?: ""
        // Empty = use the class's simple name, matching Scala's `agentDefinitionTypeName(sym).getOrElse(sym.name)`.
        val declaredTypeName = (agent.arg("typeName") as? String)?.takeIf { it.isNotEmpty() }
        // @Agent(description=...) is the primary source; a class-level @Description(text=...) wins if present.
        val agentDesc = agent.arg("description") as? String ?: ""
        // @Description, then @Agent(description = ...), then the class's KDoc -- Scala's precedence.
        val classDesc = classDecl.annotationByFqn("cloud.golem.annotations.Description")?.arg("text") as? String
            ?: agentDesc.takeIf { it.isNotEmpty() }
            ?: kdocDescription(classDecl)
            ?: ""
        val mountAuth = agent.arg("auth") as? Boolean ?: false

        @Suppress("UNCHECKED_CAST")
        val mountCors = (agent.arg("cors") as? List<String>) ?: emptyList()
        val mode = agent.arg("mode") as? String ?: "durable"
        val snapshotting = agent.arg("snapshotting") as? String ?: "disabled"

        // A `Config<T>` constructor parameter is the agent's config tree, not a constructor argument.
        val allCtorParams = classDecl.primaryConstructor?.parameters ?: emptyList()
        val configParamIndex = allCtorParams.indexOfFirst { isConfigParam(it) }
        val config = allCtorParams.getOrNull(configParamIndex)?.let { buildConfigModel(it, classDecl) }
        val constructorParams = allCtorParams.filterNot { isConfigParam(it) }.map { buildParamModel(it) }

        // Only DECLARED members (no inherited ones), filtered to KSFunctionDeclaration --
        // avoids inherited/overridden @Endpoint methods appearing twice via getAllFunctions().
        // Dedupe by name keeping first occurrence as an extra guard against duplicate declarations.
        classDecl.declarations.filterIsInstance<KSFunctionDeclaration>().filter { fn ->
            fn.inheritedAnnotation("cloud.golem.annotations.Endpoint") == null &&
                (fn.inheritedAnnotation("cloud.golem.annotations.DurableStreamSlot") != null || fn.inheritedAnnotation("cloud.golem.annotations.DurableStreamOptions") != null)
        }.forEach { fn ->
            logger.error("@DurableStreamSlot / @DurableStreamOptions on method '${fn.simpleName.asString()}': selector is ambiguous or missing (the method needs exactly one HTTP endpoint)", fn)
        }
        val methods = classDecl.declarations
            .filterIsInstance<KSFunctionDeclaration>()
            .filter { fn -> fn.inheritedAnnotation("cloud.golem.annotations.Endpoint") != null }
            .map { fn -> buildMethodModel(fn) }
            .toList()
            .distinctBy { it.name }

        // Detect `cloud.golem.Snapshotted<S>` in the supertypes and resolve `S` to a TypeDesc.
        // A non-WIT-mappable `S` is a compile error (logger.error), never a silent skip.
        val snapshotStateType: TypeDesc? = classDecl.superTypes
            .map { it.resolve() }
            .firstOrNull { it.declaration.qualifiedName?.asString() == "cloud.golem.Snapshotted" }
            ?.arguments?.firstOrNull()?.type?.resolve()
            ?.let { s ->
                try {
                    TypeMapper.resolve(s)
                } catch (e: UnsupportedTypeException) {
                    logger.error(
                        "@Agent ${classDecl.simpleName.asString()} implements Snapshotted<S> " +
                            "but S is not a WIT-mappable type: ${e.message}",
                        classDecl,
                    )
                    null
                }
            }

        val phantom = agent.arg("phantom") as? Boolean ?: false
        val phantomPath = agent.arg("phantomIdPath") as? String ?: ""
        val phantomQuery = agent.arg("phantomIdQuery") as? String ?: ""
        val phantomOptional = agent.arg("phantomIdOptional") as? Boolean ?: false
        if (phantomPath.isNotEmpty() && phantomQuery.isNotEmpty()) {
            logger.error("@Agent ${classDecl.simpleName.asString()}: phantomIdPath and phantomIdQuery are mutually exclusive", classDecl)
        }
        val phantomBinding = when {
            phantomPath.isNotEmpty() -> Triple("path", phantomPath, phantomOptional)
            phantomQuery.isNotEmpty() -> Triple("query", phantomQuery, phantomOptional)
            else -> null
        }
        val webhookSuffix = agent.arg("webhookSuffix") as? String ?: ""
        if (webhookSuffix.isNotEmpty()) {
            if (mount.isEmpty()) logger.error("@Agent ${classDecl.simpleName.asString()}: webhookSuffix requires a mount", classDecl)
            if (webhookSuffix.contains('{') || webhookSuffix.contains('}')) {
                logger.error("@Agent ${classDecl.simpleName.asString()}: webhookSuffix cannot contain path variables", classDecl)
            }
        }
        val fileResponseHeaders = headerPairs(agent.arg("fileResponseHeaders"))
        val exposeFiles = mappingPairs(agent.arg("exposeFiles"))
        val compiledFiles = compileMappings(exposeFiles, "exposeFiles", classDecl)

        val superFqns = classDecl.superTypes.map { it.resolve().declaration.qualifiedName?.asString() }.toList()
        val hooks = listOfNotNull(
            "bytes".takeIf { "cloud.golem.CustomSnapshot" in superFqns },
            "multipart".takeIf { "cloud.golem.MultipartSnapshotted" in superFqns },
        )
        val modes = hooks + listOfNotNull("typed".takeIf { snapshotStateType != null })
        // Snapshotted<S> composes with MultipartSnapshotted: the derived state codec rides in the state slot
        // of the multipart frame, beside the application's own parts. CustomSnapshot owns the whole payload,
        // so it composes with neither.
        val composes = modes.size == 2 && "typed" in modes && "multipart" in modes
        if (modes.size > 1 && !composes) {
            logger.error(
                "@Agent ${classDecl.simpleName.asString()} mixes several snapshot modes (${modes.joinToString()}): " +
                    "choose Snapshotted<S>, CustomSnapshot, MultipartSnapshotted, or Snapshotted<S> together with MultipartSnapshotted.",
                classDecl,
            )
        }

        return AgentModel(
            className = classDecl.simpleName.asString(),
            typeName = declaredTypeName ?: classDecl.simpleName.asString(),
            snapshotHooks = hooks.firstOrNull(),
            phantom = phantom || phantomBinding != null,
            phantomBinding = phantomBinding,
            webhookSuffix = webhookSuffix,
            filesystemBindingDecls = exposeFiles,
            filesystemBindings = compiledFiles,
            fileResponseHeaders = fileResponseHeaders,
            qualifiedName = classDecl.qualifiedName?.asString()
                ?: error("Anonymous class cannot be @Agent"),
            packageName = classDecl.packageName.asString(),
            mountPath = mount,
            classDescription = classDesc,
            mountAuth = mountAuth,
            mountCors = mountCors,
            mode = mode,
            snapshotting = snapshotting,
            constructorParams = constructorParams,
            methods = methods,
            snapshotStateType = snapshotStateType,
            config = config,
            configParamIndex = configParamIndex,
        )
    }

    // Unannotated classes whose superclass may be a generated middleware base, which exists only from the round after its definition is processed.
    private val undecidedMiddlewareCandidates = HashSet<String>()

    private fun reportUnannotatedMiddlewares(resolver: Resolver) {
        fun collect(decl: KSClassDeclaration) {
            val name = decl.qualifiedName?.asString()
            if (name != null && Modifier.ABSTRACT !in decl.modifiers && decl.annotationByFqn(middlewareFqn) == null) undecidedMiddlewareCandidates += name
            decl.declarations.filterIsInstance<KSClassDeclaration>().forEach(::collect)
        }
        resolver.getNewFiles().flatMap { it.declarations }.filterIsInstance<KSClassDeclaration>().forEach(::collect)
        undecidedMiddlewareCandidates.removeAll { name ->
            val decl = resolver.getClassDeclarationByName(resolver.getKSNameFromString(name)) ?: return@removeAll true
            val superTypes = decl.superTypes.map { it.resolve() }.toList()
            if (TypedMiddlewareResolver.isTyped(decl)) {
                logger.error("GolemKSP: typed middleware '${decl.simpleName.asString()}' is not annotated @ToolMiddleware", decl)
            }
            superTypes.none { it.isError }
        }
    }

    private fun buildProviderModel(decl: KSClassDeclaration): ToolProviderModel? {
        val interfaces = decl.superTypes.map { it.resolve().declaration }.filterIsInstance<KSClassDeclaration>().filter { it.isToolDefinition() }.toList()
        val problem = providerProblem(decl, interfaces.size)
        val definition = if (problem == null) definitionModel(interfaces.single()) else null
        val streaming = definition?.flatCommands()?.firstOrNull { it.stdin || it.hasOutputs }
        val unsupported = streaming?.let { "cannot serve command '${it.path.joinToString(" ")}': a provider does not support stdin, stdout or stderr" }
        (problem ?: unsupported)?.let { logger.error("GolemKSP: @ToolImplementation ${decl.simpleName.asString()} $it", decl) }
        return if (definition == null || unsupported != null) {
            null
        } else {
            ToolProviderModel(decl.packageName.asString(), decl.qualifiedName?.asString().orEmpty(), decl.simpleName.asString(), definition)
                .also { ToolProviderEmitter(codeGenerator).emit(it) }
        }
    }

    /** Why [decl] cannot serve a tool, or `null` when it can; [definitionCount] is how many `@ToolDefinition` interfaces it implements. */
    private fun providerProblem(decl: KSClassDeclaration, definitionCount: Int): String? = when {
        decl.classKind != ClassKind.CLASS || Modifier.ABSTRACT in decl.modifiers -> "must be a concrete class"
        decl.qualifiedName == null || decl.packageName.asString().isEmpty() -> "must be declared in a package"
        decl.getConstructors().none { c -> c.parameters.all { it.hasDefault } } -> "must have a constructor that takes no arguments"
        definitionCount != 1 -> "must implement exactly one @ToolDefinition interface, found $definitionCount"
        else -> null
    }

    private fun buildMiddlewareModel(decl: KSClassDeclaration, resolver: Resolver): ToolMiddlewareModel? {
        val annotation = decl.annotationByFqn(middlewareFqn) ?: error("missing @ToolMiddleware")

        @Suppress("UNCHECKED_CAST")
        val aliases = (annotation.arg("aliases") as? List<String>) ?: emptyList()
        val base = ToolMiddlewareModel(
            name = annotation.arg("name") as? String ?: "",
            version = annotation.arg("version") as? String ?: "0.1.0",
            aliases = aliases,
            description = annotation.arg("description") as? String ?: "",
            packageName = decl.packageName.asString(),
            qualifiedName = decl.qualifiedName?.asString() ?: error("Anonymous class cannot be a @ToolMiddleware"),
            parameterDesc = (annotation.arg("parameters") as? KSType)
                ?.takeIf { it.declaration.qualifiedName?.asString() != "kotlin.Unit" }
                ?.let { TypeMapper.resolve(it) },
        )
        val binding = TypedMiddlewareResolver(resolver, logger) { definitionModel(it) }.resolve(decl) ?: return if (TypedMiddlewareResolver.isTyped(decl)) null else base
        val model = base.copy(typed = binding)
        TypedMiddlewareEmitter(codeGenerator).emit(model, binding)
        return model
    }

    private fun buildToolModel(fn: KSFunctionDeclaration): ToolModel? {
        val name = fn.simpleName.asString()
        if (fn.parentDeclaration != null) {
            logger.error("@Tool function $name must be a top-level function (tools are stateless)", fn)
            return null
        }
        val tool = fn.annotationByFqn(toolFqn) ?: return null
        val toolName = tool.arg("name") as? String ?: ""
        if (toolName.isEmpty()) {
            logger.error("@Tool on $name is missing a name", fn)
            return null
        }
        val annotated = fn.parameters.firstOrNull { it.annotationByFqn("cloud.golem.annotations.Arg") != null }
        if (annotated != null) {
            logger.error("@Arg is not supported on @Tool function parameters ($name: ${annotated.name?.asString()}); declare options, flags and bounds on a @ToolDefinition interface", annotated)
            return null
        }
        val returnType = fn.returnType?.resolve()
            ?: error("Tool function $name has no return type")
        return ToolModel(
            name = toolName,
            description = tool.arg("description") as? String ?: "",
            packageName = fn.packageName.asString(),
            qualifiedName = fn.qualifiedName?.asString() ?: name,
            // A `ToolStreams` parameter is the body's stream context, not a positional.
            params = fn.parameters.filterNot { isToolStreams(it) }.map { buildParamModel(it) },
            outputTypeDesc = TypeMapper.resolve(returnType),
            stdin = tool.arg("stdin") as? Boolean ?: false,
            stdout = tool.arg("stdout") as? Boolean ?: false,
            stderr = tool.arg("stderr") as? Boolean ?: false,
            streamsParamIndex = fn.parameters.indexOfFirst { isToolStreams(it) },
            errors = (tool.arg("errors") as? List<*>).orEmpty().filterIsInstance<KSAnnotation>().mapNotNull { buildToolError(it, name, fn) },
        )
    }

    private val toolErrorName = Regex("^[a-z][a-z0-9]*(-[a-z0-9]+)*$")

    private fun buildToolError(case: KSAnnotation, toolFn: String, at: KSFunctionDeclaration): ToolErrorModel? {
        val name = case.arg("name") as? String ?: ""
        if (!toolErrorName.matches(name)) {
            logger.error("@Tool $toolFn: error case name '$name' must be lowercase kebab-case (e.g. 'not-found')", at)
            return null
        }
        val exitCode = (case.arg("exitCode") as? Int) ?: 1
        if (exitCode !in 0..255) {
            logger.error("@Tool $toolFn: error case '$name' exitCode $exitCode is outside 0..255", at)
            return null
        }
        val payload = (case.arg("payload") as? KSType)
            ?.takeIf { it.declaration.qualifiedName?.asString() != "kotlin.Unit" }
            ?.let { TypeMapper.resolve(it) }
        return ToolErrorModel(
            name = name,
            description = case.arg("description") as? String ?: "",
            payloadWitType = payload?.toWit(),
            usage = case.arg("usage") as? Boolean ?: false,
            exitCode = exitCode,
        )
    }

    /** `(source, target)` pairs of a `Array<FileMapping>` annotation argument. */
    private fun mappingPairs(arg: Any?): List<Pair<String, String>> = (arg as? List<*>).orEmpty().filterIsInstance<KSAnnotation>().map {
        (it.arg("source") as String) to (it.arg("target") as String)
    }

    private fun headerPairs(arg: Any?): List<Pair<String, String>> = (arg as? List<*>).orEmpty().filterIsInstance<KSAnnotation>().map {
        (it.arg("name") as String) to (it.arg("value") as String)
    }

    private fun compileMappings(declared: List<Pair<String, String>>, what: String, at: KSClassDeclaration): List<FileMapping> = when (val compiled = FileMappings.compile(declared)) {
        is Compiled.Ok -> compiled.value
        is Compiled.Error -> {
            logger.error("${at.simpleName.asString()}: invalid $what mapping (${compiled.code}): ${declared.joinToString { "${it.first} -> ${it.second}" }}", at)
            emptyList()
        }
    }

    /** A model for an `@HttpRouter` class: ephemeral, no identity, literal mount, optional static files and OpenAPI provider. */
    private fun buildRouterModel(classDecl: KSClassDeclaration): AgentModel {
        val router = classDecl.annotationByFqn(routerFqn) ?: error("missing @HttpRouter")
        val typeName = router.arg("typeName") as? String ?: ""
        val allCtorParams = classDecl.primaryConstructor?.parameters ?: emptyList()
        val configParamIndex = allCtorParams.indexOfFirst { isConfigParam(it) }
        val config = allCtorParams.getOrNull(configParamIndex)?.let { buildConfigModel(it, classDecl) }
        val constructorParams = allCtorParams.filterNot { isConfigParam(it) }.map { buildParamModel(it) }

        val functions = classDecl.declarations.filterIsInstance<KSFunctionDeclaration>().toList()
        val handlers = functions.filter { it.annotationByFqn("cloud.golem.annotations.HttpHandler") != null }
        val providers = functions.filter { it.annotationByFqn("cloud.golem.annotations.OpenApiProvider") != null }
        val handlerMethods = handlers.map { fn ->
            MethodModel(
                name = fn.simpleName.asString(),
                description = "",
                promptHint = "",
                inputParams = fn.parameters.map { buildParamModel(it) },
                outputTypeDesc = TypeMapper.resolve(fn.returnType?.resolve() ?: error("handler ${fn.simpleName.asString()} has no return type")),
                // The router catch-all binding (`http-method.any`, empty path suffix).
                httpEndpoints = listOf(HttpEndpointModel("ANY", "/")),
            )
        }
        val providerMethods = providers.map { fn ->
            MethodModel(
                name = fn.simpleName.asString(),
                description = "",
                promptHint = "",
                inputParams = fn.parameters.map { buildParamModel(it) },
                outputTypeDesc = TypeMapper.resolve(fn.returnType?.resolve() ?: error("provider ${fn.simpleName.asString()} has no return type")),
                httpEndpoints = emptyList(),
            )
        }
        val methods = handlerMethods + providerMethods
        val staticDecls = mappingPairs(router.arg("staticBindings"))

        @Suppress("UNCHECKED_CAST")
        val cors = (router.arg("cors") as? List<String>) ?: emptyList()
        return AgentModel(
            className = classDecl.simpleName.asString(),
            qualifiedName = classDecl.qualifiedName?.asString() ?: error("Anonymous class cannot be @HttpRouter"),
            packageName = classDecl.packageName.asString(),
            mountPath = router.arg("mount") as? String ?: "",
            classDescription = "",
            mountAuth = router.arg("auth") as? Boolean ?: false,
            mountCors = cors,
            mode = "ephemeral",
            snapshotting = "disabled",
            constructorParams = constructorParams,
            methods = methods,
            config = config,
            configParamIndex = configParamIndex,
            kind = "http-router",
            typeName = typeName,
            staticBindingDecls = staticDecls,
            staticBindings = compileMappings(staticDecls, "staticBindings", classDecl),
            fileResponseHeaders = headerPairs(router.arg("fileResponseHeaders")),
            openApiProvider = providers.firstOrNull()?.simpleName?.asString(),
        )
    }

    /** Applies the router / live-file rules (the shared HTTP corpus' metadata rules) to [model]. */
    private fun validateHttpAgent(model: AgentModel, at: KSClassDeclaration) {
        val segments = model.mountPath.split("/").filter { it.isNotEmpty() }.map {
            if (it.startsWith("{") && it.endsWith("}")) MountSegment.Variable(it.substring(1, it.length - 1)) else MountSegment.Literal(it)
        }
        val spec = AgentSpec(
            kind = model.kind,
            mode = model.mode,
            snapshotEnabled = model.snapshotting != "disabled",
            constructor = model.constructorParams.map { it.name to it.witType },
            mount = segments,
            staticBindings = model.staticBindingDecls,
            filesystemBindings = model.filesystemBindingDecls,
            methods = model.methods.map { m ->
                // The canonical HTTP envelopes are checked by name (the corpus's `HttpRequest` / `HttpResponse`).
                fun symbolic(td: TypeDesc) = when {
                    td is TypeDesc.HttpT -> if (td.response) "HttpResponse" else "HttpRequest"
                    else -> td.toWit()
                }
                MethodSpec(
                    m.name,
                    m.inputParams.map { it.name to symbolic(it.typeDesc) },
                    symbolic(m.outputTypeDesc),
                    m.httpEndpoints.map { (if (it.verb == "ANY") "Any" else it.verb) to it.path },
                    pathVars = m.httpEndpoints.flatMap { e -> e.path.split("/").filter { it.startsWith("{") && it.endsWith("}") }.map { it.trim('{', '}', '+') } }.toSet(),
                    queryParams = m.httpEndpoints.flatMap { e -> e.queryVars.map { it.first } }.toSet(),
                )
            },
            provider = model.openApiProvider,
            phantom = model.phantom,
            phantomBinding = model.phantomBinding?.let { it.first to it.second },
        )
        val code = RouterValidation.validate(spec) ?: return
        logger.error("${model.className}: ${RouterValidation.describe(code)} ($code)", at)
    }

    private fun isConfigParam(param: KSValueParameter): Boolean = param.type.resolve().declaration.qualifiedName?.asString() == "cloud.golem.config.Config"

    /** Derives the config declarations and the loader expression from `Config<T>`'s type argument. */
    private fun buildConfigModel(param: KSValueParameter, owner: KSClassDeclaration): ConfigModel? {
        val rootType = param.type.resolve().arguments.firstOrNull()?.type?.resolve()
        if (rootType == null) {
            logger.error("@Agent ${owner.simpleName.asString()}: Config<T> needs a type argument", param)
            return null
        }
        val declarations = mutableListOf<ConfigDeclModel>()
        val expression = try {
            ConfigWalker.walk(rootType, emptyList(), declarations)
        } catch (e: UnsupportedTypeException) {
            logger.error("@Agent ${owner.simpleName.asString()}: unsupported config type: ${e.message}", param)
            return null
        }
        return ConfigModel(declarations, expression)
    }

    private fun isToolStreams(param: KSValueParameter): Boolean = param.type.resolve().declaration.qualifiedName?.asString() == "cloud.golem.runtime.ToolStreams"

    private fun buildParamModel(param: KSValueParameter): ParamModel = ParamModel(
        name = param.name?.asString() ?: error("Unnamed constructor param"),
        typeDesc = TypeMapper.resolve(param.type.resolve()),
    )

    private fun buildMethodModel(fn: KSFunctionDeclaration): MethodModel {
        val endpoint = checkNotNull(fn.inheritedAnnotation("cloud.golem.annotations.Endpoint")) {
            "only a function annotated with @Endpoint reaches buildMethodModel"
        }
        val promptHint = fn.inheritedAnnotation("cloud.golem.annotations.Prompt")?.arg("hint") as? String ?: ""
        // The annotation wins; otherwise the KDoc of the method or of the contract method it overrides.
        val methodDesc = fn.inheritedAnnotation("cloud.golem.annotations.Description")?.arg("text") as? String
            ?: fn.overrideChain().firstNotNullOfOrNull { kdocDescription(it) }
            ?: ""

        val endpointAuth = endpoint.explicitBool("auth")

        @Suppress("UNCHECKED_CAST")
        val endpointCors = (endpoint.arg("cors") as? List<String>) ?: emptyList()

        // `@Header("X-Name")` parameters bind request headers on every endpoint of the method.
        val headerVars = fn.parameters.withIndex().mapNotNull { (index, p) ->
            val header = fn.inheritedParamAnnotation(index, "cloud.golem.annotations.Header") ?: return@mapNotNull null
            (header.arg("name") as String) to (p.name?.asString() ?: error("unnamed @Header parameter"))
        }
        val httpEndpoints = buildList {
            for ((verb, argName) in listOf("GET" to "get", "POST" to "post", "PUT" to "put", "DELETE" to "delete", "PATCH" to "patch")) {
                val raw = endpoint.arg(argName) as? String ?: ""
                if (raw.isEmpty()) continue
                // "/items/{id}?format={fmt}": the part after '?' binds query parameters to method parameters.
                val path = raw.substringBefore('?')
                val queryVars = if ('?' in raw) {
                    try {
                        parseQueryBindings(raw.substringAfter('?'))
                    } catch (e: IllegalArgumentException) {
                        logger.error("Method ${fn.simpleName.asString()}: ${e.message} in '$raw'", fn)
                        emptyList()
                    }
                } else {
                    emptyList()
                }
                add(HttpEndpointModel(verb, path, endpointAuth, endpointCors, headerVars, queryVars))
            }
        }

        val httpEndpointsWithStreams = withDurableStreams(fn, httpEndpoints)

        val inputParams = fn.parameters.map { buildParamModel(it) }
        val returnType = fn.returnType?.resolve()
            ?: error("Method ${fn.simpleName.asString()} has no return type")

        // @ReadOnly(cache=...) -> the read-only-config's cache policy; absent => not read-only.
        val readOnly = fn.inheritedAnnotation("cloud.golem.annotations.ReadOnly")
        val readOnlyCache = if (readOnly != null) (readOnly.arg("cache") as? String ?: "until-write") else null

        return MethodModel(
            name = fn.simpleName.asString(),
            description = methodDesc,
            promptHint = promptHint,
            inputParams = inputParams,
            outputTypeDesc = TypeMapper.resolve(returnType),
            httpEndpoints = httpEndpointsWithStreams,
            readOnlyCache = readOnlyCache,
        )
    }

    /**
     * Attaches `@DurableStreamSlot` / `@DurableStreamOptions` to the endpoints of a method, with the same
     * rules as the Scala SDK. Each annotation may name its endpoint with `endpointMethod` and / or
     * `endpointPath`, as Scala's `durableStreamSlot` and `durableStreams` do; a method that declares one
     * endpoint needs no selector. Slots that select different endpoints of the same method are allowed, so
     * one method can expose a stream on its POST and another on its GET.
     */
    private fun withDurableStreams(fn: KSFunctionDeclaration, endpoints: List<HttpEndpointModel>): List<HttpEndpointModel> {
        val slotFqn = "cloud.golem.annotations.DurableStreamSlot"
        val optionsFqn = "cloud.golem.annotations.DurableStreamOptions"
        val slotAnnotations = fn.inheritedAnnotations(slotFqn)
        val options = fn.inheritedAnnotation(optionsFqn)
        if (slotAnnotations.isEmpty() && options == null) return endpoints
        val name = fn.simpleName.asString()

        // Resolve one annotation's target endpoint, or null after reporting why it cannot be resolved.
        fun target(annotation: KSAnnotation, what: String): HttpEndpointModel? {
            val verb = (annotation.arg("endpointMethod") as? String)?.takeIf { it.isNotEmpty() }
            val path = (annotation.arg("endpointPath") as? String)?.takeIf { it.isNotEmpty() }
            val matching = endpoints.filter { ep ->
                (verb == null || ep.verb.equals(verb, ignoreCase = true)) && (path == null || ep.path == path)
            }
            return when {
                matching.size == 1 -> matching.single()
                verb == null && path == null -> {
                    logger.error("$what on method '$name': selector is ambiguous or missing (the method needs exactly one HTTP endpoint)", fn)
                    null
                }
                matching.isEmpty() -> {
                    logger.error("$what on method '$name': no endpoint matches endpointMethod='${verb ?: ""}' endpointPath='${path ?: ""}'", fn)
                    null
                }
                else -> {
                    logger.error("$what on method '$name': endpointMethod='${verb ?: ""}' endpointPath='${path ?: ""}' matches ${matching.size} endpoints", fn)
                    null
                }
            }
        }

        val slotsByEndpoint = LinkedHashMap<HttpEndpointModel, MutableList<DurableSlotModel>>()
        for (annotation in slotAnnotations) {
            val source = annotation.arg("source") as? String ?: ""
            val slot = annotation.arg("slot") as? String ?: ""
            if (source != "input" && source != "output") logger.error("@DurableStreamSlot source must be 'input' or 'output', found '$source'", fn)
            if (slot.isEmpty()) logger.error("@DurableStreamSlot on method '$name' must specify slot", fn)
            val endpoint = target(annotation, "@DurableStreamSlot") ?: continue
            val model = DurableSlotModel(source, slot, (annotation.arg("name") as? String)?.ifEmpty { null }, (annotation.arg("contentType") as? String)?.ifEmpty { null })
            val existing = slotsByEndpoint.getOrPut(endpoint) { mutableListOf() }
            if (existing.any { it.source == model.source && it.slot == model.slot }) {
                logger.error("duplicate @DurableStreamSlot for ${model.source} slot '${model.slot}' on method '$name'", fn)
            }
            existing += model
        }

        val optionsEndpoint = options?.let { target(it, "@DurableStreamOptions") }
        if (options != null && optionsEndpoint == null) return endpoints
        if (optionsEndpoint != null) slotsByEndpoint.getOrPut(optionsEndpoint) { mutableListOf() }
        if (slotsByEndpoint.isEmpty()) return endpoints

        return endpoints.map { ep ->
            val slots = slotsByEndpoint[ep] ?: return@map ep
            var model = DurableStreamsModel(slots)
            if (options != null && ep == optionsEndpoint) {
                val readers = (options.arg("maxConcurrentReadersPerStream") as? Int ?: 0).takeIf { it != 0 }
                val appends = (options.arg("maxAppendRequestsPerSecondPerStream") as? Int ?: 0).takeIf { it != 0 }
                // Only a flag the source actually writes is sent. The three are `option<bool>` on the wire, and
                // the host rejects `allow-external-writes` -- set to any value -- on a route with no writable
                // input slot, so defaulting them made @DurableStreamOptions unusable on an output-only route.
                val writes = options.explicitBool("allowExternalWrites")
                if (readers != null && readers !in 1..16) logger.error("@DurableStreamOptions reader limit must be between 1 and 16", fn)
                if (appends != null && appends <= 0) logger.error("@DurableStreamOptions append limit must be positive", fn)
                if (appends != null && writes == false) logger.error("@DurableStreamOptions append limit cannot be set when external writes are disabled", fn)
                model = model.copy(
                    allowExternalWrites = writes,
                    allowStreamDelete = options.explicitBool("allowStreamDelete"),
                    allowInvocationDelete = options.explicitBool("allowInvocationDelete"),
                    readers = readers,
                    appends = appends,
                )
            }
            ep.copy(durableStreams = model)
        }
    }

    // -------------------------------------------------------------------------
    // Annotation helpers
    // -------------------------------------------------------------------------

    private fun KSAnnotated.annotationByFqn(fqn: String): KSAnnotation? = annotations.firstOrNull {
        it.annotationType.resolve().declaration.qualifiedName?.asString() == fqn
    }

    /**
     * A class method and the supertype declarations it overrides, nearest first. A contract interface
     * can carry `@Endpoint`, `@Description`, `@Prompt` and `@ReadOnly` for its implementors, the way a
     * Scala agent trait does. Kotlin does not inherit a member annotation onto an override, so every
     * lookup walks this chain instead of reading the override alone.
     */
    private fun KSFunctionDeclaration.overrideChain(): Sequence<KSFunctionDeclaration> = sequence {
        var current: KSFunctionDeclaration? = this@overrideChain
        while (current != null) {
            yield(current)
            current = current.findOverridee() as? KSFunctionDeclaration
        }
    }

    /** The nearest declaration in [overrideChain] that carries [fqn]. */
    private fun KSFunctionDeclaration.inheritedAnnotation(fqn: String): KSAnnotation? = overrideChain().firstNotNullOfOrNull { it.annotationByFqn(fqn) }

    /**
     * Every [fqn] annotation of the nearest declaration in [overrideChain] that carries any. A repeatable
     * annotation is taken from one declaration only, so an override's slots replace the interface's
     * rather than merging with them.
     */
    private fun KSFunctionDeclaration.inheritedAnnotations(fqn: String): List<KSAnnotation> = overrideChain()
        .map { fn -> fn.annotations.filter { it.annotationType.resolve().declaration.qualifiedName?.asString() == fqn }.toList() }
        .firstOrNull { it.isNotEmpty() }
        ?: emptyList()

    /** [fqn] on the parameter at [index], taken from the nearest declaration in [overrideChain] that has it. */
    private fun KSFunctionDeclaration.inheritedParamAnnotation(index: Int, fqn: String): KSAnnotation? = overrideChain().firstNotNullOfOrNull { it.parameters.getOrNull(index)?.annotationByFqn(fqn) }

    private fun KSAnnotated.annotation(shortName: String): KSAnnotation? = annotations.firstOrNull { it.shortName.asString() == shortName }

    private fun KSAnnotation.arg(name: String): Any? = arguments.firstOrNull { it.name?.asString() == name }?.value

    /**
     * The value of a `Boolean` annotation argument the source names, or null when it leaves the argument
     * to its default. KSP fills a defaulted argument in [KSAnnotation.arguments] and marks it
     * [Origin.SYNTHETIC], so the two cases are only distinguishable by origin -- and they have to be, because
     * an endpoint's `auth-details` is an `option`: unset inherits the mount, while an explicit value overrides
     * it either way.
     */
    private fun KSAnnotation.explicitBool(name: String): Boolean? = arguments.firstOrNull { it.name?.asString() == name && it.origin != Origin.SYNTHETIC }?.value as? Boolean
}
