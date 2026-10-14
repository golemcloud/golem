package toolstreaming

import golem.{BaseAgent, HostApi}
import golem.host.PermissionsApi
import golem.host.PermissionsApi.*
import golem.runtime.annotations.*

import scala.concurrent.duration.*
import scala.concurrent.{ExecutionContext, Future}
import scala.util.Try

/** Runs the shared permission-card scenario with the typed Scala API. */
@agentDefinition()
trait ScalaPermissionCardAgent extends BaseAgent {
  class Id(val name: String)
  def scenario(target: String): Future[List[String]]
  def staleScope(target: String): Future[List[String]]
}

/** Observes from the callee side; its agent type has no initial card permissions. */
@agentDefinition()
trait ScalaPermissionCardTarget extends BaseAgent {
  class Id(val name: String)
  def selfAgentId(): Future[String]
  def inspectAllowed(): Future[Boolean]
  def expiringCardCount(): Future[Int]
  def deriveFromWalletError(): Future[String]
}

@agentImplementation()
final class ScalaPermissionCardTargetImpl(name: String) extends ScalaPermissionCardTarget {

  private def withWallet[A](f: List[PermissionCard] => A): A = {
    val cards = PermissionsApi.wallet()
    try f(cards)
    finally cards.foreach(_.release())
  }

  override def selfAgentId(): Future[String] =
    Future.successful(HostApi.getSelfMetadata().agentId.agentId)

  override def inspectAllowed(): Future[Boolean] =
    Future.successful(withWallet(_.exists(_.inspect().isRight)))

  override def expiringCardCount(): Future[Int] =
    Future.successful(withWallet(_.count(_.expiresAt.nonEmpty)))

  override def deriveFromWalletError(): Future[String] = {
    val grant = PatternGrant("card", "*", "*", "inspect", "*")
    Future.successful(PermissionsApi.deriveFromWallet(Bounds(lowerPositive = List(grant))) match {
      case Left(error) => error.getClass.getSimpleName
      case Right(card) =>
        card.release()
        "derived"
    })
  }
}

@agentImplementation()
final class ScalaPermissionCardAgentImpl(name: String) extends ScalaPermissionCardAgent {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  private def isInspectGrant(grant: PatternGrant): Boolean =
    grant.className == "card" && grant.verb == "inspect" && grant.resourceId == "*"

  private def unwrap[A](step: String, result: Either[PermissionError, A]): A =
    result.fold(error => throw new IllegalStateException(s"$step failed: $error"), identity)

  /** The scope card of the last scenario, kept to show that it goes stale. */
  private var keptScope: Option[ScopeCard] = None

  override def staleScope(target: String): Future[List[String]] = {
    val scope  = keptScope.getOrElse(throw new IllegalStateException("run the scenario first"))
    val parent = PermissionsApi.deriveScope(List(scope), Bounds.empty)
    ScalaPermissionCardTargetClient
      .get(target)
      .inspectAllowed
      .withScopeCard(scope)()
      .map(allowed => s"attach=$allowed")
      .recover { case error => s"attach-failed=${String.valueOf(error.getMessage).contains("denied")}" }
      .map { attach =>
        scope.release()
        keptScope = None
        List(
          attach,
          s"stale-parent=${parent.left.map(_.getClass.getSimpleName)}",
          s"released=${!scope.isPresent}"
        )
      }
  }

  private def walletHas(id: CardId): Boolean = {
    val cards = PermissionsApi.wallet()
    try cards.exists(_.id == id)
    finally cards.foreach(_.release())
  }

