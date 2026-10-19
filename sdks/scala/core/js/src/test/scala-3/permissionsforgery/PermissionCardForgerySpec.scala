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

package permissionsforgery

import golem.runtime.annotations.toolDefinition
import golem.runtime.macros.WireToolMacro
import golem.schema.wire.{WitSchemaGraph, WitSchemaTypeBody}
import zio.test._

import scala.compiletime.testing.{Error, typeCheckErrors}

@toolDefinition(name = "permission-card-kinds-probe", version = "1.0.0")
trait PermissionCardKindsProbe {
  def exchange(card: golem.host.PermissionsApi.PermissionCard): golem.host.PermissionsApi.PermissionCard
  def exchangeTemplate(
    card: golem.host.PermissionsApi.PolymorphicPermissionCard
  ): golem.host.PermissionsApi.PolymorphicPermissionCard
  def exchangePersistent(card: golem.host.PermissionsApi.PermissionCard): golem.host.PermissionsApi.PersistentCard
}

/**
 * User code outside the `golem` package cannot create, re-wrap or serialize a
 * card.
 */
object PermissionCardForgerySpec extends ZIOSpecDefault {

  private def inaccessible(errors: List[Error]): Boolean =
    errors.exists(_.message.contains("cannot be accessed"))

  private def noInstance(errors: List[Error]): Boolean =
    errors.exists(_.message.contains("No given instance"))

