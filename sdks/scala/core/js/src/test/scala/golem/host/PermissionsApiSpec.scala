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

import golem.host.PermissionsApi._
import golem.host.js.{JsComponentId, JsPermissionCardResource, JsUuid}
import golem.schema.{FromSchema, GuestPermissionCardHandle, IntoSchema, PermissionCardSpec, SchemaTypeBody, SchemaValue}
import golem.schema.wire.{ConcreteCodec, WitSchemaTypeBody}
import zio.test._

import scala.concurrent.duration._
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport
import scala.util.Try

object PermissionsApiSpec extends ZIOSpecDefault {

  @js.native
  @JSImport("fs", JSImport.Namespace)
  private object Fs extends js.Object {
    def existsSync(path: String): Boolean                    = js.native
    def readFileSync(path: String, encoding: String): String = js.native
  }

  private final class FakeHost {
    val calls         = scala.collection.mutable.ListBuffer.empty[(String, js.Array[js.Any])]
    val disposed      = scala.collection.mutable.ListBuffer.empty[String]
    private val impls = js.Dictionary.empty[js.Function]

    def on(name: String)(f: js.Array[js.Any] => js.Any): FakeHost = {
      impls(name) = js.Any.fromFunction1[js.Array[js.Any], js.Any] { args =>
        calls += ((name, args))
        f(args)
      }
      this
    }

    def install(): Unit = {
      val host = js.Dynamic.literal()
      impls.foreach { case (name, impl) =>
        val variadic: js.Function = js.Dynamic.global.Function
          .applyDynamic("apply")(
            null,
            js.Array[js.Any]("impl", "return function(...args) { return impl(args); }")
          )
          .asInstanceOf[js.Dynamic]
          .apply(impl)
          .asInstanceOf[js.Function]
        host.updateDynamic(name)(variadic)
      }
      js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(host)
    }

    def count(name: String): Int = calls.count(_._1 == name)

    def args(name: String): js.Array[js.Any] =
      calls.reverseIterator.find(_._1 == name).map(_._2).getOrElse(throw new NoSuchElementException(name))

    def rawCard(name: String): js.Dynamic = {
      val card = js.Dynamic.literal(name = name)
      val sym  = js.Dynamic.global.Symbol.selectDynamic("dispose")
      js.Dynamic.global.Reflect.applyDynamic("set")(
        card,
        sym,
        js.Any.fromFunction0(() => disposed += name)
      )
      card
    }
  }

  private def cardIdJs(high: Int, low: Int): js.Dynamic =
    js.Dynamic.literal(uuid = js.Dynamic.literal(highBits = js.BigInt(high), lowBits = js.BigInt(low)))

  private def grantJs(cls: String, verb: String, resourceId: String): js.Dynamic =
    js.Dynamic.literal(class_ = cls, owner = "*", recipient = "*", verb = verb, resourceId = resourceId)

  private def error(tag: String, message: String): js.Dynamic =
    js.Dynamic.literal(tag = tag, `val` = message)

  private def throwJs(value: js.Any): Nothing = throw js.JavaScriptException(value)

  private def card(raw: js.Dynamic): PermissionCard =
    PermissionCard.fromRaw(raw.asInstanceOf[JsPermissionCardResource])

  private def grantNames(value: js.Any): List[String] =
    value.asInstanceOf[js.Array[js.Dynamic]].toList.map(g => s"${g.class_}:${g.verb}:${g.resourceId}")

  private val inspectGrant = PatternGrant("card", "*", "*", "inspect", "*")
  private val deriveGrant  = PatternGrant("card", "*", "*", "derive", "*")
  private val toolGrant    = PatternGrant("tool", "*", "*", "invoke", "/private/**")
  private val agentGrant   = PatternGrant("agent", "*", "*", "invoke", "Target")

  private def witPermissionErrorCases(): List[String] = {
    val relative = "wit/deps/golem-permissions/types.wit"
    var dir      = js.Dynamic.global.process.cwd().asInstanceOf[String]
    while (!Fs.existsSync(s"$dir/$relative") && dir.contains("/")) dir = dir.substring(0, dir.lastIndexOf('/'))
    val wit   = Fs.readFileSync(s"$dir/$relative", "utf8")
    val start = wit.indexOf("variant permission-error {")
    val body  = wit.substring(wit.indexOf('{', start) + 1, wit.indexOf('}', start))
    body.split(",").toList.map(_.trim).filter(_.nonEmpty).map(_.takeWhile(_ != '('))
  }

