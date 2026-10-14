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

import zio.test._

import scala.compiletime.testing.{Error, typeCheckErrors}

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
    test("generated agent and tool clients accept a card in signatures") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.host.PermissionsApi.PermissionCard
        import golem.runtime.annotations.*
        import golem.runtime.macros.WireToolMacro
        import golem.schema.wire.ConcreteCodec

        @toolDefinition(name = "permission-card-probe", version = "1.0.0")
        trait PermissionCardProbe {
          def exchange(card: PermissionCard): PermissionCard
        }

        WireToolMacro.inputGraph[PermissionCardProbe](List("exchange"))
        ConcreteCodec.derived[PermissionCard]
      """)
      assertTrue(errors.isEmpty).label(errors.map(_.message).mkString("; "))
    }
  )
}
