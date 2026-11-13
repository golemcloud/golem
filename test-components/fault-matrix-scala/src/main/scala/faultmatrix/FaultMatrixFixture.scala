package faultmatrix

// The Scala fixture of the streaming fault matrix. It implements the fixture contract documented
// in integration-tests/src/fault_matrix.rs. Record `seq` of run `run` is the number
// `run * 100_000 + seq`, for `seq` in 1..=count.

import golem.{BaseAgent, HostApi, UInt, ULong}
import golem.host.js.{JsAgentId, JsComponentId, JsPromiseId, JsUuid}
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.schema.AgentStream
import zio.blocks.schema.Schema

import java.util.UUID
import scala.concurrent.{ExecutionContext, Future}
import scala.scalajs.js

/** The schema shape of `golem:core/types@2.0.0` component-id. */
final case class GateComponentId(uuid: UUID)
object GateComponentId {
  implicit val schema: Schema[GateComponentId] = Schema.derived
}

/** The schema shape of `golem:core/types@2.0.0` agent-id. */
final case class GateAgentId(componentId: GateComponentId, agentId: String)
object GateAgentId {
  implicit val schema: Schema[GateAgentId] = Schema.derived
}

/** The schema shape of `golem:core/types@2.0.0` promise-id. */
final case class Gate(agentId: GateAgentId, oplogIdx: ULong) {
  def toHost: HostApi.PromiseId =
    JsPromiseId(
      JsAgentId(JsComponentId(JsUuid.fromUuid(agentId.componentId.uuid)), agentId.agentId),
      js.BigInt(oplogIdx.value.toString)
    )
}
object Gate {
  implicit val schema: Schema[Gate] = Schema.derived

  def fromHost(id: HostApi.PromiseId): Gate =
    Gate(
      GateAgentId(GateComponentId(JsUuid.toUuid(id.agentId.componentId.uuid)), id.agentId.agentId),
      ULong(BigInt(id.oplogIdx.toString))
    )
}

@agentDefinition()
trait FaultMatrixFixture extends BaseAgent {
  class Id(val name: String)

  def create_gate(): Future[Gate]
  def produce(run: UInt, count: UInt, pause_after: UInt, gate: Gate): Future[AgentStream[UInt]]
  def produce_then_fail(run: UInt, fail_after: UInt): Future[AgentStream[UInt]]
  def consume(input: AgentStream[UInt]): Future[Vector[UInt]]
  def consume_then_drop(input: AgentStream[UInt], keep: UInt): Future[Vector[UInt]]
  def pipe(target: String, run: UInt, count: UInt, keep: UInt): Future[Vector[UInt]]
  def read_remote(target: String, run: UInt, count: UInt, pause_after: UInt, gate: Gate): Future[Vector[UInt]]
}

@agentImplementation()
final class FaultMatrixFixtureImpl(name: String) extends FaultMatrixFixture {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic

  private def record(run: UInt, seq: Long): UInt = UInt(run.value * 100000L + seq)

  /** A stream of records 1..=count that runs `before(seq)` before it emits record `seq`. */
  private def records(run: UInt, count: Long)(before: Long => Future[Unit]): AgentStream[UInt] = {
    var seq = 0L
    AgentStream.fromPull { () =>
      if (seq >= count) Future.successful(None)
      else {
        seq += 1
        val current = seq
        before(current).map(_ => Some(record(run, current)))
      }
    }
  }

  private def collect(input: AgentStream[UInt], keep: Long): Future[Vector[UInt]] = {
    def loop(values: Vector[UInt]): Future[Vector[UInt]] =
      if (values.length >= keep) input.close().map(_ => values)
      else
        input.pull().flatMap {
          case Some(value) => loop(values :+ value)
          case None        => Future.successful(values)
        }
    loop(Vector.empty)
  }

  override def create_gate(): Future[Gate] =
    Future.successful(Gate.fromHost(HostApi.createPromise()))

  override def produce(run: UInt, count: UInt, pause_after: UInt, gate: Gate): Future[AgentStream[UInt]] =
    Future.successful(records(run, count.value) { seq =>
      if (seq == pause_after.value + 1) HostApi.awaitPromise(gate.toHost).map(_ => ())
      else Future.unit
    })

  override def produce_then_fail(run: UInt, fail_after: UInt): Future[AgentStream[UInt]] = {
    var seq = 0L
    Future.successful(AgentStream.fromPull { () =>
      if (seq >= fail_after.value) Future.failed(new RuntimeException("the fixture producer failed"))
      else {
        seq += 1
        Future.successful(Some(record(run, seq)))
      }
    })
  }

  override def consume(input: AgentStream[UInt]): Future[Vector[UInt]] = collect(input, Long.MaxValue)

  override def consume_then_drop(input: AgentStream[UInt], keep: UInt): Future[Vector[UInt]] =
    collect(input, keep.value)

  override def pipe(target: String, run: UInt, count: UInt, keep: UInt): Future[Vector[UInt]] = {
    val client = FaultMatrixFixtureClient.get(target)
    val input  = records(run, count.value)(_ => Future.unit)
    if (keep.value >= count.value) client.consume(input) else client.consume_then_drop(input, keep)
  }

  override def read_remote(
    target: String,
    run: UInt,
    count: UInt,
    pause_after: UInt,
    gate: Gate
  ): Future[Vector[UInt]] =
    FaultMatrixFixtureClient
      .get(target)
      .produce(run, count, pause_after, gate)
      .flatMap(collect(_, Long.MaxValue))
}