  override def spec = suite("PermissionsApiSpec")(
    test("permission-error decodes every WIT case and the case count matches the WIT variant") {
      val witCases = witPermissionErrorCases()
      val decoded  = witCases.flatMap(tag => PermissionError.fromJs(error(tag, s"m-$tag")))
      assertTrue(
        witCases.size == 12,
        PermissionError.witTags == witCases.toSet,
        decoded.size == witCases.size,
        decoded.map(_.message) == witCases.map(tag => s"m-$tag"),
        decoded.contains(PermissionError.InstallRecipientMismatch("m-install-recipient-mismatch")),
        decoded.contains(PermissionError.NotPermitted("m-not-permitted"))
      )
    },
    test("wallet wraps owned handles and card ids compare by identity of the host id") {
      val host   = new FakeHost
      val root   = host.rawCard("root")
      val child  = host.rawCard("child")
      val ids    = Map("root" -> cardIdJs(1, 2), "child" -> cardIdJs(3, 4), "alias" -> cardIdJs(1, 2))
      val parent = Map("root" -> js.Array[js.Any](), "child" -> js.Array[js.Any](cardIdJs(1, 2)))
      host
        .on("selfWallet")(_ => js.Array[js.Any](root, child))
        .on("id")(args => ids(args(0).asInstanceOf[js.Dynamic].name.asInstanceOf[String]))
        .on("parents")(args => parent(args(0).asInstanceOf[js.Dynamic].name.asInstanceOf[String]))
        .on("expiresAt")(args =>
          if (args(0).asInstanceOf[js.Dynamic].name.asInstanceOf[String] == "child")
            js.Dynamic.literal(seconds = js.BigInt("1900000000"), nanoseconds = 5)
          else js.undefined
        )
        .install()
      val cards                        = PermissionsApi.wallet()
      val rootCard :: childCard :: Nil = cards: @unchecked
      assertTrue(
        cards.forall(_.isPresent),
        childCard.parents == List(rootCard.id),
        rootCard.id != childCard.id,
        rootCard.id == card(host.rawCard("alias")).id,
        rootCard.id.hashCode == card(host.rawCard("alias")).id.hashCode,
        rootCard.expiresAt.isEmpty,
        childCard.expiresAt.contains(Timestamp(1900000000L, 5)),
        rootCard.id.toString == "CardId(<opaque>)"
      )
    },
    test("derive sends retained and added grants in WIT order with the lifespan and keeps the parent") {
      val host        = new FakeHost
      val parent      = host.rawCard("parent")
      val result      = host.rawCard("result")
      var derivations = 0
      host
        .on("derive") { _ =>
          derivations += 1
          if (derivations == 1) result else host.rawCard(s"result-$derivations")
        }
        .install()
      val parentCard = card(parent)
      val bounds     = Bounds(
        lowerPositive = List(inspectGrant),
        lowerNegative = List(toolGrant),
        upperPositive = List(inspectGrant, deriveGrant),
        upperNegative = List(agentGrant)
      )
      val derived = parentCard.derive(bounds, Some(Lifespan(90.5.seconds)))
      val args    = host.args("derive")
      assertTrue(
        derived.isRight,
        derived.toOption.get.isPresent,
        parentCard.isPresent,
        args(0) eq parent,
        grantNames(args(1)) == List("card:inspect:*"),
        grantNames(args(2)) == List("tool:invoke:/private/**"),
        grantNames(args(3)) == List("card:inspect:*", "card:derive:*"),
        grantNames(args(4)) == List("agent:invoke:Target"),
        args(5).asInstanceOf[js.Dynamic].iso8601.asInstanceOf[String] == "PT90.5S",
        js.isUndefined { parentCard.derive(Bounds.empty); host.args("derive")(5) }
      )
    },
    test("an expected host error becomes Left and an unknown thrown value is rethrown") {
      val host = new FakeHost
      host
        .on("deriveFromWallet")(_ => throwJs(error("not-permitted", "card:derive missing")))
        .on("validateGrant")(_ => throwJs(error("no-such-case", "x")))
        .on("inspectCard")(_ => throwJs(js.Dynamic.newInstance(js.Dynamic.global.Error)("trap")))
        .install()
      val denied   = PermissionsApi.deriveFromWallet(Bounds(lowerPositive = List(inspectGrant)))
      val unknown  = Try(PermissionsApi.validateGrant(inspectGrant))
      val trapped  = Try(card(host.rawCard("c")).inspect())
      val wrongVal = PermissionError.fromJs(js.Dynamic.literal(tag = "not-permitted", `val` = 3))
      assertTrue(
        denied == Left(PermissionError.NotPermitted("card:derive missing")),
        unknown.failed.toOption.exists(_.isInstanceOf[js.JavaScriptException]),
        trapped.failed.toOption.exists(_.isInstanceOf[js.JavaScriptException]),
        wrongVal.isEmpty,
        PermissionError.fromJs("not-permitted").isEmpty
      )
    },
    test("revoke moves the card before the host call, also when the host returns an error") {
      val host = new FakeHost
      host
        .on("revokeCard")(args =>
          if (args(0).asInstanceOf[js.Dynamic].name.asInstanceOf[String] == "ok") 3
          else throwJs(error("card-revoked", "already revoked"))
        )
        .on("id")(_ => cardIdJs(1, 1))
        .install()
      val ok           = card(host.rawCard("ok"))
      val failed       = card(host.rawCard("failed"))
      val okResult     = ok.revoke()
      val failedResult = failed.revoke()
      val reuseId      = Try(failed.id)
      val reuseRevoke  = Try(ok.revoke())
      assertTrue(
        okResult == Right(3L),
        failedResult == Left(PermissionError.CardRevoked("already revoked")),
        !ok.isPresent,
        !failed.isPresent,
        reuseId.failed.toOption.exists(_.isInstanceOf[IllegalStateException]),
        reuseRevoke.failed.toOption.exists(_.isInstanceOf[IllegalStateException]),
        host.count("revokeCard") == 2,
        host.count("id") == 0
      )
    },
    test("install sends the holder variant and maps a recipient mismatch") {
      val host = new FakeHost
      host
        .on("installCard")(args =>
          if (args(1).asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "agent")
            throwJs(error("install-recipient-mismatch", "card recipient is another agent"))
          else js.undefined
        )
        .install()
      val componentId = JsComponentId(JsUuid(js.BigInt(7), js.BigInt(8)))
      val agentCard   = card(host.rawCard("a"))
      val agentResult = agentCard.install(Holder.agent(componentId, "Target(\"b\")"))
      val agentArgs   = host.args("installCard")
      val appCard     = card(host.rawCard("b"))
      val appResult   = appCard.install(Holder.App(golem.Uuid(BigInt(5), BigInt(6))))
      val appArgs     = host.args("installCard")
      val holder      = agentArgs(1).asInstanceOf[js.Dynamic]
      assertTrue(
        agentResult == Left(PermissionError.InstallRecipientMismatch("card recipient is another agent")),
        !agentCard.isPresent,
        holder.`val`.agentId.asInstanceOf[String] == "Target(\"b\")",
        holder.`val`.componentId.uuid.lowBits.toString == "8",
        appResult == Right(()),
        appArgs(1).asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "app",
        appArgs(1).asInstanceOf[js.Dynamic].`val`.highBits.toString == "5",
        !appCard.isPresent
      )
    },
    test("release drops the wallet handle once and leaves the card moved") {
      val host = new FakeHost
      host.on("id")(_ => cardIdJs(1, 1)).install()
      val held = card(host.rawCard("held"))
      held.release()
      held.release()
      val reuse = Try(held.id)
      assertTrue(
        host.disposed.toList == List("held"),
        !held.isPresent,
        reuse.failed.toOption.exists(_.isInstanceOf[IllegalStateException]),
        host.count("id") == 0
      )
    },
    test("deriveScope returns a scope card, borrows persistent and scope parents in order, rejects a moved parent") {
      val host     = new FakeHost
      val a        = host.rawCard("a")
      val b        = host.rawCard("b")
      val scopeRaw = host.rawCard("scope")
      var scopes   = 0
      host
        .on("deriveScope") { _ =>
          scopes += 1
          if (scopes == 1) scopeRaw else host.rawCard(s"scope-$scopes")
        }
        .install()
      val first      = card(a)
      val second     = card(b)
      val scope      = PermissionsApi.deriveScope(List(second, first), Bounds(lowerPositive = List(inspectGrant)))
      val parents    = host.args("deriveScope")(0).asInstanceOf[js.Array[js.Any]]
      val scopeCard  = scope.toOption.get
      val nested     = PermissionsApi.deriveScope(List(scopeCard, second), Bounds.empty)
      val nestedArgs = host.args("deriveScope")(0).asInstanceOf[js.Array[js.Any]]
      first.release()
      val moved = Try(PermissionsApi.deriveScope(List(second, first), Bounds.empty))
      assertTrue(
        scope.isRight,
        scopeCard.isInstanceOf[ScopeCard],
        scopeCard.toString == "ScopeCard(<opaque>)",
        parents.length == 2,
        parents(0) eq b,
        parents(1) eq a,
        nested.isRight,
        nestedArgs(0) eq scopeRaw,
        nestedArgs(1) eq b,
        scopeCard.isPresent,
        second.isPresent,
        moved.failed.toOption.exists(_.isInstanceOf[IllegalStateException]),
        host.count("deriveScope") == 2
      )
    },
    test("walletVersion, listModules and validateGrant decode the host values") {
      val host = new FakeHost
      host
        .on("selfVersion")(_ =>
          js.Dynamic.literal(
            walletIdHash = new js.typedarray.Uint8Array(js.Array[Short](1, 255, 3)),
            generation = js.BigInt("18446744073709551615")
          )
        )
        .on("listModules")(_ =>
          js.Array[js.Any](js.Dynamic.literal(className = "card", verbs = js.Array("derive", "inspect")))
        )
        .on("validateGrant")(args =>
          if (args(0).asInstanceOf[js.Dynamic].verb.asInstanceOf[String] == "fly")
            throwJs(error("invalid-verb", "fly"))
          else js.undefined
        )
        .install()
      val version = PermissionsApi.walletVersion()
      assertTrue(
        version == WalletVersionToken(Vector[Byte](1, -1, 3), BigInt("18446744073709551615")),
        PermissionsApi.listModules() == List(KernelModule("card", List("derive", "inspect"))),
        PermissionsApi.validateGrant(inspectGrant) == Right(()),
        PermissionsApi.validateGrant(inspectGrant.copy(verb = "fly")) == Left(PermissionError.InvalidVerb("fly")),
        grantNames(js.Array(host.args("validateGrant")(0))) == List("card:fly:*")
      )
    },
    test("Lifespan renders native durations as ISO-8601 and rejects invalid values") {
      assertTrue(
        Lifespan(90.5.seconds).iso8601 == "PT90.5S",
        Lifespan(3.days).iso8601 == "PT259200S",
        Lifespan(1.nanosecond).iso8601 == "PT0.000000001S",
        Lifespan.iso8601("P2DT3H").iso8601 == "P2DT3H",
        Try(Lifespan(0.seconds)).isFailure,
        Try(Lifespan(-1.second)).isFailure,
        Try(Lifespan.iso8601("30D")).isFailure,
        Lifespan.iso8601("P30D") == Lifespan.iso8601("P30D")
      )
    },
    test("the schema codecs declare a monomorphic card and refuse to wrap a held handle a second time") {
      val host    = new FakeHost
      val sent    = card(host.rawCard("sent"))
      val into    = implicitly[IntoSchema[PermissionCard]]
      val back    = implicitly[FromSchema[PermissionCard]]
      val node    = into.toValue(sent)
      val aliased = back.fromValue(node)
      val codec   = implicitly[ConcreteCodec[PermissionCard]]
      val fresh   = back.fromValue(
        SchemaValue.PermissionCardHandle(GuestPermissionCardHandle.fromRaw(host.rawCard("fresh")))
      )
      assertTrue(
        into.graph.root.body == SchemaTypeBody.PermissionCardType(PermissionCardSpec(polymorphic = false)),
        aliased.left.exists(_.toString.contains("resource is held by another card object")),
        fresh.exists(_.isPresent),
        sent.isPresent,
        back.fromValue(SchemaValue.StringValue("card-id")).isLeft,
        codec.graph.typeNodes(codec.graph.root).body ==
          WitSchemaTypeBody.PermissionCardType(PermissionCardSpec(polymorphic = false))
      )
    },
    test("lowering a card through SchemaWireInterop moves it, and the handle can be given back") {
      val host    = new FakeHost
      val raw     = host.rawCard("lowered")
      val sent    = card(raw)
      val codec   = implicitly[ConcreteCodec[PermissionCard]]
      val wit     = codec.encodeValue(sent)
      val lowered = SchemaWireInterop.valueTreeToJs(wit)
      val node    = lowered.valueNodes(lowered.root).asInstanceOf[js.Dynamic]
      val moved   = !sent.isPresent
      val reuse   = Try(sent.inspect())
      val resend  = Try(SchemaWireInterop.valueTreeToJs(codec.encodeValue(sent)))
      val tag     = node.tag.asInstanceOf[String]
      val carried = node.`val`.asInstanceOf[js.Any] eq raw
      SchemaWireInterop.returnCapabilityHandles(wit, lowered)
      val returned = sent.isPresent && sent.borrowRaw(_ eq raw)
      assertTrue(
        tag == "permission-card-handle",
        carried,
        moved,
        reuse.failed.toOption.exists(_.isInstanceOf[IllegalStateException]),
        resend.isFailure,
        returned,
        js.isUndefined(node.`val`)
      )
    },
    test("a raw resource wraps into one present card even through two hand-built value trees") {
      val host   = new FakeHost
      val raw    = host.rawCard("shared")
      def tree() = SchemaWireInterop.valueTreeFromJs(
        golem.host.js.schema.JsSchemaValueTree(
          js.Array(golem.host.js.schema.JsSchemaValueNode.permissionCardHandle(raw)),
          0
        )
      )
      val codec   = implicitly[ConcreteCodec[PermissionCard]]
      val first   = codec.decode(tree())
      val second  = Try(codec.decode(tree()))
      val decoded = implicitly[FromSchema[PermissionCard]]
        .fromValue(golem.schema.wire.SchemaWire.schemaValueFromWit(tree()))
      first.release()
      val afterRelease = Try(codec.decode(tree()))
      assertTrue(
        second.failed.toOption.exists(_.isInstanceOf[golem.schema.SchemaDecodeError]),
        second.failed.toOption.exists(_.getMessage.contains("resource is held by another card object")),
        decoded.left.exists(_.toString.contains("resource is held by another card object")),
        afterRelease.isSuccess
      )
    },
    test("a card holder exposed by its schema value never backs a second card, also while the card is lowered") {
      val host    = new FakeHost
      val raw     = host.rawCard("exposed")
      val first   = card(raw)
      val value   = implicitly[IntoSchema[PermissionCard]].toValue(first)
      val wit     = implicitly[ConcreteCodec[PermissionCard]].encodeValue(first)
      val lowered = SchemaWireInterop.valueTreeToJs(wit)
      val second  = implicitly[FromSchema[PermissionCard]].fromValue(value)
      SchemaWireInterop.returnCapabilityHandles(wit, lowered)
      val again = implicitly[FromSchema[PermissionCard]].fromValue(value)
      assertTrue(
        second.left.exists(_.toString.contains("holder already belongs to another card object")),
        again.isLeft,
        first.isPresent,
        first.borrowRaw(_ eq raw),
        host.disposed.isEmpty
      )
    },
    test("a lowered handle whose resource another card claimed meanwhile is neither restored nor dropped") {
      val host    = new FakeHost
      val raw     = host.rawCard("claimed-in-flight")
      val first   = card(raw)
      val wit     = implicitly[ConcreteCodec[PermissionCard]].encodeValue(first)
      val lowered = SchemaWireInterop.valueTreeToJs(wit)
      val other   = implicitly[ConcreteCodec[PermissionCard]].decode(
        SchemaWireInterop.valueTreeFromJs(
          golem.host.js.schema.JsSchemaValueTree(
            js.Array(golem.host.js.schema.JsSchemaValueNode.permissionCardHandle(raw)),
            0
          )
        )
      )
      SchemaWireInterop.returnCapabilityHandles(wit, lowered)
      assertTrue(
        !first.isPresent,
        other.isPresent,
        other.borrowRaw(_ eq raw),
        host.disposed.isEmpty
      )
    },
    test("failed wire decodes and released values leave a card-owned holder untouched") {
      val host        = new FakeHost
      val raw         = host.rawCard("kept")
      val owner       = card(raw)
      val unreachable = Try(
        golem.schema.wire.SchemaWire.schemaValueFromWit(
          golem.schema.wire.WitSchemaValueTree(
            Vector(
              golem.schema.wire.WitSchemaValueNode.StringValue("root"),
              golem.schema.wire.WitSchemaValueNode.PermissionCardHandle(owner.handle)
            ),
            0
          )
        )
      )
      golem.schema.wire.SchemaWire.releaseOwned(SchemaValue.PermissionCardHandle(owner.handle))
      val keptAfterRelease = owner.isPresent && owner.borrowRaw(_ eq raw)
      golem.schema.wire.SchemaWire.releaseOwned(
        SchemaValue.PermissionCardHandle(owner.handle),
        includeCardOwned = true
      )
      assertTrue(
        unreachable.isFailure,
        keptAfterRelease,
        !owner.isPresent
      )
    },
    test("a lowered handle whose raw value is not a resource object is given back") {
      val holder = GuestPermissionCardHandle.fromRaw("not-a-resource")
      val codec  = implicitly[ConcreteCodec[PermissionCard]]
      val asCard = codec.decode(
        golem.schema.wire
          .WitSchemaValueTree(Vector(golem.schema.wire.WitSchemaValueNode.PermissionCardHandle(holder)), 0)
      )
      val wit     = codec.encodeValue(asCard)
      val lowered = SchemaWireInterop.valueTreeToJs(wit)
      val moved   = !asCard.isPresent
      SchemaWireInterop.returnCapabilityHandles(wit, lowered)
      assertTrue(moved, asCard.isPresent, asCard.borrowRaw(raw => (raw: Any) == "not-a-resource"))
    },
    test("a rejected decode leaves the card that owns the holder untouched") {
      val host  = new FakeHost
      val raw   = host.rawCard("owned")
      val owner = card(raw)
      val tree  = golem.schema.wire.WitSchemaValueTree(
        Vector(golem.schema.wire.WitSchemaValueNode.PermissionCardHandle(owner.handle)),
        0
      )
      val rejected = Try(implicitly[ConcreteCodec[PermissionCard]].decode(tree))
      assertTrue(
        rejected.failed.toOption.exists(_.isInstanceOf[golem.schema.SchemaDecodeError]),
        owner.isPresent,
        owner.borrowRaw(_ eq raw)
      )
    },
    test("a card released while it is lowered into a call is dropped, not given back") {
      val host    = new FakeHost
      val sent    = card(host.rawCard("in-flight"))
      val wit     = implicitly[ConcreteCodec[PermissionCard]].encodeValue(sent)
      val lowered = SchemaWireInterop.valueTreeToJs(wit)
      sent.release()
      val droppedEarly = host.disposed.toList
      SchemaWireInterop.returnCapabilityHandles(wit, lowered)
      assertTrue(
        droppedEarly.isEmpty,
        host.disposed.toList == List("in-flight"),
        !sent.isPresent
      )
    },
    test("a card id never shows its value in toString, JSON or console output") {
      val host = new FakeHost
      host.on("id")(_ => cardIdJs(987654321, 123456789)).install()
      val id      = card(host.rawCard("secret-id")).id
      val json    = js.JSON.stringify(id.asInstanceOf[js.Any])
      val inspect = Util.inspect(id.asInstanceOf[js.Any], js.Dynamic.literal(depth = 10, showHidden = true))
      val shown   = List(id.toString, s"$id", String.valueOf(json), inspect)
      assertTrue(
        shown.forall(text => !text.contains("987654321") && !text.contains("123456789")),
        id == card(host.rawCard("same-id")).id
      )
    }
  ) @@ TestAspect.sequential

  @js.native
  @JSImport("util", JSImport.Namespace)
  private object Util extends js.Object {
    def inspect(value: js.Any, options: js.Any): String = js.native
  }
}