  override def scenario(target: String): Future[List[String]] = {
    val wallet          = PermissionsApi.wallet()
    val (roots, others) = wallet.partition(card => card.inspect().exists(_.lowerPositive.exists(isInspectGrant)))
    others.foreach(_.release())
    val root         = roots.headOption.getOrElse(throw new IllegalStateException("no card:inspect card in the wallet"))
    roots.drop(1).foreach(_.release())
    val inspectGrant = unwrap("inspect root", root.inspect()).lowerPositive
      .find(isInspectGrant)
      .getOrElse(throw new IllegalStateException("the root card does not grant card:inspect"))
    val inspectOnly  = Bounds(lowerPositive = List(inspectGrant))

    // 1. Derive a card from the wallet. 2. Inspect it.
    val derived     = unwrap("deriveFromWallet", PermissionsApi.deriveFromWallet(inspectOnly))
    val derivedView = unwrap("inspect derived", derived.inspect())

    // 3. Derive a child with a lifespan.
    val child       = unwrap("derive child", derived.derive(inspectOnly, Some(Lifespan(1.hour))))
    val childParent = child.parents == List(derived.id)
    val childExpiry = child.expiresAt.nonEmpty && derived.expiresAt.isEmpty

    // 4. Derive a scope card and send it with awaited RPC calls.
    val scope  = unwrap("deriveScope", PermissionsApi.deriveScope(List(root), inspectOnly))
    val remote = ScalaPermissionCardTargetClient.get(target)

    for {
      targetAgentId <- remote.selfAgentId()
      before        <- remote.inspectAllowed()
      perCall       <- remote.inspectAllowed.withScopeCard(scope)()
      byDefault     <- remote.withDefaultScopeCard(scope).inspectAllowed()
      cancelable    <- remote.inspectAllowed.withScopeCard(scope).cancelable()._1
      after         <- remote.inspectAllowed()
      scopeUsable    = scope.isPresent && scope.inspect().isRight
      deniedDerive  <- remote.deriveFromWalletError()

      // 5. Install a card on another agent.
      holder          = Holder.agent(HostApi.getSelfMetadata().componentId, targetAgentId)
      installed       = child.install(holder)
      targetExpiring <- remote.expiringCardCount()
      targetInspect  <- remote.inspectAllowed()

      // 6. Revoke a card and check the returned count and the lost authority.
      selfHeld        = unwrap("derive self-held", derived.derive(inspectOnly))
      grandchild      = unwrap("derive grandchild", selfHeld.derive(inspectOnly))
      selfHeldId      = selfHeld.id
      selfHolder      = Holder.agent(HostApi.getSelfMetadata().componentId, HostApi.getSelfMetadata().agentId.agentId)
      selfInstalled   = selfHeld.install(selfHolder)
      heldBefore      = walletHas(selfHeldId)
      revoked         = {
                          val (mine, rest) = PermissionsApi.wallet().partition(_.id == selfHeldId)
                          rest.foreach(_.release())
                          mine.drop(1).foreach(_.release())
                          mine.headOption.map(_.revoke())
                        }
      heldAfter       = walletHas(selfHeldId)

      // Revoking the parent of the card installed on the target removes the
      // target's authority.
      parentRevoked        = derived.revoke()
      targetInspectAfter  <- remote.inspectAllowed()
      targetExpiringAfter <- remote.expiringCardCount()

      // 7. Negative cases.
      movedChild      = Try(child.inspect()).failed.toOption.exists(_.isInstanceOf[IllegalStateException])
      mismatchCard    = unwrap(
                          "derive mismatch card",
                          PermissionsApi.deriveFromWallet(
                            Bounds(lowerPositive = List(inspectGrant.copy(recipient = "nobody@invalid.example")))
                          )
                        )
      mismatch        = mismatchCard.install(holder)
    } yield {
      derived.release()
      root.release()
      keptScope = Some(scope)
      grandchild.release()
      List(
        s"derived-inspect=${derivedView.lowerPositive == List(inspectGrant)}",
        s"child-parent=$childParent",
        s"child-expiry=$childExpiry",
        s"before=$before",
        s"per-call=$perCall",
        s"default=$byDefault",
        s"cancelable=$cancelable",
        s"after=$after",
        s"scope-usable=$scopeUsable",
        s"denied-derive=$deniedDerive",
        s"installed=${installed.isRight}",
        s"target-expiring=$targetExpiring",
        s"target-inspect=$targetInspect",
        s"self-installed=${selfInstalled.isRight}",
        s"held-before=$heldBefore",
        s"revoked=$revoked",
        s"held-after=$heldAfter",
        s"parent-revoked=$parentRevoked",
        s"target-inspect-after-revoke=$targetInspectAfter",
        s"target-expiring-after-revoke=$targetExpiringAfter",
        s"moved-child=$movedChild",
        s"mismatch=${mismatch.left.map(_.getClass.getSimpleName)}"
      )
    }
  }
}

/** A user type named like a scoped call; the generated client must keep its meaning. */
final case class Scoped(value: String)
object Scoped {
  implicit val schema: zio.blocks.schema.Schema[Scoped] = zio.blocks.schema.Schema.derived
}

@agentDefinition()
trait ScalaScopedNameAgent extends BaseAgent {
  class Id(val name: String)
  def run(scoped: Scoped): Future[Scoped]
}

@agentImplementation()
final class ScalaScopedNameAgentImpl(name: String) extends ScalaScopedNameAgent {
  override def run(scoped: Scoped): Future[Scoped] = Future.successful(Scoped(s"$name:${scoped.value}"))
}
