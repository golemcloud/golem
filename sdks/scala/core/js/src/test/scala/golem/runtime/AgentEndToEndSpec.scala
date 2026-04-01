/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime

import golem.runtime.autowire.{AgentImplementation, HostPayload, MethodBinding}
import golem.runtime.Sum
import golem.{BaseAgent, Principal}
import golem.runtime.annotations.{DurabilityMode, agentDefinition, agentImplementation}
import golem.FutureInterop
import zio._
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future
import scala.scalajs.js

object AgentEndToEndSpec extends ZIOSpecDefault {

  // ---------------------------------------------------------------------------
  // Fixture types
  // ---------------------------------------------------------------------------

  final case class DeepNested(label: String, values: List[Int])
  object DeepNested { implicit val schema: Schema[DeepNested] = Schema.derived }

  final case class Outer(name: String, inner: DeepNested)
  object Outer { implicit val schema: Schema[Outer] = Schema.derived }

  // ---------------------------------------------------------------------------
  // Agent with many method signatures for roundtrip testing
  // ---------------------------------------------------------------------------

  @agentDefinition("E2eBroad", mode = DurabilityMode.Durable)
  trait BroadAgent extends BaseAgent {
    class Id()
    def echo(in: String): Future[String]
    def add(in: Sum): Future[Int]
    def echoInt(in: Int): Future[Int]
    def echoBoolean(in: Boolean): Future[Boolean]
    def echoOptionSome(in: Option[String]): Future[Option[String]]
    def echoOptionNone(in: Option[String]): Future[Option[String]]
    def echoList(in: List[Int]): Future[List[Int]]
    def echoListEmpty(in: List[Int]): Future[List[Int]]
    def echoNested(in: Outer): Future[Outer]
    def multiParam(a: String, b: Int): Future[String]
    def asyncVoid(in: String): Future[Unit]
    def echoLong(in: Long): Future[Long]
    def echoDouble(in: Double): Future[Double]
  }

  @agentImplementation()
  final class BroadAgentImpl() extends BroadAgent {
    override def echo(in: String): Future[String]                           = Future.successful(s"hello $in")
    override def add(in: Sum): Future[Int]                                  = Future.successful(in.a + in.b)
    override def echoInt(in: Int): Future[Int]                              = Future.successful(in)
    override def echoBoolean(in: Boolean): Future[Boolean]                  = Future.successful(in)
    override def echoOptionSome(in: Option[String]): Future[Option[String]] = Future.successful(in)
    override def echoOptionNone(in: Option[String]): Future[Option[String]] = Future.successful(in)
    override def echoList(in: List[Int]): Future[List[Int]]                 = Future.successful(in)
    override def echoListEmpty(in: List[Int]): Future[List[Int]]            = Future.successful(in)
    override def echoNested(in: Outer): Future[Outer]                       = Future.successful(in)
    override def multiParam(a: String, b: Int): Future[String]              = Future.successful(s"$a-$b")
    override def asyncVoid(in: String): Future[Unit]                        = Future.successful(())
    override def echoLong(in: Long): Future[Long]                           = Future.successful(in)
    override def echoDouble(in: Double): Future[Double]                     = Future.successful(in)
  }

  private lazy val broadDefn = AgentImplementation.registerClass[BroadAgent, BroadAgentImpl]
  private lazy val broadImpl = new BroadAgentImpl()

  private val testPrincipal: Principal = Principal.Anonymous

  private def liftEither[A](e: Either[String, A]): Future[A] =
    e.fold(err => Future.failed(js.JavaScriptException(err)), Future.successful)

  private def binding[T](
    name: String,
    defn: golem.runtime.autowire.AgentDefinition[T]
  ): MethodBinding[T] =
    defn.methodMetadata.find(_.metadata.name == name).getOrElse(sys.error(s"binding not found: $name"))

  private def roundtrip[In: Schema, Out: Schema](
    methodName: String,
    input: In,
    expected: Out
  ): ZIO[Any, Throwable, TestResult] = {
    val b = binding(methodName, broadDefn)
    ZIO.fromFuture { implicit ec =>
      for {
        payload <- liftEither(HostPayload.encode[In](input))
        raw     <- FutureInterop.fromPromise(b.invoke(broadImpl, payload, testPrincipal))
        decoded <- liftEither(HostPayload.decode[Out](raw))
      } yield decoded
    }.map(decoded => assertTrue(decoded == expected))
  }

  // ---------------------------------------------------------------------------
  // Tests
  // ---------------------------------------------------------------------------

  def spec = suite("AgentEndToEndSpec")(
    test("echo string roundtrips through binding") {
      roundtrip[String, String]("echo", "world", "hello world")
    },
    test("case class payload roundtrips through binding") {
      roundtrip[Sum, Int]("add", Sum(2, 3), 5)
    },
    test("Int roundtrips through binding") {
      roundtrip[Int, Int]("echoInt", 42, 42)
    },
    test("Boolean roundtrips through binding") {
      roundtrip[Boolean, Boolean]("echoBoolean", true, true)
    },
    test("Long roundtrips through binding") {
      roundtrip[Long, Long]("echoLong", 9876543210L, 9876543210L)
    },
    test("Double roundtrips through binding") {
      roundtrip[Double, Double]("echoDouble", 3.14159, 3.14159)
    },
    test("Option[String] Some roundtrips through binding") {
      roundtrip[Option[String], Option[String]]("echoOptionSome", Some("present"), Some("present"))
    },
    test("Option[String] None roundtrips through binding") {
      roundtrip[Option[String], Option[String]]("echoOptionNone", None, None)
    },
    test("List[Int] non-empty roundtrips through binding") {
      roundtrip[List[Int], List[Int]]("echoList", List(1, 2, 3), List(1, 2, 3))
    },
    test("List[Int] empty roundtrips through binding") {
      roundtrip[List[Int], List[Int]]("echoListEmpty", Nil, Nil)
    },
    test("nested case class roundtrips through binding") {
      val input = Outer("root", DeepNested("child", List(10, 20)))
      roundtrip[Outer, Outer]("echoNested", input, input)
    },
    test("multi-parameter method roundtrips through binding") {
      val b = binding("multiParam", broadDefn)
      ZIO.fromFuture { implicit ec =>
        for {
          payload <- liftEither(HostPayload.encode[(String, Int)](("hello", 42)))
          raw     <- FutureInterop.fromPromise(b.invoke(broadImpl, payload, testPrincipal))
          decoded <- liftEither(HostPayload.decode[String](raw))
        } yield decoded
      }.map(decoded => assertTrue(decoded == "hello-42"))
    },
    test("Future[Unit] return roundtrips through binding") {
      val b = binding("asyncVoid", broadDefn)
      ZIO.fromFuture { implicit ec =>
        for {
          payload <- liftEither(HostPayload.encode[String]("ignored"))
          raw     <- FutureInterop.fromPromise(b.invoke(broadImpl, payload, testPrincipal))
          decoded <- liftEither(HostPayload.decode[Unit](raw))
        } yield decoded
      }.as(assertCompletes)
    }
  )
}
