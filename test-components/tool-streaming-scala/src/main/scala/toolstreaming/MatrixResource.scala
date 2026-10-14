package toolstreaming

import golem.config.{AgentConfig, Config, Secret}
import golem.host.QuotaApi.{NamedQuotaToken, QuotaToken}
import golem.host.SecretApi
import golem.runtime.annotations.*
import golem.schema.{AgentStream, GuestPermissionCardHandle, GuestSecretHandle}
import golem.{BaseAgent, HostApi, Principal, UInt}
import zio.blocks.schema.Schema

import scala.concurrent.ExecutionContext
import scala.concurrent.Future

final case class SecretExchange(
  provider: String,
  principal: String,
  ownerAgentId: String,
  revealed: Boolean,
  secret: GuestSecretHandle
)

final case class QuotaExchange(
  provider: String,
  principal: String,
  ownerAgentId: String,
  reserved: Boolean,
  token: NamedQuotaToken["matrix-capacity"]
)

final case class PermissionExchange(
  provider: String,
  principal: String,
  ownerAgentId: String,
  card: GuestPermissionCardHandle
)

final case class PermissionIssue(
  card: GuestPermissionCardHandle,
  issuer: String,
  principal: String,
  ownerAgentId: String
)

@toolDefinition(name = "matrix-resource", version = "1.0.0")
trait MatrixResourceTool {
  def secret(): MatrixSecretTool
  def quota(): MatrixQuotaTool
  def permissions(): MatrixPermissionsTool
  def typed(): MatrixTypedTool
}

@toolDefinition(name = "secret", version = "1.0.0")
trait MatrixSecretTool {
  @command(name = "exchange")
  def secretExchange(secret: GuestSecretHandle, principal: Principal): SecretExchange
}

@toolDefinition(name = "quota", version = "1.0.0")
trait MatrixQuotaTool {
  @command(name = "exchange")
  def quotaExchange(token: NamedQuotaToken["matrix-capacity"], principal: Principal): QuotaExchange
}

@toolDefinition(name = "permissions", version = "1.0.0")
trait MatrixPermissionsTool {
  @command(name = "exchange")
  def permissionExchange(card: GuestPermissionCardHandle, principal: Principal): PermissionExchange
}

@toolDefinition(name = "typed", version = "1.0.0")
trait MatrixTypedTool {
  def transform(input: AgentStream[UInt]): AgentStream[UInt]
}

@toolDefinition(name = "matrix-permission-issuer", version = "1.0.0")
trait MatrixPermissionIssuer {
  def issue(principal: Principal): PermissionIssue
}

@toolImplementation()
final class MatrixResourceToolImpl extends MatrixResourceTool {
  override def secret(): MatrixSecretTool           = new MatrixSecretToolImpl
  override def quota(): MatrixQuotaTool             = new MatrixQuotaToolImpl
  override def permissions(): MatrixPermissionsTool = new MatrixPermissionsToolImpl
  override def typed(): MatrixTypedTool             = new MatrixTypedToolImpl
}

final class MatrixSecretToolImpl extends MatrixSecretTool {
  override def secretExchange(secret: GuestSecretHandle, principal: Principal): SecretExchange = {
    val (provider, principalEvidence, ownerAgentId) = matrixResourceEvidence(principal)
    SecretExchange(
      provider = provider,
      principal = principalEvidence,
      ownerAgentId = ownerAgentId,
      revealed = SecretApi.reveal[String](secret) == "matrix-secret-value",
      secret = secret
    )
  }
}

final class MatrixQuotaToolImpl extends MatrixQuotaTool {
  override def quotaExchange(token: NamedQuotaToken["matrix-capacity"], principal: Principal): QuotaExchange = {
    val (provider, principalEvidence, ownerAgentId) = matrixResourceEvidence(principal)
    val reserved = token.reserve(BigInt(1)) match {
      case Right(reservation) => reservation.commit(BigInt(1)); true
      case Left(_)            => false
    }
    QuotaExchange(provider, principalEvidence, ownerAgentId, reserved, token)
  }
}