  override def spec = suite("PermissionCardForgerySpec")(
    test("a card cannot be constructed from a handle, a raw resource or a card id") {
      val constructor: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        new PermissionCard(null)
      """)
      val fromRaw: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        PermissionCard.fromRaw(null)
      """)
      val handle: List[Error] = typeCheckErrors("""
        golem.schema.GuestPermissionCardHandle.fromRaw("card-1")
      """)
      val cardId: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.CardId
        new CardId(() => (BigInt(1), BigInt(2)))
      """)
      val rawHandle: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        def leak(card: PermissionCard) = card.handle.take()
      """)
      val borrowRaw: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        def leak(card: PermissionCard) = card.borrowRaw(identity)
      """)
      assertTrue(
        inaccessible(constructor),
        inaccessible(fromRaw),
        inaccessible(handle),
        inaccessible(cardId),
        inaccessible(rawHandle),
        inaccessible(borrowRaw)
      )
    },
    test("cards and card ids have no data codec") {
      val cardSchema: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        summon[zio.blocks.schema.Schema[PermissionCard]]
      """)
      val cardIdSchema: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.CardId
        summon[zio.blocks.schema.Schema[CardId]]
      """)
      val cardIdInto: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.CardId
        summon[golem.schema.IntoSchema[CardId]]
      """)
      val cardIdFrom: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.CardId
        summon[golem.schema.FromSchema[CardId]]
      """)
      val cardIdAccess: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.CardId
        def bits(id: CardId) = id.bits()
      """)
      val capabilityCodec: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        summon[golem.schema.IntoSchema[PermissionCard]]
        summon[golem.schema.FromSchema[PermissionCard]]
        summon[golem.schema.wire.ConcreteCodec[PermissionCard]]
      """)
      assertTrue(
        noInstance(cardSchema),
        noInstance(cardIdSchema),
        noInstance(cardIdInto),
        noInstance(cardIdFrom),
        inaccessible(cardIdAccess),
        capabilityCodec.isEmpty
      ).label((cardSchema ++ capabilityCodec).map(_.message).mkString("; "))
    },
    test("a scope card cannot be built, persisted, sent or confused with a permission card") {
      val construct: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.ScopeCard
        new ScopeCard(null)
      """)
      val revoke: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.ScopeCard
        def leak(card: ScopeCard) = card.revoke()
      """)
      val install: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{Holder, ScopeCard}
        def leak(card: ScopeCard, holder: Holder) = card.install(holder)
      """)
      val derive: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{Bounds, ScopeCard}
        def leak(card: ScopeCard) = card.derive(Bounds.empty)
      """)
      val into: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.ScopeCard
        summon[golem.schema.IntoSchema[ScopeCard]]
      """)
      val from: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.ScopeCard
        summon[golem.schema.FromSchema[ScopeCard]]
      """)
      val wire: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.ScopeCard
        summon[golem.schema.wire.ConcreteCodec[ScopeCard]]
      """)
      val persistentAsScope: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        import golem.runtime.rpc.AgentClientRuntime.WireResolvedAgent
        def attach(agent: WireResolvedAgent[Any], card: PermissionCard) = agent.withScopeCard(card)
      """)
      val scopeAsPersistent: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{PermissionCard, ScopeCard}
        def widen(card: ScopeCard): PermissionCard = card
      """)
      val scopeParent: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{Bounds, PermissionCard, ScopeCard, deriveScope}
        def nest(scope: ScopeCard, card: PermissionCard) = deriveScope(List(scope, card), Bounds.empty)
      """)
      assertTrue(
        inaccessible(construct),
        revoke.nonEmpty,
        install.nonEmpty,
        derive.nonEmpty,
        noInstance(into),
        noInstance(from),
        noInstance(wire),
        persistentAsScope.exists(_.message.contains("Found:")),
        scopeAsPersistent.exists(_.message.contains("Found:")),
        scopeParent.isEmpty
      ).label(
        (revoke ++ install ++ derive ++ persistentAsScope ++ scopeAsPersistent ++ scopeParent)
          .map(_.message)
          .mkString("; ")
      )
    },
    test("a template card cannot be built or serialized, and is not typed as another card type") {
      val construct: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PolymorphicPermissionCard
        new PolymorphicPermissionCard(null)
      """)
      val fromRaw: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PolymorphicPermissionCard
        PolymorphicPermissionCard.fromRaw(null)
      """)
      val rawHandle: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PolymorphicPermissionCard
        def leak(card: PolymorphicPermissionCard) = card.handle.take()
      """)
      val borrowRaw: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PolymorphicPermissionCard
        def leak(card: PolymorphicPermissionCard) = card.borrowRaw(identity)
      """)
      val dataCodec: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PolymorphicPermissionCard
        summon[zio.blocks.schema.Schema[PolymorphicPermissionCard]]
      """)
      val capabilityCodec: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PolymorphicPermissionCard
        summon[golem.schema.IntoSchema[PolymorphicPermissionCard]]
        summon[golem.schema.FromSchema[PolymorphicPermissionCard]]
        summon[golem.schema.wire.ConcreteCodec[PolymorphicPermissionCard]]
      """)
      val templateAsConcrete: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{PermissionCard, PolymorphicPermissionCard}
        def widen(card: PolymorphicPermissionCard): PermissionCard = card
      """)
      val concreteAsTemplate: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{PermissionCard, PolymorphicPermissionCard}
        def widen(card: PermissionCard): PolymorphicPermissionCard = card
      """)
      val scopeAsTemplate: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{PolymorphicPermissionCard, ScopeCard}
        def widen(card: ScopeCard): PolymorphicPermissionCard = card
      """)
      val templateAsScope: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PolymorphicPermissionCard
        import golem.runtime.rpc.AgentClientRuntime.WireResolvedAgent
        def attach(agent: WireResolvedAgent[Any], card: PolymorphicPermissionCard) = agent.withScopeCard(card)
      """)
      val mislabelled: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{PermissionCard, WalletCard}
        def label(card: PermissionCard) = WalletCard.Template(card)
      """)
      val derivesConcrete: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.{Bounds, PersistentCard, PermissionError, PolymorphicPermissionCard}
        def child(card: PolymorphicPermissionCard): Either[PermissionError, PersistentCard] = card.derive(Bounds.empty)
      """)
      assertTrue(
        inaccessible(construct),
        inaccessible(fromRaw),
        inaccessible(rawHandle),
        inaccessible(borrowRaw),
        noInstance(dataCodec),
        capabilityCodec.isEmpty,
        templateAsConcrete.exists(_.message.contains("Found:")),
        concreteAsTemplate.exists(_.message.contains("Found:")),
        scopeAsTemplate.exists(_.message.contains("Found:")),
        templateAsScope.exists(_.message.contains("Found:")),
        mislabelled.exists(_.message.contains("Found:")),
        derivesConcrete.isEmpty
      ).label(
        (capabilityCodec ++ templateAsConcrete ++ concreteAsTemplate ++ scopeAsTemplate ++ templateAsScope ++
          mislabelled ++ derivesConcrete).map(_.message).mkString("; ")
      )
    },
    test("an agent signature can carry a permission card but not a scope card") {
      val card: List[Error] = typeCheckErrors("""
        import golem.BaseAgent
        import golem.host.PermissionsApi.PermissionCard
        import golem.runtime.annotations.agentDefinition
        import scala.concurrent.Future

        @agentDefinition()
        trait PermissionCardReceiver extends BaseAgent {
          class Id(val name: String)
          def accept(card: PermissionCard): Future[PermissionCard]
          def acceptTemplate(card: golem.host.PermissionsApi.PolymorphicPermissionCard): Future[Boolean]
        }

        golem.runtime.macros.AgentClientMacro.wireType[PermissionCardReceiver]
      """)
      val scope: List[Error] = typeCheckErrors("""
        import golem.BaseAgent
        import golem.host.PermissionsApi.ScopeCard
        import golem.runtime.annotations.agentDefinition
        import scala.concurrent.Future

        @agentDefinition()
        trait ScopeCardReceiver extends BaseAgent {
          class Id(val name: String)
          def accept(card: ScopeCard): Future[Boolean]
        }

        golem.runtime.macros.AgentClientMacro.wireType[ScopeCardReceiver]
      """)
      assertTrue(card.isEmpty, scope.nonEmpty).label((card ++ scope).map(_.message).mkString("; "))
    },
    test("tool metadata declares a template card as polymorphic and a concrete card as not") {
      def cardSpecs(graph: WitSchemaGraph): List[Boolean] =
        graph.typeNodes.toList.map(_.body).collect { case WitSchemaTypeBody.PermissionCardType(spec) =>
          spec.polymorphic
        }
      assertTrue(
        cardSpecs(WireToolMacro.inputGraph[PermissionCardKindsProbe](List("exchange"))) == List(false),
        cardSpecs(WireToolMacro.inputGraph[PermissionCardKindsProbe](List("exchange-template"))) == List(true),
        cardSpecs(WireToolMacro.inputGraph[PermissionCardKindsProbe](List("exchange-persistent"))) == List(false)
      )
    },
    test("generated agent and tool clients accept a card in signatures") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        import golem.runtime.annotations.*
        import golem.runtime.macros.WireToolMacro
        import golem.schema.wire.ConcreteCodec

        @toolDefinition(name = "permission-card-probe", version = "1.0.0")
        trait PermissionCardProbe {
          def exchange(card: PermissionCard): PermissionCard
          def exchangeTemplate(
            card: golem.host.PermissionsApi.PolymorphicPermissionCard
          ): golem.host.PermissionsApi.PolymorphicPermissionCard
        }

        WireToolMacro.inputGraph[PermissionCardProbe](List("exchange"))
        WireToolMacro.inputGraph[PermissionCardProbe](List("exchange-template"))
        ConcreteCodec.derived[PermissionCard]
        ConcreteCodec.derived[golem.host.PermissionsApi.PolymorphicPermissionCard]
      """)
      assertTrue(errors.isEmpty).label(errors.map(_.message).mkString("; "))
    }
  )
}
