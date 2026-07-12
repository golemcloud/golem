package cloud.golem.ksp

import com.google.devtools.ksp.processing.CodeGenerator
import com.google.devtools.ksp.processing.KSPLogger
import com.google.devtools.ksp.processing.Resolver
import com.google.devtools.ksp.processing.SymbolProcessor
import com.google.devtools.ksp.symbol.KSAnnotated
import com.google.devtools.ksp.symbol.KSAnnotation
import com.google.devtools.ksp.symbol.KSClassDeclaration
import com.google.devtools.ksp.symbol.KSFunctionDeclaration
import com.google.devtools.ksp.symbol.KSValueParameter
import com.google.devtools.ksp.validate

class GolemAgentProcessor(
    private val codeGenerator: CodeGenerator,
    private val logger: KSPLogger,
) : SymbolProcessor {

    private val agentFqn = "cloud.golem.annotations.Agent"
    private val remoteAgentFqn = "cloud.golem.annotations.RemoteAgent"

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

        val models = valid
            .filterIsInstance<KSClassDeclaration>()
            .map { classDecl ->
                val model = buildAgentModel(classDecl)
                logger.info("GolemKSP: processing ${model.qualifiedName}")
                HttpValidation.validate(model)?.let { logger.error(it, classDecl) }
                NativeRegistrationEmitter(codeGenerator).emit(model)
                WitEmitter(codeGenerator).emit(model)
                model
            }
            .toList()

        if (models.isNotEmpty()) {
            // Mirrors the JS-path processor: all @Agent classes must currently share one
            // package so the generated entry point can call every register<Class>() via a
            // single fully-qualified reference set without per-package import plumbing.
            val distinctPackages = models.map { it.packageName }.distinct()
            if (distinctPackages.size > 1) {
                logger.error(
                    "GolemKSP: All @Agent classes must currently share one package so the " +
                        "generated native registration entry point can see them; " +
                        "multi-package support is not yet available. " +
                        "Found packages: ${distinctPackages.joinToString()}",
                )
            } else {
                // A single entry point that registers every @Agent and declares the real
                // @WasmExport golem:agent/guest@2.0.0 functions (see NativeRegistrationEmitter's
                // doc comment for why registration is triggered from here, not from main()).
                NativeRegistrationEmitter(codeGenerator).emitEntryPoint(models)
            }
        }

        return deferred + rpcDeferred
    }

    // -------------------------------------------------------------------------
    // Model building
    // -------------------------------------------------------------------------

    private fun buildAgentModel(classDecl: KSClassDeclaration): AgentModel {
        val agent = classDecl.annotation("Agent")
            ?: error("Class ${classDecl.simpleName.asString()} is missing @Agent")
        val mount = agent.arg("mount") as? String ?: ""
        // @Agent(description=...) is the primary source; a class-level @Description(text=...) wins if present.
        val agentDesc = agent.arg("description") as? String ?: ""
        val classDesc = classDecl.annotationByFqn("cloud.golem.annotations.Description")?.arg("text") as? String
            ?: agentDesc
        val mountAuth = agent.arg("auth") as? Boolean ?: false

        @Suppress("UNCHECKED_CAST")
        val mountCors = (agent.arg("cors") as? List<String>) ?: emptyList()
        val mode = agent.arg("mode") as? String ?: "durable"
        val snapshotting = agent.arg("snapshotting") as? String ?: "disabled"

        val constructorParams = classDecl.primaryConstructor
            ?.parameters
            ?.map { buildParamModel(it) }
            ?: emptyList()

        // Only DECLARED members (no inherited ones), filtered to KSFunctionDeclaration --
        // avoids inherited/overridden @Endpoint methods appearing twice via getAllFunctions().
        // Dedupe by name keeping first occurrence as an extra guard against duplicate declarations.
        val methods = classDecl.declarations
            .filterIsInstance<KSFunctionDeclaration>()
            .filter { fn -> fn.annotationByFqn("cloud.golem.annotations.Endpoint") != null }
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
                } catch (e: Exception) {
                    logger.error(
                        "@Agent ${classDecl.simpleName.asString()} implements Snapshotted<S> " +
                            "but S is not a WIT-mappable type: ${e.message}",
                        classDecl,
                    )
                    null
                }
            }

        return AgentModel(
            className = classDecl.simpleName.asString(),
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
        )
    }

    private fun buildParamModel(param: KSValueParameter): ParamModel = ParamModel(
        name = param.name?.asString() ?: error("Unnamed constructor param"),
        typeDesc = TypeMapper.resolve(param.type.resolve()),
    )

    private fun buildMethodModel(fn: KSFunctionDeclaration): MethodModel {
        val endpoint = fn.annotationByFqn("cloud.golem.annotations.Endpoint")!!
        val promptHint = fn.annotationByFqn("cloud.golem.annotations.Prompt")?.arg("hint") as? String ?: ""
        val methodDesc = fn.annotationByFqn("cloud.golem.annotations.Description")?.arg("text") as? String ?: ""

        val endpointAuth = endpoint.arg("auth") as? Boolean ?: false

        @Suppress("UNCHECKED_CAST")
        val endpointCors = (endpoint.arg("cors") as? List<String>) ?: emptyList()

        val httpEndpoints = buildList {
            val get = endpoint.arg("get") as? String ?: ""
            val post = endpoint.arg("post") as? String ?: ""
            val put = endpoint.arg("put") as? String ?: ""
            val delete = endpoint.arg("delete") as? String ?: ""
            if (get.isNotEmpty()) add(HttpEndpointModel("GET", get, endpointAuth, endpointCors))
            if (post.isNotEmpty()) add(HttpEndpointModel("POST", post, endpointAuth, endpointCors))
            if (put.isNotEmpty()) add(HttpEndpointModel("PUT", put, endpointAuth, endpointCors))
            if (delete.isNotEmpty()) add(HttpEndpointModel("DELETE", delete, endpointAuth, endpointCors))
        }

        val inputParams = fn.parameters.map { buildParamModel(it) }
        val returnType = fn.returnType?.resolve()
            ?: error("Method ${fn.simpleName.asString()} has no return type")

        // @ReadOnly(cache=...) -> the read-only-config's cache policy; absent => not read-only.
        val readOnly = fn.annotationByFqn("cloud.golem.annotations.ReadOnly")
        val readOnlyCache = if (readOnly != null) (readOnly.arg("cache") as? String ?: "until-write") else null

        return MethodModel(
            name = fn.simpleName.asString(),
            description = methodDesc,
            promptHint = promptHint,
            inputParams = inputParams,
            outputTypeDesc = TypeMapper.resolve(returnType),
            httpEndpoints = httpEndpoints,
            readOnlyCache = readOnlyCache,
        )
    }

    // -------------------------------------------------------------------------
    // Annotation helpers
    // -------------------------------------------------------------------------

    private fun KSAnnotated.annotationByFqn(fqn: String): KSAnnotation? = annotations.firstOrNull {
        it.annotationType.resolve().declaration.qualifiedName?.asString() == fqn
    }

    private fun KSAnnotated.annotation(shortName: String): KSAnnotation? = annotations.firstOrNull { it.shortName.asString() == shortName }

    private fun KSAnnotation.arg(name: String): Any? = arguments.firstOrNull { it.name?.asString() == name }?.value
}