final class MatrixPermissionsToolImpl extends MatrixPermissionsTool {
  override def permissionExchange(card: GuestPermissionCardHandle, principal: Principal): PermissionExchange = {
    val (provider, principalEvidence, ownerAgentId) = matrixResourceEvidence(principal)
    PermissionExchange(provider, principalEvidence, ownerAgentId, card)
  }
}

final class MatrixTypedToolImpl extends MatrixTypedTool {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic

  override def transform(input: AgentStream[UInt]): AgentStream[UInt] =
    input.map(value => UInt(value.value * 3L + 1L))
}

private def matrixResourceEvidence(principal: Principal): (String, String, String) =
  (
    "scala",
    principal match {
      case Principal.Anonymous                             => "anonymous"
      case Principal.Oidc(sub, _, _, _, _, _, _, _, _, _) => s"oidc:$sub"
      case Principal.Agent(_, _)                           => "agent"
      case Principal.GolemUser(_)                          => "golem-user"
    },
    HostApi.getSelfMetadata().agentId.agentId
  )

final case class MatrixResourceConfig(secret: Secret[String])
object MatrixResourceConfig {
  implicit val schema: Schema[MatrixResourceConfig] = Schema.derived
}

final case class MatrixResourceObservation(
  secretFirstProvider: String,
  secretSecondProvider: String,
  secretFirstRevealed: Boolean,
  secretSecondRevealed: Boolean,
  secretPrincipal: String,
  secretOwnerAgentId: String,
  quotaProvider: String,
  quotaReserved: Boolean,
  quotaReturnedUsable: Boolean,
  quotaOriginalConsumed: Boolean,
  quotaPrincipal: String,
  quotaOwnerAgentId: String,
  permissionSupported: Boolean,
  permissionProvider: String,
  permissionSameIdentity: Boolean,
  permissionOriginalConsumed: Boolean,
  permissionPrincipal: String,
  permissionOwnerAgentId: String,
  typedValues: List[UInt]
)
object MatrixResourceObservation {
  implicit val schema: Schema[MatrixResourceObservation] = Schema.derived
}

@agentDefinition()
trait ScalaResourceToolStreamingCaller extends BaseAgent with AgentConfig[MatrixResourceConfig] {
  class Id(val name: String)
  def matrix_secret_observation(): Future[MatrixResourceObservation]
  def matrix_quota_observation(): Future[MatrixResourceObservation]
  def matrix_permission_observation(): Future[MatrixResourceObservation]
  def matrix_typed_stream_observation(): Future[MatrixResourceObservation]
  def matrix_resource_observation(): Future[MatrixResourceObservation]
}

