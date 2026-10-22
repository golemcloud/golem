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

package golem.runtime.macros

import golem.runtime.annotations.agentDefinition
import golem.runtime.OutputMetadata
import golem.schema.{FromSchema, IntoSchema, SchemaGraph}
import golem.schema.wire.{ConcreteCodec, SchemaWire}
import zio.blocks.schema.Schema
import zio.test.*

import scala.concurrent.{Await, Future}
import scala.concurrent.duration.*

private[macros] object WireMetadataParityTypes {
  sealed trait Animal
  sealed trait Pet                    extends Animal
  final case class Dog(name: String)  extends Pet
  case object Cat                     extends Pet
  final case class Wild(kind: String) extends Animal
  object Animal { implicit val schema: Schema[Animal] = Schema.derived }

  final case class Timed(at: java.time.Instant, took: java.time.Duration)
  object Timed { implicit val schema: Schema[Timed] = Schema.derived }

  type Wide = (
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    Int,
    String
  )
  implicit val wideSchema: Schema[Wide] = Schema.derived
}

/**
 * The compiler-emitted metadata of a parameter, the concrete codec the
 * generated code uses for it, and the schema derivation describe one schema.
 */
object WireMetadataParitySpec extends ZIOSpecDefault {
  import WireMetadataParityTypes.*

  @agentDefinition()
  trait ParityAgent {
    class Id()
    def set(value: Set[Int]): Future[Set[Int]]
    def animal(value: Animal): Future[Animal]
    def wide(value: Wide): Future[Wide]
    def timed(value: Timed): Future[Timed]
  }

  final class ParityAgentImpl() extends ParityAgent {
    override def set(value: Set[Int]): Future[Set[Int]] = Future.successful(value)
    override def animal(value: Animal): Future[Animal]  = Future.successful(value)
    override def wide(value: Wide): Future[Wide]        = Future.successful(value)
    override def timed(value: Timed): Future[Timed]     = Future.successful(value)
  }

  private val metadata = AgentMacros.agentMetadata[ParityAgent]

  private def input(name: String): SchemaGraph  = metadata.methods.find(_.name == name).get.input.userSupplied.head.graph
  private def output(name: String): SchemaGraph =
    metadata.methods.find(_.name == name).get.output.asInstanceOf[OutputMetadata.Single].graph

  private def invoke[A](method: String, codec: ConcreteCodec[A], value: A): A = {
    val implType = AgentImplementationMacro.wireImplementationTypeFromClass[ParityAgent, ParityAgentImpl]
    val in       = ConcreteCodec.record(Vector("value" -> codec.asInstanceOf[ConcreteCodec[Any]]))
    val invoked  = implType.methods
      .find(_.name == method)
      .get
      .invoke(new ParityAgentImpl, in.encodeValue(Vector(value)), golem.Principal.Anonymous)
    codec.decode(Await.result(invoked, 5.seconds).get)
  }

  private def agree[A](method: String, codec: ConcreteCodec[A], values: List[A])(implicit
    into: IntoSchema[A],
    from: FromSchema[A]
  ): Boolean = {
    val graph = SchemaWire.schemaGraphFromWit(codec.graph)
    input(method) == graph && output(method) == graph && into.graph == graph &&
    values.forall { value =>
      val encoded = codec.encodeValue(value)
      invoke(method, codec, value) == value &&
      SchemaWire.schemaValueFromWit(encoded) == into.toValue(value) &&
      from.fromValue(SchemaWire.schemaValueFromWit(encoded)) == Right(value)
    }
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("WireMetadataParitySpec")(
      test("Set parameters are lists on every path") {
        assertTrue(agree("set", ConcreteCodec.derived[Set[Int]], List(Set.empty[Int], Set(3, 7))))
      },
      test("nested sealed traits are nested variants on every path") {
        assertTrue(agree("animal", ConcreteCodec.derived[Animal], List(Dog("rex"), Cat, Wild("wolf"))))
      },
      test("tuples over 22 elements are flat tuples on every path") {
        val value: Wide = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, "last")
        assertTrue(agree("wide", ConcreteCodec.derived[Wide], List(value)))
      },
      test("nested Instant and Duration fields agree on every path") {
        val value = Timed(java.time.Instant.ofEpochSecond(12, 34), java.time.Duration.ofNanos(-56))
        assertTrue(agree("timed", ConcreteCodec.derived[Timed], List(value)))
      }
    )
}
