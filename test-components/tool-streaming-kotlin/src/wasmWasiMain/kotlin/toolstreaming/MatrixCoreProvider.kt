package toolstreaming

import cloud.golem.Principal
import cloud.golem.annotations.ToolImplementation
import cloud.golem.runtime.Either
import cloud.golem.runtime.HostApi

@ToolImplementation
class MatrixCoreToolImpl : MatrixCoreTool {
    override fun artifact(): MatrixArtifactTool = MatrixArtifactToolImpl()
}

class MatrixArtifactToolImpl : MatrixArtifactTool {
    override fun inspect(request: MatrixRequest, multiplier: Long, principal: Principal): Either<MatrixError, MatrixResult> =
        if (request.source == "reject.me") {
            Either.Left(MatrixError.Rejected(MatrixRejectedPayload(field = "request.source", reason = "unsupported source", retryable = false)))
        } else {
            Either.Right(
                MatrixResult(
                    provider = "kotlin",
                    command = "artifact/inspect",
                    normalizedSource = request.source.uppercase(),
                    weightedSize = request.dimensions.width.toLong() * request.dimensions.height.toLong() * multiplier + request.labels.size,
                    labelSummary = request.labels.reversed().joinToString("|"),
                    principal = when (principal) {
                        is Principal.Anonymous -> "anonymous"
                        is Principal.Oidc -> "oidc:${principal.sub}"
                        is Principal.Agent -> "agent"
                        is Principal.GolemUser -> "golem-user"
                    },
                    ownerAgentId = HostApi.getSelfMetadata().agentId.agentId,
                ),
            )
        }
}
