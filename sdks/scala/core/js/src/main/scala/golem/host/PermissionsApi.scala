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

package golem.host

import golem.HostApi
import golem.Uuid
import golem.host.js._
import golem.schema.{
  FromSchema,
  FromSchemaError,
  GuestPermissionCardHandle,
  IntoSchema,
  PermissionCardSpec,
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaDecodeError,
  SchemaValue
}
import golem.schema.wire.{ConcreteCodec, WitSchemaTypeBody, WitSchemaValueNode}

import scala.collection.immutable.ListMap
import scala.concurrent.duration.FiniteDuration
import scala.util.control.NonFatal
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * Typed Scala.js API for `golem:permissions@0.1.0`.
 *
 * Three card types wrap the opaque, unforgeable `permission-card` resource from
 * `golem:core/types@2.0.0`: a concrete [[PermissionCard]], a template
 * [[PolymorphicPermissionCard]] and an invocation-local [[ScopeCard]]. The SDK
 * offers no conversion between them. A card that a schema codec decodes takes
 * the type that the decoder declares; the host keeps the real kind of the card,
 * so the declared type gives no authority. Guest code obtains cards only from
 * the host: from [[wallet]], from a derivation, or as a value received through
 * an agent or tool invocation. No API turns a [[CardId]], a UUID, a string or
 * other data into a card.
 *
 * Expected failures are returned as `Left(PermissionError)`. Host traps are not
 * converted: the card accessors [[Card.id]], [[Card.parents]] and
 * [[Card.expiresAt]] trap when the host does not know the handle.
 *
 * A card is affine. `revoke`, `install`, [[Card.release]] and sending the card
 * as an RPC argument or result move it; every later use throws
 * `IllegalStateException`. [[deriveScope]] returns a separate [[ScopeCard]]
 * type; only a scope card can be attached to an awaited RPC call, and attaching
 * borrows it.
 */
object PermissionsApi {

  // --- WIT: card-id ---

  /**
   * Opaque identifier of a permission card. It supports equality and hashing
   * only, so that a guest can compare [[PermissionCard.id]] with the
   * [[PermissionCard.parents]] of another card. It has no public constructor,
   * no accessor and no codec. The value is kept in a closure, so neither
   * `toString`, `JSON.stringify` nor `console.log` shows it.
   */
  final class CardId private[PermissionsApi] (private val bits: () => (BigInt, BigInt)) {
    override def equals(other: Any): Boolean =
      other match {
        case that: CardId => bits() == that.bits()
        case _            => false
      }

    override def hashCode(): Int = bits().##

    override def toString: String = "CardId(<opaque>)"
  }

  private object CardId {
    def fromJs(raw: JsCardId): CardId = {
      val value = (BigInt(raw.uuid.highBits.toString), BigInt(raw.uuid.lowBits.toString))
      new CardId(() => value)
    }
  }

  // --- WIT: timestamp ---

  /** Expiry instant of a card: seconds and nanoseconds since the Unix epoch. */
  final case class Timestamp(seconds: Long, nanoseconds: Int)

  // --- WIT: duration ---

  /**
   * Lifespan of a derived card, carried to the host as an ISO-8601 duration in
   * the `PnDTnHnMnS` form, such as `"P30D"` or `"PT90.5S"`. The host validates
   * the value and accepts only day and time units.
   */
  final class Lifespan private (val iso8601: String) {
    override def equals(other: Any): Boolean =
      other match {
        case that: Lifespan => iso8601 == that.iso8601
        case _              => false
      }

    override def hashCode(): Int = iso8601.##

    override def toString: String = s"Lifespan($iso8601)"
  }

  object Lifespan {

    /**
     * An ISO-8601 duration in the `PnDTnHnMnS` form. The value must start with
     * the `P` designator; the host validates the rest.
     */
    def iso8601(value: String): Lifespan = {
      require(value.startsWith("P"), s"an ISO-8601 lifespan must start with 'P': $value")
      new Lifespan(value)
    }

    /** A lifespan of exactly `duration`, which must be positive. */
    def apply(duration: FiniteDuration): Lifespan = {
      require(duration.length > 0, s"a lifespan must be positive: $duration")
      val nanos   = BigInt(duration.toNanos)
      val seconds = nanos / 1000000000
      val rest    = nanos % 1000000000
      if (rest == 0) new Lifespan(s"PT${seconds}S")
      else {
        val fraction = f"${rest.toLong}%09d".reverse.dropWhile(_ == '0').reverse
        new Lifespan(s"PT$seconds.${fraction}S")
      }
    }
  }

