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

package golem.host

import golem.HostApi
import golem.runtime.rpc.host.AgentHostApi
import zio.test._

import scala.scalajs.js

object OplogApiCompileSpec extends ZIOSpecDefault {
  import OplogApi._

  private val ts        = ContextApi.DateTime(BigInt(1700000000L), 500000000L)
  private val attr      = ContextApi.Attribute("k", ContextApi.AttributeValue.StringValue("v"))
  private val sampleVat = WitValueTypes.ValueAndType(
    WitValueTypes.WitValue(List(WitValueTypes.WitNode.PrimString("test"))),
    WitValueTypes.WitType(List(WitValueTypes.NamedWitTypeNode(None, None, WitValueTypes.WitTypeNode.PrimStringType)))
  )
  private val sampleTdv = TypedDataValue("""{"tag":"tuple","val":[]}""", """{"tag":"tuple","val":[]}""")

  private val pluginDesc  = PluginInstallationDescription("plug", "1.0", Map("key" -> "val"))
  private val oplogRegion = OplogRegion(BigInt(0), BigInt(10))

  private val localSpan                 = LocalSpanData("span1", ts, Some("parent"), Some(BigInt(1)), List(attr), inherited = false)
  private val externalSpan              = ExternalSpanData("span2")
  private val spanDatas: List[SpanData] = List(SpanData.LocalSpan(localSpan), SpanData.ExternalSpan(externalSpan))

  private val agentInvocations: List[AgentInvocation] = List(
    AgentInvocation.ExportedFunction(
      AgentMethodInvocationParameters(
        "idem-1",
        "func",
        Some(List(sampleTdv)),
        "trace1",
        List("state1"),
        List(spanDatas)
      )
    ),
    AgentInvocation.ManualUpdate(BigInt(2))
  )

  private val updateDescs: List[UpdateDescription] = List(
    UpdateDescription.AutoUpdate,
    UpdateDescription.SnapshotBased(Array[Byte](1, 2, 3))
  )

  private val logLevels: List[LogLevel] = List(
    LogLevel.Stdout,
    LogLevel.Stderr,
    LogLevel.Trace,
    LogLevel.Debug,
    LogLevel.Info,
    LogLevel.Warn,
    LogLevel.Error,
    LogLevel.Critical
  )

  private def describeLogLevel(l: LogLevel): String = l match {
    case LogLevel.Stdout   => "stdout"
    case LogLevel.Stderr   => "stderr"
    case LogLevel.Trace    => "trace"
    case LogLevel.Debug    => "debug"
    case LogLevel.Info     => "info"
    case LogLevel.Warn     => "warn"
    case LogLevel.Error    => "error"
    case LogLevel.Critical => "critical"
  }

  @SuppressWarnings(Array("all"))
  private def describeEntry(e: OplogEntry): String = e match {
    case OplogEntry.Create(p)                       => s"create(${p.componentRevision})"
    case OplogEntry.HostCall(p)                     => s"import(${p.functionName})"
    case OplogEntry.AgentInvocationStarted(p)       => s"export(${p.functionName})"
    case OplogEntry.AgentInvocationFinished(p)      => s"completed(${p.consumedFuel})"
    case OplogEntry.Suspend(t)                      => s"suspend(${t.seconds})"
    case OplogEntry.Error(p)                        => s"error(${p.error})"
    case OplogEntry.NoOp(t)                         => s"noop(${t.seconds})"
    case OplogEntry.Jump(p)                         => s"jump(${p.jump.start})"
    case OplogEntry.Interrupted(t)                  => s"interrupted(${t.seconds})"
    case OplogEntry.Exited(t)                       => s"exited(${t.seconds})"
    case OplogEntry.SetRetryPolicy(p)               => s"set-retry(${p.name})"
    case OplogEntry.RemoveRetryPolicy(p)            => s"remove-retry(${p.name})"
    case OplogEntry.BeginAtomicRegion(t)            => s"begin-atomic(${t.seconds})"
    case OplogEntry.EndAtomicRegion(p)              => s"end-atomic(${p.beginIndex})"
    case OplogEntry.BeginRemoteWrite(t)             => s"begin-rw(${t.seconds})"
    case OplogEntry.EndRemoteWrite(p)               => s"end-rw(${p.beginIndex})"
    case OplogEntry.PendingAgentInvocation(p)       => s"pending-inv(${p.invocation})"
    case OplogEntry.PendingUpdate(p)                => s"pending-upd(${p.targetRevision})"
    case OplogEntry.SuccessfulUpdate(p)             => s"success-upd(${p.targetRevision})"
    case OplogEntry.FailedUpdate(p)                 => s"failed-upd(${p.details})"
    case OplogEntry.GrowMemory(p)                   => s"grow(${p.delta})"
    case OplogEntry.CreateResource(p)               => s"create-res(${p.name})"
    case OplogEntry.DropResource(p)                 => s"drop-res(${p.name})"
    case OplogEntry.Log(p)                          => s"log(${p.level},${p.message})"
    case OplogEntry.Restart(t)                      => s"restart(${t.seconds})"
    case OplogEntry.ActivatePlugin(p)               => s"activate(${p.plugin.name})"
    case OplogEntry.DeactivatePlugin(p)             => s"deactivate(${p.plugin.name})"
    case OplogEntry.Revert(p)                       => s"revert(${p.start})"
    case OplogEntry.CancelPendingInvocation(p)      => s"cancel(${p.idempotencyKey})"
    case OplogEntry.StartSpan(p)                    => s"start-span(${p.spanId})"
    case OplogEntry.FinishSpan(p)                   => s"finish-span(${p.spanId})"
    case OplogEntry.SetSpanAttribute(p)             => s"set-attr(${p.key})"
    case OplogEntry.ChangePersistenceLevel(p)       => s"change-pl(${p.persistenceLevel})"
    case OplogEntry.BeginRemoteTransaction(p)       => s"begin-tx(${p.transactionId})"
    case OplogEntry.PreCommitRemoteTransaction(p)   => s"pre-commit(${p.beginIndex})"
    case OplogEntry.PreRollbackRemoteTransaction(p) => s"pre-rollback(${p.beginIndex})"
    case OplogEntry.CommittedRemoteTransaction(p)   => s"committed(${p.beginIndex})"
    case OplogEntry.RolledBackRemoteTransaction(p)  => s"rolled-back(${p.beginIndex})"
    case OplogEntry.Snapshot(ts, _, mime)           => s"snapshot($ts,$mime)"
    case OplogEntry.OplogProcessorCheckpoint(p)     => s"checkpoint(${p.confirmedUpTo})"
  }

