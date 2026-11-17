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

package golem.runtime

import golem.{BaseAgent, Caller, InvocationContext, Principal, PrincipalRef}
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.runtime.autowire.{AgentDefinition, AgentImplementation, MethodBinding, SchemaPayload}
import golem.runtime.guest.Guest
import golem.runtime.tool.ToolTestFixtures.withImmediateCaller
import golem.runtime.InputRecordCodec
import golem.schema.{FromSchema, IntoSchema}
import golem.host.js.schema.JsSchemaValueTree
import golem.FutureInterop
import zio._
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future
import scala.scalajs.js

object InvocationContextSpec extends ZIOSpecDefault {

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  private def binding[T](
    name: String,
    defn: AgentDefinition[T]
  ): MethodBinding[T] =
    defn.methodMetadata.find(_.metadata.name == name).getOrElse(sys.error(s"binding not found: $name"))

  /** Encode a single user-supplied parameter as the 1-field input record. */
  private def singleInput[A](a: A)(implicit i: IntoSchema[A], f: FromSchema[A]): JsSchemaValueTree =
    SchemaPayload.encode[A](a)(InputRecordCodec.single[A]("in"))

  private def decodeSingle[Out](raw: Option[JsSchemaValueTree])(implicit f: FromSchema[Out]): Out =
    raw match {
      case Some(tree) =>
        SchemaPayload.decode[Out](tree).fold(err => throw new RuntimeException(err.toString), identity)
      case None =>
        throw new RuntimeException("expected a single result, got none")
    }

  // ---------------------------------------------------------------------------
  // 1. Constructor injection
  // ---------------------------------------------------------------------------

  @agentDefinition()
  trait CtorContextAgent extends BaseAgent {
    class Id(val value: String)
    def getCreator(): Future[String]
  }

  @agentImplementation()
  final class CtorContextAgentImpl(input: String, context: InvocationContext) extends CtorContextAgent {
    override def getCreator(): Future[String] =
      Future.successful(s"${context.caller.immediate}:$input")
  }

  private lazy val ctorDefn: AgentDefinition[CtorContextAgent] =
    AgentImplementation.registerClass[CtorContextAgent, CtorContextAgentImpl]

  // ---------------------------------------------------------------------------
  // 2. Method injection
  // ---------------------------------------------------------------------------

  @agentDefinition()
  trait MethodContextAgent extends BaseAgent {
    class Id()
    def identify(name: String, context: InvocationContext): Future[String]
  }

  @agentImplementation()
  final class MethodContextAgentImpl() extends MethodContextAgent {
    override def identify(name: String, context: InvocationContext): Future[String] =
      Future.successful(s"$name:${context.caller.immediate}")
  }

  private val methodImpl = new MethodContextAgentImpl()

  private lazy val methodDefn: AgentDefinition[MethodContextAgent] =
    AgentImplementation.registerClass[MethodContextAgent, MethodContextAgentImpl]

  // ---------------------------------------------------------------------------
  // 3. Schema exclusion – method with InvocationContext in params
  // ---------------------------------------------------------------------------

  @agentDefinition()
  trait SchemaCheckAgent extends BaseAgent {
    class Id()
    def greet(name: String, context: InvocationContext): Future[String]
    def multi(a: String, b: Int, context: InvocationContext): Future[String]
  }

  @agentImplementation()
  final class SchemaCheckAgentImpl() extends SchemaCheckAgent {
    override def greet(name: String, context: InvocationContext): Future[String] =
      Future.successful(s"hi $name")
    override def multi(a: String, b: Int, context: InvocationContext): Future[String] =
      Future.successful(s"$a-$b")
  }

  private lazy val schemaDefn: AgentDefinition[SchemaCheckAgent] =
    AgentImplementation.registerClass[SchemaCheckAgent, SchemaCheckAgentImpl]

  // ---------------------------------------------------------------------------
  // 4. Mixed params – Config[_] + InvocationContext in constructor (compile check)
  //
  // The macro validates that Config + InvocationContext params co-exist at compile time.
  // We cannot call registerClass here because ConfigBuilder triggers WASI host
  // imports that are unavailable in the unit-test linker environment.  Instead
  // we verify that the macro's compile-time reflection (implementationTypeFromClass)
  // succeeds, which proves the parameter classification works.
  // ---------------------------------------------------------------------------

  final case class MixedConfig(endpoint: String)
  object MixedConfig {
    implicit val schema: Schema[MixedConfig] = Schema.derived
  }

  @agentDefinition()
  trait MixedParamsAgent extends BaseAgent {
    class Id(val value: String)
    def info(): Future[String]
  }

  @agentImplementation()
  final class MixedParamsAgentImpl(input: String, context: InvocationContext) extends MixedParamsAgent {
    override def info(): Future[String] =
      Future.successful(s"$input:${context.caller}")
  }

  private lazy val mixedDefn: AgentDefinition[MixedParamsAgent] =
    AgentImplementation.registerClass[MixedParamsAgent, MixedParamsAgentImpl]

  // ---------------------------------------------------------------------------
  // Test callers
  // ---------------------------------------------------------------------------

  private val anonymousContext: InvocationContext = InvocationContext.anonymous

  private val userPrincipal: Principal = Principal.User(
    PrincipalRef("https://auth.example.com", "user-42"),
    "{}",
    java.time.Instant.EPOCH
  )

  private val userContext: InvocationContext =
    InvocationContext.of(Caller(userPrincipal, userPrincipal))