  // --- WIT: pattern-grant / card-view ---

  /**
   * One grant pattern of a card lattice. `className` is the WIT `class` field.
   */
  final case class PatternGrant(
    className: String,
    owner: String,
    recipient: String,
    verb: String,
    resourceId: String
  )

  private object PatternGrantJs {
    def fromJs(raw: JsPatternGrant): PatternGrant =
      PatternGrant(raw.className, raw.owner, raw.recipient, raw.verb, raw.resourceId)

    def toJs(grant: PatternGrant): JsPatternGrant =
      JsPatternGrant(grant.className, grant.owner, grant.recipient, grant.verb, grant.resourceId)

    def toJsArray(grants: List[PatternGrant]): js.Array[JsPatternGrant] =
      js.Array(grants.map(toJs): _*)
  }

  /** The four grant lists of a card lattice, as returned by `inspect`. */
  final case class CardView(
    lowerPositive: List[PatternGrant],
    lowerNegative: List[PatternGrant],
    upperPositive: List[PatternGrant],
    upperNegative: List[PatternGrant]
  )

  private object CardViewJs {
    def fromJs(raw: JsCardView): CardView =
      CardView(
        raw.lowerPositive.toList.map(PatternGrantJs.fromJs),
        raw.lowerNegative.toList.map(PatternGrantJs.fromJs),
        raw.upperPositive.toList.map(PatternGrantJs.fromJs),
        raw.upperNegative.toList.map(PatternGrantJs.fromJs)
      )
  }

  /**
   * The requested lattice of a derivation.
   *
   * For `derive` on a card the positive lists are the parent grants to retain
   * and the negative lists are the denials to add. For [[deriveFromWallet]] and
   * [[deriveScope]] the four lists are the complete requested lattice. The host
   * validates every request against the parent cards; nothing in a `Bounds`
   * value carries authority.
   */
  final case class Bounds(
    lowerPositive: List[PatternGrant] = Nil,
    lowerNegative: List[PatternGrant] = Nil,
    upperPositive: List[PatternGrant] = Nil,
    upperNegative: List[PatternGrant] = Nil
  )

  object Bounds {
    val empty: Bounds = Bounds()
  }

  // --- WIT: wallet-version-token ---

  /**
   * Version of the caller's wallet. It changes whenever the set of installed
   * cards changes.
   */
  final case class WalletVersionToken(walletIdHash: Vector[Byte], generation: BigInt)

  // --- WIT: kernel-module ---

  /** A resource class registered in the permission kernel, with its verbs. */
  final case class KernelModule(className: String, verbs: List[String])

  // --- WIT: holder ---

  /**
   * The target of `install` on a card. The host currently accepts only agent
   * targets and returns [[PermissionError.NotPermitted]] for account and app
   * targets.
   */
  sealed trait Holder extends Product with Serializable

  object Holder {
    final case class Account(accountId: Uuid)               extends Holder
    final case class App(appId: Uuid)                       extends Holder
    final case class Agent(agentId: HostApi.AgentIdLiteral) extends Holder

    /** An agent holder from a component id and an agent id string. */
    def agent(componentId: HostApi.ComponentIdLiteral, agentId: String): Holder =
      Agent(JsAgentId(componentId, agentId))

    private[PermissionsApi] def toJs(holder: Holder): JsHolder =
      holder match {
        case Account(uuid) => JsHolder.account(uuidToJs(uuid))
        case App(uuid)     => JsHolder.app(uuidToJs(uuid))
        case Agent(id)     => JsHolder.agent(id)
      }

    private def uuidToJs(uuid: Uuid): JsUuid =
      JsUuid(js.BigInt(uuid.highBits.toString), js.BigInt(uuid.lowBits.toString))
  }

  // --- WIT: permission-error ---

  /** An expected failure of a `golem:permissions` operation. */
  sealed trait PermissionError extends Product with Serializable {
    def message: String
  }