@agentImplementation()
final class ScalaResourceToolStreamingCallerImpl(name: String, config: Config[MatrixResourceConfig])
    extends ScalaResourceToolStreamingCaller {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  private def emptyObservation: MatrixResourceObservation =
    MatrixResourceObservation(
      secretFirstProvider = "",
      secretSecondProvider = "",
      secretFirstRevealed = false,
      secretSecondRevealed = false,
      secretPrincipal = "",
      secretOwnerAgentId = "",
      quotaProvider = "",
      quotaReserved = false,
      quotaReturnedUsable = false,
      quotaOriginalConsumed = false,
      quotaPrincipal = "",
      quotaOwnerAgentId = "",
      permissionSupported = false,
      permissionProvider = "",
      permissionSameIdentity = false,
      permissionOriginalConsumed = false,
      permissionPrincipal = "",
      permissionOwnerAgentId = "",
      typedValues = Nil
    )

  override def matrix_secret_observation(): Future[MatrixResourceObservation] = {
    val configuredSecret = config.value.secret.handle()
    MatrixResourceToolClient().secret().secretExchange(configuredSecret).flatMap {
      case Left(error) => Future.failed(new IllegalStateException(s"first matrix secret exchange failed: $error"))
      case Right(first) =>
        MatrixResourceToolClient().secret().secretExchange(first.secret).map {
          case Left(error) => throw new IllegalStateException(s"second matrix secret exchange failed: $error")
          case Right(second) =>
            emptyObservation.copy(
              secretFirstProvider = first.provider,
              secretSecondProvider = second.provider,
              secretFirstRevealed = first.revealed,
              secretSecondRevealed = second.revealed,
              secretPrincipal = second.principal,
              secretOwnerAgentId = second.ownerAgentId
            )
        }
    }
  }

  override def matrix_quota_observation(): Future[MatrixResourceObservation] = {
    val original = QuotaToken.named["matrix-capacity"](BigInt(2))
    MatrixResourceToolClient().quota().quotaExchange(original).map {
      case Left(error) => throw new IllegalStateException(s"matrix quota exchange failed: $error")
      case Right(exchange) =>
        val originalConsumed = !original.isPresent
        val returnedUsable = exchange.token.reserve(BigInt(0)) match {
          case Right(reservation) => reservation.commit(BigInt(0)); true
          case Left(_)            => false
        }
        emptyObservation.copy(
          quotaProvider = exchange.provider,
          quotaReserved = exchange.reserved,
          quotaReturnedUsable = returnedUsable,
          quotaOriginalConsumed = originalConsumed,
          quotaPrincipal = exchange.principal,
          quotaOwnerAgentId = exchange.ownerAgentId
        )
    }
  }

  override def matrix_permission_observation(): Future[MatrixResourceObservation] =
    MatrixPermissionIssuerClient().issue().flatMap {
      case Left(error) => Future.failed(new IllegalStateException(s"matrix permission issue failed: $error"))
      case Right(issue) if issue.issuer != "rust" =>
        Future.failed(new IllegalStateException(s"matrix permission issuer was ${issue.issuer}, expected rust"))
      case Right(issue) if issue.principal != "oidc:matrix-resource-acceptance" =>
        Future.failed(
          new IllegalStateException(
            s"matrix permission issuer principal was ${issue.principal}, expected oidc:matrix-resource-acceptance"
          )
        )
      case Right(issue) if issue.ownerAgentId != HostApi.getSelfMetadata().agentId.agentId =>
        Future.failed(
          new IllegalStateException(
            s"matrix permission issuer owner was ${issue.ownerAgentId}, expected the calling agent"
          )
        )
      case Right(issue) =>
        val original = issue.card
        MatrixResourceToolClient().permissions().permissionExchange(original).map {
          case Left(error) => throw new IllegalStateException(s"matrix permission exchange failed: $error")
          case Right(exchange) =>
            val originalConsumed = !original.isPresent
            emptyObservation.copy(
              permissionSupported = true,
              permissionProvider = exchange.provider,
              permissionSameIdentity = originalConsumed && exchange.card.isPresent,
              permissionOriginalConsumed = originalConsumed,
              permissionPrincipal = exchange.principal,
              permissionOwnerAgentId = exchange.ownerAgentId
            )
        }
    }

  override def matrix_typed_stream_observation(): Future[MatrixResourceObservation] =
    typedValues().map(values => emptyObservation.copy(typedValues = values))

  override def matrix_resource_observation(): Future[MatrixResourceObservation] = {
    val configuredSecret = config.value.secret.handle()

    MatrixResourceToolClient().secret().secretExchange(configuredSecret).flatMap {
      case Left(error) => Future.failed(new IllegalStateException(s"first matrix secret exchange failed: $error"))
      case Right(firstSecret) =>
        MatrixResourceToolClient().secret().secretExchange(firstSecret.secret).flatMap {
          case Left(error) => Future.failed(new IllegalStateException(s"second matrix secret exchange failed: $error"))
          case Right(secondSecret) =>
            val quota = QuotaToken.named["matrix-capacity"](BigInt(2))
            MatrixResourceToolClient().quota().quotaExchange(quota).flatMap {
              case Left(error) => Future.failed(new IllegalStateException(s"matrix quota exchange failed: $error"))
              case Right(quotaExchange) =>
                val quotaOriginalConsumed = !quota.isPresent
                val quotaReturnedUsable = quotaExchange.token.reserve(BigInt(0)) match {
                  case Right(reservation) => reservation.commit(BigInt(0)); true
                  case Left(_)            => false
                }
                MatrixPermissionIssuerClient().issue().flatMap {
                  case Left(error) => Future.failed(new IllegalStateException(s"matrix permission issue failed: $error"))
                  case Right(issue) if issue.issuer != "rust" =>
                    Future.failed(new IllegalStateException(s"matrix permission issuer was ${issue.issuer}, expected rust"))
                  case Right(issue) if issue.principal != "oidc:matrix-resource-acceptance" =>
                    Future.failed(
                      new IllegalStateException(
                        s"matrix permission issuer principal was ${issue.principal}, expected oidc:matrix-resource-acceptance"
                      )
                    )
                  case Right(issue) if issue.ownerAgentId != HostApi.getSelfMetadata().agentId.agentId =>
                    Future.failed(
                      new IllegalStateException(
                        s"matrix permission issuer owner was ${issue.ownerAgentId}, expected the calling agent"
                      )
                    )
                  case Right(issue) =>
                    val originalCard = issue.card
                    MatrixResourceToolClient().permissions().permissionExchange(originalCard).flatMap {
                      case Left(error) =>
                        Future.failed(new IllegalStateException(s"matrix permission exchange failed: $error"))
                      case Right(permissionExchange) =>
                        val permissionOriginalConsumed = !originalCard.isPresent
                        val permissionSameIdentity =
                          permissionOriginalConsumed && permissionExchange.card.isPresent
                        typedValues().map { values =>
                          MatrixResourceObservation(
                            secretFirstProvider = firstSecret.provider,
                            secretSecondProvider = secondSecret.provider,
                            secretFirstRevealed = firstSecret.revealed,
                            secretSecondRevealed = secondSecret.revealed,
                            secretPrincipal = secondSecret.principal,
                            secretOwnerAgentId = secondSecret.ownerAgentId,
                            quotaProvider = quotaExchange.provider,
                            quotaReserved = quotaExchange.reserved,
                            quotaReturnedUsable = quotaReturnedUsable,
                            quotaOriginalConsumed = quotaOriginalConsumed,
                            quotaPrincipal = quotaExchange.principal,
                            quotaOwnerAgentId = quotaExchange.ownerAgentId,
                            permissionSupported = true,
                            permissionProvider = permissionExchange.provider,
                            permissionSameIdentity = permissionSameIdentity,
                            permissionOriginalConsumed = permissionOriginalConsumed,
                            permissionPrincipal = permissionExchange.principal,
                            permissionOwnerAgentId = permissionExchange.ownerAgentId,
                            typedValues = values
                          )
                        }
                    }
                }
            }
        }
    }
  }

  private def typedValues(): Future[List[UInt]] = {
    val values = Iterator(UInt(2), UInt(5), UInt(9))
    val input = AgentStream.fromPull(() => Future.successful(if (values.hasNext) Some(values.next()) else None))
    MatrixResourceToolClient().typed().transform(input).flatMap {
      case Left(error)  => Future.failed(new IllegalStateException(s"matrix-resource typed transform failed: $error"))
      case Right(output) => drainAgentStream(output, Nil)
    }
  }

  private def drainAgentStream(stream: AgentStream[UInt], values: List[UInt]): Future[List[UInt]] =
    stream.pull().flatMap {
      case Some(value) => drainAgentStream(stream, values :+ value)
      case None        => Future.successful(values)
    }
}
