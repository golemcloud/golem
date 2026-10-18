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
  def sendTemplate(target: String): Future[List[String]]
}

/** Observes from the callee side; its agent type has no initial card permissions. */
@agentDefinition()
trait ScalaPermissionCardTarget extends BaseAgent {
  class Id(val name: String)
  def selfAgentId(): Future[String]
  def inspectAllowed(): Future[Boolean]
  def expiringCardCount(): Future[Int]
  def deriveFromWalletError(): Future[String]
  def walletKinds(): Future[String]
  def lentScopeExpiry(): Future[Option[Long]]
  // Generated RPC clients copy signature types without the imports of this
  // file, so card types are fully qualified here.
  def receiveTemplate(card: golem.host.PermissionsApi.PolymorphicPermissionCard): Future[List[String]]
  def receiveConcrete(card: golem.host.PermissionsApi.PermissionCard): Future[Boolean]
}

object PermissionCardKinds {
  def kind(entry: WalletCard): String =
    entry match {
      case WalletCard.Concrete(_) => "concrete"
      case WalletCard.Template(_) => "template"
      case WalletCard.Scope(_)    => "scope"
    }
}

@agentImplementation()
final class ScalaPermissionCardTargetImpl(name: String) extends ScalaPermissionCardTarget {

  private def withWallet[A](f: List[WalletCard] => A): A = {
    val cards = PermissionsApi.wallet()
    try f(cards)
    finally cards.foreach(_.card.release())
  }

  override def selfAgentId(): Future[String] =
    Future.successful(HostApi.getSelfMetadata().agentId.agentId)

  override def inspectAllowed(): Future[Boolean] =
    Future.successful(withWallet(_.exists(_.card.inspect().isRight)))

  override def expiringCardCount(): Future[Int] =
    Future.successful(withWallet(_.count(_.card.expiresAt.nonEmpty)))

  override def walletKinds(): Future[String] =
    Future.successful(withWallet(_.map(PermissionCardKinds.kind).sorted.mkString(",")))

  override def lentScopeExpiry(): Future[Option[Long]] =
    Future.successful(withWallet(_.collectFirst { case WalletCard.Scope(scope) => scope.expiresAt.map(_.seconds) }.flatten))

  override def receiveTemplate(card: PolymorphicPermissionCard): Future[List[String]] = {
    val observed = List(s"received-parents=${card.parents.size}", s"received-inspect=${card.inspect().isRight}")
    card.release()
    Future.successful(observed)
  }