  object PermissionError {
    final case class NotPermitted(message: String)              extends PermissionError
    final case class LowerBoundTooBroad(message: String)        extends PermissionError
    final case class UpperBoundTooBroad(message: String)        extends PermissionError
    final case class WalletMultiSourceRequired(message: String) extends PermissionError
    final case class CardRevoked(message: String)               extends PermissionError
    final case class CardExpired(message: String)               extends PermissionError
    final case class InstallRecipientMismatch(message: String)  extends PermissionError
    final case class UnknownResourceClass(message: String)      extends PermissionError
    final case class InvalidPattern(message: String)            extends PermissionError
    final case class InvalidVerb(message: String)               extends PermissionError
    final case class InvalidOwner(message: String)              extends PermissionError
    final case class InvalidRecipient(message: String)          extends PermissionError

    private val byTag: Map[String, String => PermissionError] = Map(
      "not-permitted"                -> NotPermitted.apply,
      "lower-bound-too-broad"        -> LowerBoundTooBroad.apply,
      "upper-bound-too-broad"        -> UpperBoundTooBroad.apply,
      "wallet-multi-source-required" -> WalletMultiSourceRequired.apply,
      "card-revoked"                 -> CardRevoked.apply,
      "card-expired"                 -> CardExpired.apply,
      "install-recipient-mismatch"   -> InstallRecipientMismatch.apply,
      "unknown-resource-class"       -> UnknownResourceClass.apply,
      "invalid-pattern"              -> InvalidPattern.apply,
      "invalid-verb"                 -> InvalidVerb.apply,
      "invalid-owner"                -> InvalidOwner.apply,
      "invalid-recipient"            -> InvalidRecipient.apply
    )

    /** The WIT case names of `permission-error` that this SDK recognises. */
    private[golem] val witTags: Set[String] = byTag.keySet

    /**
     * Decodes a thrown host value. Returns `None` unless the value is a
     * `{ tag, val }` object with a known tag and a string payload.
     */
    private[golem] def fromJs(thrown: Any): Option[PermissionError] =
      thrown match {
        case obj: js.Object =>
          val dynamic = obj.asInstanceOf[js.Dynamic]
          val tag     = dynamic.selectDynamic("tag").asInstanceOf[Any]
          val value   = dynamic.selectDynamic("val").asInstanceOf[Any]
          (tag, value) match {
            case (tag: String, message: String) => byTag.get(tag).map(_(message))
            case _                              => None
          }
        case _ => None
      }
  }

  // --- WIT: permission-card resource ---

  /**
   * A card this agent holds: a concrete [[PermissionCard]], a template
   * [[PolymorphicPermissionCard]] or an invocation-local [[ScopeCard]]. Each
   * wraps the opaque host resource and offers the borrowing operations; each
   * can be a parent in [[deriveScope]].
   */
  sealed trait Card {
    private[golem] def handle: GuestPermissionCardHandle

    /** True while the card has not been moved or released. */
    final def isPresent: Boolean = handle.isPresent

    /**
     * The identifier of this card. Traps if the host does not know the handle.
     */
    final def id: CardId =
      borrow(raw => CardId.fromJs(TypesModule.id(raw)))

    /**
     * The parent identifiers in the derivation graph; for a scope card, its
     * root cards. Traps if the host does not know the handle.
     */
    final def parents: List[CardId] =
      borrow(raw => TypesModule.parents(raw).toList.map(CardId.fromJs))

    /**
     * The expiry of the card, if it has a bounded lifespan. For a
     * [[ScopeCard]], the earliest expiry of its root cards. Traps if the host
     * does not know the handle.
     */
    final def expiresAt: Option[Timestamp] =
      borrow(raw =>
        TypesModule
          .expiresAt(raw)
          .toOption
          .map(ts => Timestamp(BigInt(ts.seconds.toString).toLong, ts.nanoseconds.toInt))
      )

    /** The complete grant lattice of the card. Requires `card:inspect`. */
    final def inspect(): Either[PermissionError, CardView] =
      borrow(raw => hostCall(CardViewJs.fromJs(InspectModule.inspectCard(raw))))

    /**
     * Drop the handle without revoking the card. Use it for handles that the
     * caller does not keep. Moves the card; a second call does nothing.
     */
    final def release(): Unit =
      handle.takeForRelease().foreach(PermissionsApi.dispose)

    /**
     * Borrow the raw resource for a host call that takes
     * `borrow<permission-card>`.
     */
    private[golem] final def borrowRaw[A](f: js.Any => A): A =
      borrow(raw => f(raw))

    protected final def borrow[A](f: JsPermissionCardResource => A): A =
      handle
        .withHandle(raw => f(raw.asInstanceOf[JsPermissionCardResource]))
        .getOrElse(throw new IllegalStateException(CARD_MOVED))
  }

