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

package example.streamingrpc

import golem.BaseAgent
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.schema.{AgentStream, FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, SchemaGraph, SchemaValue, t}
import golem.schema.SchemaValue.RecordValue

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

private object StreamingRecordSchema {
  def into2[A: IntoSchema, B: IntoSchema, C](
    firstName: String,
    secondName: String
  )(split: C => (A, B)): IntoSchema[C] =
    new IntoSchema[C] {
      private val first  = IntoSchema[A]
      private val second = IntoSchema[B]

      override lazy val graph: SchemaGraph =
        SchemaGraph(
          SchemaBuilder.mergeGraphDefs(List(first.graph, second.graph)),
          t.record(List(t.field(firstName, first.graph.root), t.field(secondName, second.graph.root)))
        )

      override def toValue(value: C): SchemaValue = {
        val (a, b) = split(value)
        RecordValue(List(first.toValue(a), second.toValue(b)))
      }
    }

  def from2[A: FromSchema, B: FromSchema, C](build: (A, B) => C): FromSchema[C] =
    new FromSchema[C] {
      private val first  = FromSchema[A]
      private val second = FromSchema[B]

      override def fromValue(value: SchemaValue): Either[FromSchemaError, C] =
        value match {
          case RecordValue(List(a, b)) =>
            for {
              decodedA <- first.fromValue(a)
              decodedB <- second.fromValue(b)
            } yield build(decodedA, decodedB)
          case other => Left(FromSchemaError(s"expected a two-field streaming record, got $other"))
        }
    }
}

final case class NestedStreams(labels: AgentStream[String], values: AgentStream[Int])
object NestedStreams {
  implicit val intoSchema: IntoSchema[NestedStreams] =
    StreamingRecordSchema.into2("labels", "values")(value => (value.labels, value.values))
  implicit val fromSchema: FromSchema[NestedStreams] =
    StreamingRecordSchema.from2(NestedStreams.apply)
}

final case class NestedStreamItem(label: String, values: AgentStream[Int])
object NestedStreamItem {
  implicit val intoSchema: IntoSchema[NestedStreamItem] =
    StreamingRecordSchema.into2("label", "values")(value => (value.label, value.values))
  implicit val fromSchema: FromSchema[NestedStreamItem] =
    StreamingRecordSchema.from2(NestedStreamItem.apply)
}

final case class MixedStreamOutput(label: String, values: AgentStream[Int])
object MixedStreamOutput {
  implicit val intoSchema: IntoSchema[MixedStreamOutput] =
    StreamingRecordSchema.into2("label", "values")(value => (value.label, value.values))
  implicit val fromSchema: FromSchema[MixedStreamOutput] =
    StreamingRecordSchema.from2(MixedStreamOutput.apply)
}

final case class SiblingStreams(strings: AgentStream[String], numbers: AgentStream[Int])
object SiblingStreams {
  implicit val intoSchema: IntoSchema[SiblingStreams] =
    StreamingRecordSchema.into2("strings", "numbers")(value => (value.strings, value.numbers))
  implicit val fromSchema: FromSchema[SiblingStreams] =
    StreamingRecordSchema.from2(SiblingStreams.apply)
}

@agentDefinition()
trait ScalaStreamingTarget extends BaseAgent {
  class Id(val name: String)

  def consume(input: golem.schema.AgentStream[Int]): Future[List[Int]]
  def produce(values: List[Int]): Future[golem.schema.AgentStream[Int]]
  def transform(label: String, input: golem.schema.AgentStream[Int]): Future[MixedStreamOutput]
  def consumeNested(input: NestedStreams): Future[String]
  def produceNested(): Future[golem.schema.AgentStream[NestedStreamItem]]
  def produceSiblings(): Future[SiblingStreams]
  def ping(): Future[Int]
}

@agentDefinition()
trait ScalaStreamingCaller extends BaseAgent {
  class Id(val name: String)

  def run(): Future[String]
}