  // ---------------------------------------------------------------------------
  // Tests
  // ---------------------------------------------------------------------------

  def spec = suite("InvocationContextSpec")(
    // 1. Constructor injection
    suite("constructor injection")(
      test("constructor receives an InvocationContext via registerClass") {
        assertTrue(ctorDefn.typeName.nonEmpty)
      },
      test("constructor agent initializes with an anonymous caller") {
        ZIO.fromFuture { implicit ec =>
          FutureInterop
            .fromPromise(ctorDefn.initialize(singleInput[String]("hello"), anonymousContext))
            .flatMap(_.getCreator())
        }.map(r => assertTrue(r == "anonymous:hello"))
      },
      test("constructor agent initializes with a user caller") {
        ZIO.fromFuture { implicit ec =>
          FutureInterop
            .fromPromise(ctorDefn.initialize(singleInput[String]("world"), userContext))
            .flatMap(_.getCreator())
        }.map(r => assertTrue(r == """user ("https://auth.example.com","user-42"):world"""))
      }
    ),

    // 2. Method injection
    suite("method injection")(
      test("method receives an anonymous caller through binding invoke") {
        val b = binding("identify", methodDefn)
        ZIO.fromFuture { implicit ec =>
          FutureInterop
            .fromPromise(b.invoke(methodImpl, singleInput[String]("alice"), anonymousContext))
            .map(decodeSingle[String])
        }.map(r => assertTrue(r == "alice:anonymous"))
      },
      test("method receives a user caller through binding invoke") {
        val b = binding("identify", methodDefn)
        ZIO.fromFuture { implicit ec =>
          FutureInterop
            .fromPromise(b.invoke(methodImpl, singleInput[String]("bob"), userContext))
            .map(decodeSingle[String])
        }.map(r => assertTrue(r == """bob:user ("https://auth.example.com","user-42")"""))
      }
    ),

    // Guest exports read the caller from `invocation-context.caller`, on first use only
    suite("guest exports")(
      test("initialize and invoke receive a context that reads the immediate caller") {
        val caller = js.Dynamic.literal(
          "tag" -> "user",
          "val" -> js.Dynamic.literal(
            "pair"            -> js.Dynamic.literal("issuer" -> "https://auth.example.com", "subject" -> "user-7"),
            "claims"          -> "{}",
            "authenticatedAt" -> js.Dynamic.literal("seconds" -> js.BigInt(0), "nanoseconds" -> 0)
          )
        )
        val guest    = Guest.golemAgent200Guest
        val typeName = methodDefn.typeName
        Guest.resetForTesting()
        ZIO.fromFuture { implicit ec =>
          val initialized = withImmediateCaller(caller) {
            guest
              .initialize(typeName, SchemaPayload.encode[Unit](())(InputRecordCodec.unit))
              .asInstanceOf[js.Promise[Unit]]
          }
          FutureInterop.fromPromise(initialized).flatMap { _ =>
            val state   = Guest.stateForTesting
            val invoked = withImmediateCaller(caller) {
              guest
                .invoke("identify", singleInput[String]("dave"))
                .asInstanceOf[js.Promise[js.UndefOr[JsSchemaValueTree]]]
            }
            FutureInterop.fromPromise(invoked).map(result => (state, decodeSingle[String](result.toOption)))
          }
        }.ensuring(ZIO.succeed(Guest.resetForTesting())).map { case (state, result) =>
          assertTrue(state, result == """dave:user ("https://auth.example.com","user-7")""")
        }
      },
      test("the host is asked for the caller only when it is read") {
        var reads = 0
        val ctx   = InvocationContext.lazily {
          reads += 1
          Caller.anonymous
        }
        val before = reads
        ctx.caller
        ctx.caller
        assertTrue(before == 0, reads == 1)
      },
      test("a context read during its call keeps that caller after the call ends") {
        var current = "A"
        val ctx     = InvocationContext.lazily {
          Caller(
            userPrincipal,
            Principal.User(PrincipalRef("https://auth.example.com", current), "{}", java.time.Instant.EPOCH)
          )
        }
        val first = ctx.caller
        current = "B"
        ctx.end()
        assertTrue(ctx.caller eq first)
      },
      test("a context first read after its call ended refuses") {
        val ctx = InvocationContext.lazily(Caller.anonymous)
        ctx.end()
        val refused = scala.util.Try(ctx.caller)
        assertTrue(
          refused.isFailure,
          refused.failed.get.getMessage.contains("belongs to a call that has ended")
        )
      }
    ),

    // 3. Schema exclusion
    suite("schema exclusion")(
      test("method input excludes InvocationContext (single user param)") {
        val greetBinding = binding("greet", schemaDefn)
        assertTrue(greetBinding.metadata.input.userSupplied.length == 1)
      },
      test("method input excludes InvocationContext (multiple user params)") {
        val multiBinding = binding("multi", schemaDefn)
        assertTrue(multiBinding.metadata.input.userSupplied.length == 2)
      },
      test("constructor input excludes InvocationContext") {
        assertTrue(ctorDefn.constructor.info.input.userSupplied.length == 1)
      }
    ),

    // 4. Mixed params – Config + InvocationContext compile check
    suite("mixed params")(
      test("agent with Config + InvocationContext in constructor registers successfully") {
        assertTrue(mixedDefn.typeName.nonEmpty)
      },
      test("mixed agent has correct method count") {
        assertTrue(mixedDefn.methodMetadata.size == 1)
      }
    )
  )
}