  /**
   * The card object that claimed each raw resource. A raw resource wraps into
   * at most one card object while that card holds it; a card that moved its
   * handle no longer counts. A holder wraps into at most one card object ever.
   */
  private val cardsByResource = js.Dynamic.newInstance(js.Dynamic.global.WeakMap)()

  private def isResourceObject(raw: Any): Boolean =
    raw != null && (js.typeOf(raw) == "object" || js.typeOf(raw) == "function")

  private def claimantOf(raw: Any): Option[Card] =
    if (!isResourceObject(raw)) None
    else cardsByResource.get(raw.asInstanceOf[js.Any]).asInstanceOf[js.UndefOr[Card]].toOption

  private def holderOf(raw: Any): Option[Card] =
    claimantOf(raw).filter { card =>
      card.handle != null && card.handle.withHandle(_.asInstanceOf[AnyRef] eq raw.asInstanceOf[AnyRef]).contains(true)
    }

  private val RESOURCE_HELD = "this permission-card resource is held by another card object"
  private val HOLDER_OWNED  =
    "this permission-card holder already belongs to another card object, also when that card moved its handle"

  private def claimed(card: Card, handle: GuestPermissionCardHandle): GuestPermissionCardHandle = {
    val raw = handle.withHandle(identity)
    if (raw.exists(holderOf(_).isDefined)) throw new IllegalArgumentException(RESOURCE_HELD)
    if (!handle.claimForCard()) throw new IllegalArgumentException(HOLDER_OWNED)
    raw.filter(isResourceObject).foreach(r => cardsByResource.set(r.asInstanceOf[js.Any], card.asInstanceOf[js.Any]))
    handle
  }

  /**
   * Give a lowered card handle back to its holder after the call never reached
   * the host. The handle is restored only when the raw resource still belongs
   * to this holder: its claiming card wraps this holder, no card claimed
   * either, or the raw value is not a resource object that cards can claim.
   * Otherwise another card owns the resource now, and the handle is neither
   * restored nor dropped. A restored handle whose card was released meanwhile
   * is dropped.
   */
  private[golem] def returnCardHandle(handle: GuestPermissionCardHandle, raw: Any): Unit = {
    val belongsHere = claimantOf(raw) match {
      case Some(card) => card.handle eq handle
      case None       => !isResourceObject(raw) || !handle.isClaimedByCard
    }
    if (belongsHere && !handle.giveBack(raw)) dispose(raw)
  }

  /**
   * An unforgeable concrete permission card held by this agent: every grant
   * names concrete owners and resources.
   *
   * The card wraps an owned host resource. It has no public constructor and no
   * data codec; the only schema codec moves the handle into an RPC value tree,
   * so a card can be sent exactly once. Its schema is
   * `permission-card { polymorphic: false }`. A raw host resource wraps into at
   * most one card object while that card holds it, and a schema holder into at
   * most one card object ever.
   */
  final class PermissionCard private[golem] (handle0: GuestPermissionCardHandle) extends Card {
    private[golem] val handle: GuestPermissionCardHandle = claimed(this, handle0)

    /**
     * Derive a child card with a narrower lattice. The positive lists of
     * `bounds` are the grants of this card to retain, the negative lists are
     * the denials to add. Requires `card:derive`. This card stays usable.
     */
    def derive(bounds: Bounds, lifespan: Option[Lifespan] = None): Either[PermissionError, PermissionCard] =
      borrow(deriveChild(_, bounds, lifespan))

    /**
     * Revoke this card and every card derived from it. Returns the number of
     * revoked cards. Requires `card:revoke`. Moves the card, also when the host
     * returns an error.
     */
    def revoke(): Either[PermissionError, Long] = revokeCard(handle)

    /**
     * Install this card into the wallet of `target`. Requires `card:install`.
     * Moves the card, also when the host returns an error.
     */
    def install(target: Holder): Either[PermissionError, Unit] = installCard(handle, target)

    override def toString: String = "PermissionCard(<opaque>)"
  }

  object PermissionCard {
    private[golem] def fromRaw(raw: JsPermissionCardResource): PermissionCard =
      new PermissionCard(GuestPermissionCardHandle.fromRaw(raw))

    private val codecs = new CardCodecs[PermissionCard](
      "PermissionCard",
      GuestPermissionCardHandle.nonPolymorphicSpec,
      new PermissionCard(_)
    )