  override def receiveConcrete(card: PermissionCard): Future[Boolean] = {
    card.release()
    Future.successful(true)
  }

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
    try cards.exists(_.card.id == id)
    finally cards.foreach(_.card.release())
  }

  /** The wallet entry that grants `card:inspect`; every other entry is released. */
  private def rootEntry(): WalletCard = {
    val wallet          = PermissionsApi.wallet()
    val (roots, others) =
      wallet.partition(entry => entry.card.inspect().exists(_.lowerPositive.exists(isInspectGrant)))
    others.foreach(_.card.release())
    roots.drop(1).foreach(_.card.release())
    roots.headOption.getOrElse(throw new IllegalStateException("no card:inspect card in the wallet"))
  }

  override def sendTemplate(target: String): Future[List[String]] =
    rootEntry() match {
      case WalletCard.Template(template) =>
        val remote = ScalaPermissionCardTargetClient.get(target)
        for {
          received <- remote.receiveTemplate(template)
          kinds    <- remote.walletKinds()
        } yield received ++ List(s"sent-moved=${!template.isPresent}", s"receiver-kinds=$kinds")
      case other =>
        other.card.release()
        Future.successful(List(s"root-kind=${PermissionCardKinds.kind(other)}"))
    }

  override def scenario(target: String): Future[List[String]] = {
    val entry        = rootEntry()
    val rootKind     = PermissionCardKinds.kind(entry)
    val root         = entry.card
    val inspectGrant = unwrap("inspect root", root.inspect()).lowerPositive
      .find(isInspectGrant)
      .getOrElse(throw new IllegalStateException("the root card does not grant card:inspect"))
    val inspectOnly  = Bounds(lowerPositive = List(inspectGrant))

    // 1. Derive a card from the wallet. 2. Inspect it.
    val derived: PersistentCard = unwrap("deriveFromWallet", PermissionsApi.deriveFromWallet(inspectOnly))
    val derivedView = unwrap("inspect derived", derived.inspect())

    // 3. Derive a child with a lifespan.
    val child          = unwrap("derive child", derived.derive(inspectOnly, Some(Lifespan(1.hour))))
    val childParent    = child match {
      case PersistentCard.Concrete(card) => card.parents == List(derived.id)
      case PersistentCard.Template(_)   => false
    }
    val childExpiry    = child.expiresAt.nonEmpty && derived.expiresAt.isEmpty

    // These concrete bounds produce concrete children. The host labels the
    // result; a template parent alone does not determine the child's kind.
    val template = entry match {
      case WalletCard.Template(card) => card
      case other                     => throw new IllegalStateException(s"the root card is ${PermissionCardKinds.kind(other)}")
    }
    val self            = HostApi.getSelfMetadata()
    val selfHolder      = Holder.agent(self.componentId, self.agentId.agentId)
    val lifespans       = List(2.hours, 1.hour, 3.hours)
    val timed           = lifespans.map(l => unwrap(s"derive $l child", template.derive(inspectOnly, Some(Lifespan(l)))))
    val templateDerived = timed.forall(_.parents == List(template.id))
    val timedIds        = timed.map(_.id)
    val shortExpiry     = timed(1).expiresAt
    timed.foreach(card => unwrap("install timed child", card.install(selfHolder)))
    val (children, rest) = PermissionsApi.wallet().partition(e => timedIds.contains(e.card.id))
    rest.foreach(_.card.release())
    val childKinds = children.map(PermissionCardKinds.kind).sorted.mkString(",")
    val timedRoots = children.sortBy(e => timedIds.indexOf(e.card.id)).map(_.card)

    // 4. Derive a scope card and send it with awaited RPC calls.
    val scope  = unwrap("deriveScope", PermissionsApi.deriveScope(List(root), inspectOnly))
    val remote = ScalaPermissionCardTargetClient.get(target)

    // A scope card expires with its earliest root card, also for the callee:
    // the roots are a card without expiry and the cards with 2h, 1h and 3h
    // lifespans, in this order, so neither the first nor the last expiring
    // root is the earliest.
    val expiringScope =
      unwrap("deriveScope expiring", PermissionsApi.deriveScope(root :: timedRoots, inspectOnly))
    val scopeExpiry = shortExpiry.nonEmpty && expiringScope.expiresAt == shortExpiry

    for {
      targetAgentId <- remote.selfAgentId()
      before        <- remote.inspectAllowed()
      perCall       <- remote.inspectAllowed.withScopeCard(scope)()
      byDefault     <- remote.withDefaultScopeCard(scope).inspectAllowed()
      cancelable    <- remote.inspectAllowed.withScopeCard(scope).cancelable()._1
      after         <- remote.inspectAllowed()
      scopeUsable    = scope.isPresent && scope.inspect().isRight
      deniedDerive  <- remote.deriveFromWalletError()
      kindsPlain    <- remote.walletKinds()
      kindsLent     <- remote.walletKinds.withScopeCard(scope)()
      lentExpiry    <- remote.lentScopeExpiry.withScopeCard(expiringScope)()
      lentNoExpiry  <- remote.lentScopeExpiry.withScopeCard(scope)()

      // 5. Install a card on another agent.
      holder          = Holder.agent(HostApi.getSelfMetadata().componentId, targetAgentId)
      installed       = child.install(holder)
      targetExpiring <- remote.expiringCardCount()
      targetInspect  <- remote.inspectAllowed()

      // 6. Revoke a card and check the returned count and the lost authority.
      selfHeld        = unwrap("derive self-held", derived.derive(inspectOnly))
      grandchild      = unwrap("derive grandchild", selfHeld.derive(inspectOnly))
      selfHeldId      = selfHeld.id
      selfInstalled   = selfHeld.install(selfHolder)
      heldBefore      = walletHas(selfHeldId)
      (selfKind, revoked) = {
                              val (mine, rest) = PermissionsApi.wallet().partition(_.card.id == selfHeldId)
                              rest.foreach(_.card.release())
                              mine.drop(1).foreach(_.card.release())
                              val kinds  = mine.headOption.map(PermissionCardKinds.kind)
                              val result = mine.headOption.map {
                                case WalletCard.Concrete(card) => card.revoke()
                                case other                     =>
                                  other.card.release()
                                  Left(PermissionError.NotPermitted("not a concrete card"))
                              }
                              (kinds, result)
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
      val childrenRevoked = children.map {
        case WalletCard.Concrete(card) => card.revoke()
        case other                     =>
          other.card.release()
          Left(PermissionError.NotPermitted("not a concrete card"))
      }
      derived.release()
      root.release()
      expiringScope.release()
      keptScope = Some(scope)
      grandchild.release()
      List(
        s"root-kind=$rootKind",
        s"derived-inspect=${derivedView.lowerPositive == List(inspectGrant)}",
        s"child-parent=$childParent",
        s"child-expiry=$childExpiry",
        s"template-derive=$templateDerived",
        s"template-children=$childKinds",
        s"scope-expiry=$scopeExpiry",
        s"root-scope-expiry=${scope.expiresAt}",
        s"before=$before",
        s"per-call=$perCall",
        s"default=$byDefault",
        s"cancelable=$cancelable",
        s"after=$after",
        s"scope-usable=$scopeUsable",
        s"denied-derive=$deniedDerive",
        s"kinds-plain=$kindsPlain",
        s"kinds-lent=$kindsLent",
        s"lent-expiry=${lentExpiry.nonEmpty && lentExpiry == shortExpiry.map(_.seconds)}",
        s"lent-no-expiry=$lentNoExpiry",
        s"installed=${installed.isRight}",
        s"target-expiring=$targetExpiring",
        s"target-inspect=$targetInspect",
        s"self-installed=${selfInstalled.isRight}",
        s"held-before=$heldBefore",
        s"self-held-kind=$selfKind",
        s"revoked=$revoked",
        s"held-after=$heldAfter",
        s"parent-revoked=$parentRevoked",
        s"target-inspect-after-revoke=$targetInspectAfter",
        s"target-expiring-after-revoke=$targetExpiringAfter",
        s"moved-child=$movedChild",
        s"mismatch=${mismatch.left.map(_.getClass.getSimpleName)}",
        s"children-revoked=$childrenRevoked"
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
