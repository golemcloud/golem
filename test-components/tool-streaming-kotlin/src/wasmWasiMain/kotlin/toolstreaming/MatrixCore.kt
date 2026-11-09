package toolstreaming

import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.ToolDefinition
import cloud.golem.annotations.ToolError
import cloud.golem.annotations.ToolErrorKind
import cloud.golem.BaseAgent
import cloud.golem.Principal
import cloud.golem.runtime.Either
import cloud.golem.runtime.ToolInvokeError
import cloud.golem.runtime.ToolRpc

data class MatrixDimensions(val width: UInt, val height: UInt)

data class MatrixRequest(val source: String, val dimensions: MatrixDimensions, val labels: List<String>)

data class MatrixResult(
    val provider: String,
    val command: String,
    val normalizedSource: String,
    val weightedSize: Long,
    val labelSummary: String,
    val principal: String,
    val ownerAgentId: String,
)

data class MatrixRejectedPayload(val field: String, val reason: String, val retryable: Boolean)

sealed class MatrixError {
    @ToolError(kind = ToolErrorKind.USAGE, exitCode = 2)
    data class Rejected(val payload: MatrixRejectedPayload) : MatrixError()
}

@ToolDefinition(name = "matrix-core", version = "1.0.0")
interface MatrixCoreTool {
    fun artifact(): MatrixArtifactTool
}

@ToolDefinition(name = "artifact", version = "1.0.0")
interface MatrixArtifactTool {
    fun inspect(request: MatrixRequest, multiplier: Long, principal: Principal): Either<MatrixError, MatrixResult>
}

data class MatrixCoreObservation(
    val provider: String,
    val command: String,
    val normalizedSource: String,
    val weightedSize: Long,
    val labelSummary: String,
    val principal: String,
    val ownerAgentId: String,
    val errorField: String,
    val errorReason: String,
    val errorRetryable: Boolean,
)

@Agent(description = "Calls matrix-core through the generated typed client")
class KotlinToolStreamingCaller(val name: String) : BaseAgent() {
    @Endpoint
    fun matrix_core_observation(): MatrixCoreObservation {
        val success = MatrixRequest(
            source = "matrix.sample",
            dimensions = MatrixDimensions(width = 3u, height = 5u),
            labels = listOf("north", "east", "south"),
        )
        val rejected = success.copy(source = "reject.me")
        val artifact = MatrixCoreToolClient(ToolRpc("matrix-core")).artifact()
        val ok = artifact.inspect(success, 7L)
        val failed = artifact.inspect(rejected, 7L)
        if (ok !is Either.Right || failed !is Either.Left) error("unexpected matrix-core results: $ok / $failed")
        val declared = failed.value as? ToolInvokeError.Tool ?: error("unexpected matrix-core results: $ok / $failed")
        val rejection = declared.error as? MatrixError.Rejected ?: error("unexpected matrix-core results: $ok / $failed")
        val result = ok.value
        return MatrixCoreObservation(
            provider = result.provider,
            command = result.command,
            normalizedSource = result.normalizedSource,
            weightedSize = result.weightedSize,
            labelSummary = result.labelSummary,
            principal = result.principal,
            ownerAgentId = result.ownerAgentId,
            errorField = rejection.payload.field,
            errorReason = rejection.payload.reason,
            errorRetryable = rejection.payload.retryable,
        )
    }
}