    /**
     * Schema codec for agent and tool signatures. Encoding moves the handle
     * into the value tree, so a card can be sent exactly once.
     */
    implicit val intoSchema: IntoSchema[PermissionCard] = codecs.intoSchema

    /**
     * Wraps a decoded handle. A raw resource that another present card object
     * holds is rejected, also when it arrives through a hand-built JS value
     * tree, so this decoder cannot alias an SDK card. A JS value that is not a
     * host resource decodes into a card whose every use the host rejects.
     * Authority is always checked by the host: the worst case is a second
     * wrapper around a resource that user code imported through its own raw JS
     * bindings, never new authority.
     */
    implicit val fromSchema: FromSchema[PermissionCard] = codecs.fromSchema

    implicit val concreteCodec: ConcreteCodec[PermissionCard] = codecs.concreteCodec
  }

  /**
   * An unforgeable template permission card held by this agent: its grants may
   * contain slot variables such as `?agent` and `?env`, which the host resolves
   * for each holder.
   *
   * It offers the same operations as a [[PermissionCard]];
   * [[PolymorphicPermissionCard.derive]] returns a concrete [[PermissionCard]],
   * because the host resolves the template parent for the holder. Its schema is
   * `permission-card { polymorphic: true }`. The non-forgeability rules of
   * [[PermissionCard]] apply unchanged. The SDK offers no conversion between
   * the two types; a decoded card takes the type that the decoder declares.
   */
  final class PolymorphicPermissionCard private[golem] (handle0: GuestPermissionCardHandle) extends Card {
    private[golem] val handle: GuestPermissionCardHandle = claimed(this, handle0)

    /**
     * Derive a concrete child card with a narrower lattice. The positive lists
     * of `bounds` are the grants of this card to retain, the negative lists are
     * the denials to add. The host resolves the slot variables of this card for
     * the holder. Requires `card:derive`. This card stays usable.
     */
    def derive(bounds: Bounds, lifespan: Option[Lifespan] = None): Either[PermissionError, PermissionCard] =
      borrow(deriveChild(_, bounds, lifespan))

    /**
     * Revoke this card and every card derived from it. Returns the number of
     * revoked cards. Requires `card:revoke`. Moves the card, also when the host
     * returns an error.
     */
    def revoke(): Either[PermissionError, Long] = revokeCard(handle)

    /**
     * Install this card into the wallet of `target`. Requires `card:install`.
     * Moves the card, also when the host returns an error.
     */
    def install(target: Holder): Either[PermissionError, Unit] = installCard(handle, target)

    override def toString: String = "PolymorphicPermissionCard(<opaque>)"
  }

  object PolymorphicPermissionCard {
    private[golem] def fromRaw(raw: JsPermissionCardResource): PolymorphicPermissionCard =
      new PolymorphicPermissionCard(GuestPermissionCardHandle.fromRaw(raw))

    private val codecs = new CardCodecs[PolymorphicPermissionCard](
      "PolymorphicPermissionCard",
      PermissionCardSpec(polymorphic = true),
      new PolymorphicPermissionCard(_)
    )

    /**
     * Schema codec for agent and tool signatures. Encoding moves the handle
     * into the value tree, so a card can be sent exactly once.
     */
    implicit val intoSchema: IntoSchema[PolymorphicPermissionCard] = codecs.intoSchema

    /**
     * Wraps a decoded handle with the rules of [[PermissionCard.fromSchema]].
     * The declared type chooses the wrapper; the host refuses an agent call
     * input or output whose card does not match the declared kind.
     */
    implicit val fromSchema: FromSchema[PolymorphicPermissionCard] = codecs.fromSchema

    implicit val concreteCodec: ConcreteCodec[PolymorphicPermissionCard] = codecs.concreteCodec
  }

  /**
   * The schema codecs of a sendable card type with one permission-card spec.
   */
  private final class CardCodecs[C <: Card](
    name: String,
    spec: PermissionCardSpec,
    wrap: GuestPermissionCardHandle => C
  ) {
    val intoSchema: IntoSchema[C] =
      new IntoSchema[C] {
        override lazy val graph: SchemaGraph =
          SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.PermissionCardType(spec)))