  private val mockUuid    = AgentHostApi.UuidLiteral(js.BigInt("0"), js.BigInt("0"))
  private val mockCompId  = AgentHostApi.ComponentIdLiteral(mockUuid)
  private val mockAgentId = AgentHostApi.AgentIdLiteral(mockCompId, "test-agent")

  private val allEntries: List[OplogEntry] = {
    List(
      OplogEntry.Create(
        CreateParameters(
          ts,
          mockAgentId,
          BigInt(1),
          List("arg1"),
          Map("env" -> "val"),
          "golem",
          "test-env",
          None,
          BigInt(1024),
          BigInt(65536),
          List(pluginDesc),
          Map("cfg" -> "v")
        )
      ),
      OplogEntry.Suspend(ts),
      OplogEntry.NoOp(ts),
      OplogEntry.Interrupted(ts),
      OplogEntry.Exited(ts),
      OplogEntry.BeginAtomicRegion(ts),
      OplogEntry.BeginRemoteWrite(ts),
      OplogEntry.Restart(ts),
      OplogEntry.Error(ErrorParameters(ts, "boom", BigInt(5))),
      OplogEntry.Jump(JumpParameters(ts, oplogRegion)),
      OplogEntry.SetRetryPolicy(
        SetRetryPolicyParameters(ts, "default", 0, """{"nodes":[]}""", """{"nodes":[]}""")
      ),
      OplogEntry.RemoveRetryPolicy(RemoveRetryPolicyParameters(ts, "default")),
      OplogEntry.EndAtomicRegion(EndAtomicRegionParameters(ts, BigInt(1))),
      OplogEntry.EndRemoteWrite(EndRemoteWriteParameters(ts, BigInt(2))),
      OplogEntry.GrowMemory(GrowMemoryParameters(ts, BigInt(65536))),
      OplogEntry.CancelPendingInvocation(CancelPendingInvocationParameters(ts, "idem-key")),
      OplogEntry.FinishSpan(FinishSpanParameters(ts, "span-1")),
      OplogEntry.ChangePersistenceLevel(ChangePersistenceLevelParameters(ts, HostApi.PersistenceLevel.Smart)),
      OplogEntry.BeginRemoteTransaction(BeginRemoteTransactionParameters(ts, "tx-1")),
      OplogEntry.PreCommitRemoteTransaction(RemoteTransactionParameters(ts, BigInt(10))),
      OplogEntry.PreRollbackRemoteTransaction(RemoteTransactionParameters(ts, BigInt(11))),
      OplogEntry.CommittedRemoteTransaction(RemoteTransactionParameters(ts, BigInt(12))),
      OplogEntry.RolledBackRemoteTransaction(RemoteTransactionParameters(ts, BigInt(13))),
      OplogEntry.AgentInvocationFinished(AgentInvocationFinishedParameters(ts, Some(sampleTdv), 1000L)),
      OplogEntry.AgentInvocationFinished(AgentInvocationFinishedParameters(ts, None, 0L)),
      OplogEntry.HostCall(
        HostCallParameters(
          ts,
          "wasi:io/read",
          sampleVat,
          sampleVat,
          DurabilityApi.DurableFunctionType.ReadRemote
        )
      ),
      OplogEntry.AgentInvocationStarted(
        AgentInvocationStartedParameters(
          ts,
          "increment",
          List(sampleTdv),
          "idem-1",
          "trace-1",
          List("state"),
          List(spanDatas)
        )
      ),
      OplogEntry.PendingAgentInvocation(PendingAgentInvocationParameters(ts, agentInvocations.head)),
      OplogEntry.PendingUpdate(PendingUpdateParameters(ts, BigInt(3), UpdateDescription.AutoUpdate)),
      OplogEntry.SuccessfulUpdate(SuccessfulUpdateParameters(ts, BigInt(3), BigInt(1024), List(pluginDesc))),
      OplogEntry.FailedUpdate(FailedUpdateParameters(ts, BigInt(3), Some("compile error"))),
      OplogEntry.FailedUpdate(FailedUpdateParameters(ts, BigInt(3), None)),
      OplogEntry.CreateResource(CreateResourceParameters(ts, BigInt(1), "handle", "golem:api")),
      OplogEntry.DropResource(DropResourceParameters(ts, BigInt(1), "handle", "golem:api")),
      OplogEntry.Log(LogParameters(ts, LogLevel.Info, "main", "started")),
      OplogEntry.ActivatePlugin(ActivatePluginParameters(ts, pluginDesc)),
      OplogEntry.DeactivatePlugin(DeactivatePluginParameters(ts, pluginDesc)),
      OplogEntry.Revert(RevertParameters(ts, BigInt(0), BigInt(10))),
      OplogEntry.StartSpan(StartSpanParameters(ts, "span-1", Some("parent"), Some("linked"), List(attr))),
      OplogEntry.SetSpanAttribute(
        SetSpanAttributeParameters(ts, "span-1", "key", ContextApi.AttributeValue.StringValue("v"))
      )
    )
  }

