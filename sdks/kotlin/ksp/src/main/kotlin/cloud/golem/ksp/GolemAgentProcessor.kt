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
    private val logger: KSPLogger
) : SymbolProcessor {

    private val agentFqn = "cloud.golem.annotations.Agent"

    override fun process(resolver: Resolver): List<KSAnnotated> {
        val (valid, deferred) = resolver
            .getSymbolsWithAnnotation(agentFqn)
            .partition { it.validate() }

        val models = valid
            .filterIsInstance<KSClassDeclaration>()
            .map { classDecl ->
                val model = buildAgentModel(classDecl)
                logger.info("GolemKSP: processing ${model.qualifiedName}")
                RegistrationEmitter(codeGenerator).emit(model)
                WitEmitter(codeGenerator).emit(model)
                model
            }
            .toList()

        if (models.isNotEmpty()) {
            // I1/I2: GolemKotlinSdk is a @JsModule external declared in one package;
            // it cannot live in two packages simultaneously. Until the Phase D shared
            // artifact moves GolemKotlinSdk to a stable importable location, all @Agent
            // classes must share one package.
            val distinctPackages = models.map { it.packageName }.distinct()
            if (distinctPackages.size > 1) {
                logger.error(
                    "GolemKSP: All @Agent classes must currently share one package so the " +
                        "generated registration can see the GolemKotlinSdk external; " +
                        "multi-package support arrives with the Phase D shared artifact. " +
                        "Found packages: ${distinctPackages.joinToString()}"
                )
            } else {
                // A single entry point that registers every @Agent in this module.
                RegistrationEmitter(codeGenerator).emitEntryPoint(models)
            }
        }

        return deferred
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
        // M1: use FQN for sub-annotation lookup.
        val classDesc = classDecl.annotationByFqn("cloud.golem.annotations.Description")?.arg("text") as? String
            ?: agentDesc

        val constructorParams = classDecl.primaryConstructor
            ?.parameters
            ?.map { buildParamModel(it) }
            ?: emptyList()

        // I3: use classDecl.declarations to get only DECLARED members (no inherited ones),
        // filtering down to KSFunctionDeclaration — avoids inherited/overridden @Endpoint
        // methods appearing twice via getAllFunctions(). Dedupe by name keeping first
        // occurrence as an extra guard against duplicate declarations.
        val methods = classDecl.declarations
            .filterIsInstance<KSFunctionDeclaration>()
            .filter { fn -> fn.annotationByFqn("cloud.golem.annotations.Endpoint") != null }
            .map { fn -> buildMethodModel(fn) }
            .toList()
            .distinctBy { it.name }

        return AgentModel(
            className = classDecl.simpleName.asString(),
            qualifiedName = classDecl.qualifiedName?.asString()
                ?: error("Anonymous class cannot be @Agent"),
            packageName = classDecl.packageName.asString(),
            mountPath = mount,
            classDescription = classDesc,
            constructorParams = constructorParams,
            methods = methods
        )
    }

    private fun buildParamModel(param: KSValueParameter): ParamModel {
        val type = param.type.resolve()
        return ParamModel(
            name = param.name?.asString() ?: error("Unnamed constructor param"),
            witType = TypeMapper.toWit(type),
            kotlinQualifiedType = type.declaration.qualifiedName?.asString()
                ?: error("Unresolved constructor param type")
        )
    }

    private fun buildMethodModel(fn: KSFunctionDeclaration): MethodModel {
        // M1: sub-annotations matched by FQN.
        val endpoint = fn.annotationByFqn("cloud.golem.annotations.Endpoint")!!
        val promptHint = fn.annotationByFqn("cloud.golem.annotations.Prompt")?.arg("hint") as? String ?: ""
        val methodDesc = fn.annotationByFqn("cloud.golem.annotations.Description")?.arg("text") as? String ?: ""

        val httpEndpoints = buildList {
            val get = endpoint.arg("get") as? String ?: ""
            val post = endpoint.arg("post") as? String ?: ""
            val put = endpoint.arg("put") as? String ?: ""
            val delete = endpoint.arg("delete") as? String ?: ""
            if (get.isNotEmpty()) add(HttpEndpointModel("GET", get))
            if (post.isNotEmpty()) add(HttpEndpointModel("POST", post))
            if (put.isNotEmpty()) add(HttpEndpointModel("PUT", put))
            if (delete.isNotEmpty()) add(HttpEndpointModel("DELETE", delete))
        }

        val inputParams = fn.parameters.map { buildParamModel(it) }
        val returnType = fn.returnType?.resolve()
            ?: error("Method ${fn.simpleName.asString()} has no return type")

        return MethodModel(
            name = fn.simpleName.asString(),
            description = methodDesc,
            promptHint = promptHint,
            inputParams = inputParams,
            outputWitType = TypeMapper.toWit(returnType),
            outputKotlinType = returnType.declaration.qualifiedName?.asString()
                ?: error("Unresolved return type on ${fn.simpleName.asString()}"),
            httpEndpoints = httpEndpoints
        )
    }

    // -------------------------------------------------------------------------
    // Annotation helpers
    // -------------------------------------------------------------------------

    // M1: match by fully-qualified name for precision. Short-name matching would
    // collide if user code declares its own @Endpoint / @Prompt / @Description.
    private fun KSAnnotated.annotationByFqn(fqn: String): KSAnnotation? =
        annotations.firstOrNull {
            it.annotationType.resolve().declaration.qualifiedName?.asString() == fqn
        }

    // Keep short-name helper for the top-level @Agent scan (processor already uses
    // agentFqn for the resolver query; short-name is fine on the already-filtered set).
    private fun KSAnnotated.annotation(shortName: String): KSAnnotation? =
        annotations.firstOrNull { it.shortName.asString() == shortName }

    private fun KSAnnotation.arg(name: String): Any? =
        arguments.firstOrNull { it.name?.asString() == name }?.value
}