        override def toValue(card: C): SchemaValue =
          SchemaValue.PermissionCardHandle(card.handle)
      }

    val fromSchema: FromSchema[C] =
      new FromSchema[C] {
        override def fromValue(value: SchemaValue): Either[FromSchemaError, C] =
          value match {
            case SchemaValue.PermissionCardHandle(h) =>
              try Right(wrap(h))
              catch { case error: IllegalArgumentException => Left(FromSchemaError(error.getMessage)) }
            case other =>
              Left(FromSchemaError(s"expected permission-card handle for $name, got $other"))
          }
      }

    val concreteCodec: ConcreteCodec[C] =
      GuestPermissionCardHandle.concreteCodec(spec).xmap(decoded, _.handle)

    private def decoded(handle: GuestPermissionCardHandle): C =
      try wrap(handle)
      catch { case error: IllegalArgumentException => throw SchemaDecodeError(error.getMessage) }
  }

  private def deriveChild(
    raw: JsPermissionCardResource,
    bounds: Bounds,
    lifespan: Option[Lifespan]
  ): Either[PermissionError, PermissionCard] =
    hostCall(
      PermissionCard.fromRaw(
        DeriveModule.derive(
          raw,
          PatternGrantJs.toJsArray(bounds.lowerPositive),
          PatternGrantJs.toJsArray(bounds.lowerNegative),
          PatternGrantJs.toJsArray(bounds.upperPositive),
          PatternGrantJs.toJsArray(bounds.upperNegative),
          lifespanToJs(lifespan)
        )
      )
    )

  private def revokeCard(handle: GuestPermissionCardHandle): Either[PermissionError, Long] = {
    val raw = take(handle)
    hostCall(RevokeModule.revokeCard(raw).toLong)
  }

  private def installCard(handle: GuestPermissionCardHandle, target: Holder): Either[PermissionError, Unit] = {
    val jsTarget = Holder.toJs(target)
    val raw      = take(handle)
    hostCall(WalletModule.installCard(raw, jsTarget))
  }

  private def take(handle: GuestPermissionCardHandle): JsPermissionCardResource =
    handle.take() match {
      case Some(raw) => raw.asInstanceOf[JsPermissionCardResource]
      case None      => throw new IllegalStateException(CARD_MOVED)
    }

  /**
   * An invocation-local scope card: derived with [[deriveScope]], or lent to
   * the current invocation and listed by [[wallet]] as a [[WalletCard.Scope]].
   *
   * Attach it to an awaited RPC call with the generated client's
   * `withScopeCard` or `withDefaultScopeCard`; attaching borrows the card. A
   * scope card is valid only during the invocation that derived it or that it
   * was lent to. After that invocation the handle stays safe to hold and to
   * [[Card.release]], but attaching it (also as a client default) makes awaited
   * calls fail with `denied`, and using it as a [[deriveScope]] parent returns
   * [[PermissionError.NotPermitted]]. It cannot be revoked, installed, derived
   * from persistently or sent as an RPC argument, and it has no codec.
   *
   * [[Card.expiresAt]] is the earliest expiry of the root cards of this scope
   * card, or none when no root card expires. The scope card stops authorizing
   * then.
   */
  final class ScopeCard private[golem] (handle0: GuestPermissionCardHandle) extends Card {
    private[golem] val handle: GuestPermissionCardHandle = claimed(this, handle0)

    override def toString: String = "ScopeCard(<opaque>)"
  }

  // --- WIT: wallet-card ---

  /**
   * A card in the caller's wallet, labelled with its kind by the host. Each
   * case carries the card type of its kind.
   */
  sealed trait WalletCard extends Product with Serializable {

    /** The card of this entry. */
    def card: Card
  }

  object WalletCard {

    /** A concrete card: every grant names concrete owners and resources. */
    final case class Concrete(card: PermissionCard) extends WalletCard

    /**
     * A template card: its grants may contain slot variables (`?agent`, `?env`,
     * ...), which resolve for each holder.
     */
    final case class Template(card: PolymorphicPermissionCard) extends WalletCard

    /**
     * The scope card lent to the current invocation. The host lists it only
     * when the caller holds `card:inspect`, and retires it at the end of the
     * invocation.
     */
    final case class Scope(card: ScopeCard) extends WalletCard

    private val byTag: Map[String, GuestPermissionCardHandle => WalletCard] = Map(
      "concrete" -> (handle => Concrete(new PermissionCard(handle))),
      "template" -> (handle => Template(new PolymorphicPermissionCard(handle))),
      "scope"    -> (handle => Scope(new ScopeCard(handle)))
    )

    /** The WIT case names of `wallet-card` that this SDK recognises. */
    private[golem] val witTags: Set[String] = byTag.keySet

    private[PermissionsApi] def fromJs(raw: JsWalletCard): WalletCard =
      byTag.get(raw.tag) match {
        case Some(wrap) => wrap(GuestPermissionCardHandle.fromRaw(raw.card))
        case None       => throw new IllegalStateException(s"unknown wallet-card case: ${raw.tag}")
      }
  }

  // --- Module-level operations ---

  /**
   * All cards in the caller's wallet, each labelled with its kind. During an
   * invocation that received a scope card, the list also contains that card as
   * a [[WalletCard.Scope]] when the caller holds `card:inspect`. Each card is
   * an owned handle; call [[Card.release]] on the cards that the caller does
   * not keep.
   *
   * When the list cannot be wrapped, because of an unknown case or a resource
   * that another card object holds, no card is returned: the cards already
   * wrapped are released, the other handles are dropped unless another card
   * object holds them, and the failure is rethrown.
   */
  def wallet(): List[WalletCard] = {
    val entries = WalletModule.selfWallet().toList
    entries.find(entry => !WalletCard.witTags.contains(entry.tag)).foreach { unknown =>
      disposeUnheld(entries.map(_.card))
      throw new IllegalStateException(s"unknown wallet-card case: ${unknown.tag}")
    }
    val wrapped = scala.collection.mutable.ListBuffer.empty[WalletCard]
    try {
      entries.foreach(entry => wrapped += WalletCard.fromJs(entry))
      wrapped.toList
    } catch {
      case NonFatal(error) =>
        val unwrapped = entries.drop(wrapped.size).map(_.card).filter(raw => holderOf(raw).isEmpty)
        wrapped.foreach(entry => quietly(entry.card.release()))
        disposeUnheld(unwrapped)
        throw error
    }
  }

  /**
   * Drop each distinct raw resource that no card object holds. A failing drop
   * does not stop the others.
   */
  private def disposeUnheld(raws: List[Any]): Unit =
    raws
      .filter(raw => isResourceObject(raw) && holderOf(raw).isEmpty)
      .foldLeft(List.empty[Any])((seen, raw) =>
        if (seen.exists(_.asInstanceOf[AnyRef] eq raw.asInstanceOf[AnyRef])) seen else raw :: seen
      )
      .reverse
      .foreach(raw => quietly(dispose(raw)))

  private def quietly(action: => Unit): Unit =
    try action
    catch { case NonFatal(_) => () }

  /** The current version of the caller's wallet. */
  def walletVersion(): WalletVersionToken = {
    val raw = WalletModule.selfVersion()
    WalletVersionToken(
      raw.walletIdHash.toArray.iterator.map(_.toByte).toVector,
      BigInt(raw.generation.toString)
    )
  }

  /**
   * Derive a card from the caller's wallet. `bounds` is the complete requested
   * lattice. Requires `card:derive`.
   */
  def deriveFromWallet(bounds: Bounds, lifespan: Option[Lifespan] = None): Either[PermissionError, PermissionCard] =
    hostCall(
      PermissionCard.fromRaw(
        DeriveModule.deriveFromWallet(
          PatternGrantJs.toJsArray(bounds.lowerPositive),
          PatternGrantJs.toJsArray(bounds.lowerNegative),
          PatternGrantJs.toJsArray(bounds.upperPositive),
          PatternGrantJs.toJsArray(bounds.upperNegative),
          lifespanToJs(lifespan)
        )
      )
    )

  /**
   * Derive an invocation-local [[ScopeCard]] from one or more parents, which
   * can be persistent cards and scope cards. `bounds` is the complete requested
   * lattice. The parents are borrowed and stay usable. Requires `card:derive`.
   */
  def deriveScope(parents: List[Card], bounds: Bounds): Either[PermissionError, ScopeCard] = {
    val rawParents = js.Array[JsPermissionCardResource]()
    parents.foreach(parent => rawParents.push(parent.borrowRaw(_.asInstanceOf[JsPermissionCardResource])))
    hostCall(
      new ScopeCard(
        GuestPermissionCardHandle.fromRaw(
          DeriveModule.deriveScope(
            rawParents,
            PatternGrantJs.toJsArray(bounds.lowerPositive),
            PatternGrantJs.toJsArray(bounds.lowerNegative),
            PatternGrantJs.toJsArray(bounds.upperPositive),
            PatternGrantJs.toJsArray(bounds.upperNegative)
          )
        )
      )
    )
  }

  /** Every resource class of the permission kernel, with its verbs. */
  def listModules(): List[KernelModule] =
    KernelIntrospectionModule.listModules().toList.map(m => KernelModule(m.className, m.verbs.toList))

  /** Validate a grant against the schema of the permission kernel. */
  def validateGrant(grant: PatternGrant): Either[PermissionError, Unit] =
    hostCall(KernelIntrospectionModule.validateGrant(PatternGrantJs.toJs(grant)))

  // --- Internals ---

  private val CARD_MOVED =
    "permission card has already been moved (revoked, installed, released or sent) and can no longer be used"

  private def lifespanToJs(lifespan: Option[Lifespan]): js.UndefOr[JsPermissionDuration] =
    lifespan.fold[js.UndefOr[JsPermissionDuration]](js.undefined)(l => JsPermissionDuration(l.iso8601))

  private def hostCall[A](thunk: => A): Either[PermissionError, A] =
    try Right(thunk)
    catch {
      case e @ js.JavaScriptException(thrown) =>
        PermissionError.fromJs(thrown) match {
          case Some(error) => Left(error)
          case None        => throw e
        }
    }

  private[golem] def dispose(raw: Any): Unit = {
    val symbol   = js.Dynamic.global.Symbol.selectDynamic("dispose")
    val disposer = js.Dynamic.global.Reflect.applyDynamic("get")(raw.asInstanceOf[js.Any], symbol)
    if (!js.isUndefined(disposer))
      js.Dynamic.global.Reflect.applyDynamic("apply")(disposer, raw.asInstanceOf[js.Any], js.Array())
    ()
  }

  @js.native
  @JSImport("golem:permissions/types@0.1.0", JSImport.Namespace)
  private object TypesModule extends js.Object {
    def id(card: JsPermissionCardResource): JsCardId                                 = js.native
    def parents(card: JsPermissionCardResource): js.Array[JsCardId]                  = js.native
    def expiresAt(card: JsPermissionCardResource): js.UndefOr[JsPermissionTimestamp] = js.native
  }

  @js.native
  @JSImport("golem:permissions/inspect@0.1.0", JSImport.Namespace)
  private object InspectModule extends js.Object {
    def inspectCard(card: JsPermissionCardResource): JsCardView = js.native
  }

  @js.native
  @JSImport("golem:permissions/derive@0.1.0", JSImport.Namespace)
  private object DeriveModule extends js.Object {
    def derive(
      parent: JsPermissionCardResource,
      lowerPositiveToRetain: js.Array[JsPatternGrant],
      lowerNegativeToAdd: js.Array[JsPatternGrant],
      upperPositiveToRetain: js.Array[JsPatternGrant],
      upperNegativeToAdd: js.Array[JsPatternGrant],
      lifespan: js.UndefOr[JsPermissionDuration]
    ): JsPermissionCardResource = js.native

    def deriveFromWallet(
      lowerPositive: js.Array[JsPatternGrant],
      lowerNegative: js.Array[JsPatternGrant],
      upperPositive: js.Array[JsPatternGrant],
      upperNegative: js.Array[JsPatternGrant],
      lifespan: js.UndefOr[JsPermissionDuration]
    ): JsPermissionCardResource = js.native

    def deriveScope(
      parents: js.Array[JsPermissionCardResource],
      lowerPositive: js.Array[JsPatternGrant],
      lowerNegative: js.Array[JsPatternGrant],
      upperPositive: js.Array[JsPatternGrant],
      upperNegative: js.Array[JsPatternGrant]
    ): JsPermissionCardResource = js.native
  }

  @js.native
  @JSImport("golem:permissions/revoke@0.1.0", JSImport.Namespace)
  private object RevokeModule extends js.Object {
    def revokeCard(card: JsPermissionCardResource): Double = js.native
  }

  @js.native
  @JSImport("golem:permissions/wallet@0.1.0", JSImport.Namespace)
  private object WalletModule extends js.Object {
    def selfWallet(): js.Array[JsWalletCard]                                = js.native
    def selfVersion(): JsWalletVersionToken                                 = js.native
    def installCard(card: JsPermissionCardResource, target: JsHolder): Unit = js.native
  }

  @js.native
  @JSImport("golem:permissions/kernel-introspection@0.1.0", JSImport.Namespace)
  private object KernelIntrospectionModule extends js.Object {
    def listModules(): js.Array[JsKernelModule]    = js.native
    def validateGrant(grant: JsPatternGrant): Unit = js.native
  }
}
