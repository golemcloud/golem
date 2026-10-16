/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package example.integrationtests

import golem.{HostApi, Principal, UInt}
import golem.runtime.annotations.*
import zio.blocks.schema.Schema

final case class MatrixDimensions(width: UInt, height: UInt)
object MatrixDimensions {
  implicit val schema: Schema[MatrixDimensions] = Schema.derived
}

final case class MatrixRequest(source: String, dimensions: MatrixDimensions, labels: Seq[String])
object MatrixRequest {
  implicit val schema: Schema[MatrixRequest] = Schema.derived
}

final case class MatrixResult(
  provider: String,
  command: String,
  normalizedSource: String,
  weightedSize: Long,
  labelSummary: String,
  principal: String,
  ownerAgentId: String
)
object MatrixResult {
  implicit val schema: Schema[MatrixResult] = Schema.derived
}

final case class MatrixRejectedPayload(field: String, reason: String, retryable: Boolean)
object MatrixRejectedPayload {
  implicit val schema: Schema[MatrixRejectedPayload] = Schema.derived
}

enum MatrixError {
  @error(kind = "usage-error", exitCode = 2)
  case Rejected(payload: MatrixRejectedPayload)
}

@toolDefinition(name = "matrix-core", version = "1.0.0")
trait MatrixCoreTool {
  def artifact(): MatrixArtifactTool
}

@toolDefinition(name = "artifact", version = "1.0.0")
trait MatrixArtifactTool {
  def inspect(
    request: MatrixRequest,
    multiplier: Long,
    principal: Principal
  ): Either[MatrixError, MatrixResult]
}

@toolImplementation()
final class MatrixCoreToolImpl extends MatrixCoreTool {
  override def artifact(): MatrixArtifactTool = new MatrixArtifactToolImpl
}

final class MatrixArtifactToolImpl extends MatrixArtifactTool {
  override def inspect(
    request: MatrixRequest,
    multiplier: Long,
    principal: Principal
  ): Either[MatrixError, MatrixResult] =
    if (request.source == "reject.me")
      Left(
        MatrixError.Rejected(
          MatrixRejectedPayload(
            field = "request.source",
            reason = "unsupported source",
            retryable = false
          )
        )
      )
    else
      Right(
        MatrixResult(
          provider = "scala",
          command = "artifact/inspect",
          normalizedSource = request.source.toUpperCase,
          weightedSize =
            request.dimensions.width.value * request.dimensions.height.value * multiplier + request.labels.length,
          labelSummary = request.labels.reverse.mkString("|"),
          principal = principal match {
            case Principal.Anonymous                            => "anonymous"
            case Principal.Oidc(sub, _, _, _, _, _, _, _, _, _) => s"oidc:$sub"
            case Principal.Agent(_, _)                          => "agent"
            case Principal.GolemUser(_)                         => "golem-user"
          },
          ownerAgentId = HostApi.getSelfMetadata().agentId.agentId
        )
      )
}

final case class MatrixCoreObservation(
  provider: String,
  command: String,
  normalizedSource: String,
  weightedSize: Long,
  labelSummary: String,
  principal: String,
  ownerAgentId: String,
  errorField: String,
  errorReason: String,
  errorRetryable: Boolean
)
object MatrixCoreObservation {
  implicit val schema: Schema[MatrixCoreObservation] = Schema.derived
}