  def spec = suite("OplogApiCompileSpec")(
    test("all 38 OplogEntry variants constructed") {
      val distinctTags = allEntries.map(describeEntry).map(_.takeWhile(_ != '(')).distinct
      assertTrue(distinctTags.size >= 38)
    },
    test("exhaustive OplogEntry match compiles") {
      allEntries.foreach(e => Predef.assert(describeEntry(e).nonEmpty))
      assertCompletes
    },
    test("every entry has timestamp") {
      allEntries.foreach(e => Predef.assert(e.timestamp.seconds >= BigInt(0)))
      assertCompletes
    },
    test("SpanData exhaustive match") {
      spanDatas.foreach {
        case SpanData.LocalSpan(d)    => Predef.assert(d.spanId.nonEmpty)
        case SpanData.ExternalSpan(d) => Predef.assert(d.spanId.nonEmpty)
      }
      assertCompletes
    },
    test("AgentInvocation exhaustive match") {
      agentInvocations.foreach {
        case AgentInvocation.ExportedFunction(p)    => Predef.assert(p.functionName.nonEmpty)
        case AgentInvocation.ManualUpdate(rev)      => Predef.assert(rev > 0)
        case AgentInvocation.AgentInitialization(k) => Predef.assert(k.nonEmpty)
        case AgentInvocation.SaveSnapshot           => ()
        case AgentInvocation.LoadSnapshot           => ()
        case AgentInvocation.ProcessOplogEntries(k) => Predef.assert(k.nonEmpty)
      }
      assertCompletes
    },
    test("UpdateDescription exhaustive match") {
      updateDescs.foreach {
        case UpdateDescription.AutoUpdate       => Predef.assert(true)
        case UpdateDescription.SnapshotBased(d) => Predef.assert(d.nonEmpty)
      }
      assertCompletes
    },
    test("LogLevel exhaustive match") {
      logLevels.foreach(l => Predef.assert(describeLogLevel(l).nonEmpty))
      assertTrue(logLevels.size == 8)
    },
    test("supporting record construction") {
      assertTrue(
        pluginDesc.name == "plug",
        oplogRegion.start == BigInt(0),
        localSpan.spanId == "span1",
        externalSpan.spanId == "span2"
      )
    },
    test("OplogIndex type alias") {
      val idx: OplogIndex = BigInt(99)
      assertTrue(idx == BigInt(99))
    }
  )
}