private object StreamingFixture {
  def streamOf[A](values: List[A]): AgentStream[A] = {
    val remaining = values.iterator
    AgentStream.fromPull(() => Future.successful(if (remaining.hasNext) Some(remaining.next()) else None))
  }

  def collect[A](stream: AgentStream[A]): Future[List[A]] = {
    val result = List.newBuilder[A]

    def loop(): Future[List[A]] =
      stream.pull().flatMap {
        case Some(value) =>
          result += value
          loop()
        case None => Future.successful(result.result())
      }

    loop()
  }

  def nestedItems(): AgentStream[NestedStreamItem] = {
    val remaining = List("first" -> List(1, 2), "second" -> List(3, 4, 5)).iterator
    AgentStream.fromPull(() =>
      Future.successful(
        if (remaining.hasNext) {
          val (label, values) = remaining.next()
          Some(NestedStreamItem(label, streamOf(values)))
        } else None
      )
    )
  }

  def collectNested(stream: AgentStream[NestedStreamItem]): Future[List[String]] = {
    val result = List.newBuilder[String]

    def loop(): Future[List[String]] =
      stream.pull().flatMap {
        case Some(item) =>
          collect(item.values).flatMap { values =>
            result += s"${item.label}:${values.mkString(",")}"
            loop()
          }
        case None => Future.successful(result.result())
      }

    loop()
  }
}

@agentImplementation()
final class ScalaStreamingTargetImpl(@unused private val name: String) extends ScalaStreamingTarget {
  import StreamingFixture._

  override def consume(input: AgentStream[Int]): Future[List[Int]] =
    collect(input)

  override def produce(values: List[Int]): Future[AgentStream[Int]] =
    Future.successful(streamOf(values))

  override def transform(label: String, input: AgentStream[Int]): Future[MixedStreamOutput] =
    Future.successful(MixedStreamOutput(label, input.map(_ * 10)))

  override def consumeNested(input: NestedStreams): Future[String] =
    for {
      labels <- collect(input.labels)
      values <- collect(input.values)
    } yield s"${labels.mkString(",")}|${values.mkString(",")}"

  override def produceNested(): Future[AgentStream[NestedStreamItem]] =
    Future.successful(nestedItems())

  override def produceSiblings(): Future[SiblingStreams] =
    Future.successful(SiblingStreams(streamOf(List("a", "b")), streamOf(List(20, 21, 22))))

  override def ping(): Future[Int] =
    Future.successful(42)
}

@agentImplementation()
final class ScalaStreamingCallerImpl(private val name: String) extends ScalaStreamingCaller {
  import StreamingFixture._

  override def run(): Future[String] = {
    val target = ScalaStreamingTargetClient.get(name)

    for {
      inputOnly    <- target.consume(streamOf(List(1, 2, 3)))
      outputStream <- target.produce(List(4, 5, 6))
      outputOnly   <- collect(outputStream)
      mixed        <- target.transform("mapped", streamOf(List(7, 8, 9)))
      mixedValues  <- collect(mixed.values)
      nestedInput  <- target.consumeNested(
                       NestedStreams(streamOf(List("left", "right")), streamOf(List(10, 11)))
                     )
      nestedStream   <- target.produceNested()
      nestedOutput   <- collectNested(nestedStream)
      siblings       <- target.produceSiblings()
      siblingStrings <- collect(siblings.strings)
      siblingNumbers <- collect(siblings.numbers)
      ping           <- target.ping()
    } yield List(
      s"input=${inputOnly.mkString(",")}",
      s"output=${outputOnly.mkString(",")}",
      s"mixed=${mixed.label}:${mixedValues.mkString(",")}",
      s"nested-input=$nestedInput",
      s"nested-output=${nestedOutput.mkString("|")}",
      s"siblings=${siblingStrings.mkString(",")}|${siblingNumbers.mkString(",")}",
      s"ping=$ping"
    ).mkString(";")
  }
}
