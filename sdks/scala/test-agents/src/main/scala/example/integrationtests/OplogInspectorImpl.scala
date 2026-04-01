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

package example.integrationtests

import golem.HostApi
import golem.host.{OplogApi, WitValueTypes}
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class OplogInspectorImpl(@unused private val name: String) extends OplogInspector {

  override def inspectRecent(): Future[String] = Future.successful {
    val sb       = new StringBuilder
    val meta     = HostApi.getSelfMetadata()
    val startIdx = {
      val cur = HostApi.getOplogIndex()
      if (cur > BigInt(20)) cur - BigInt(20) else BigInt(0)
    }

    sb.append(s"=== Oplog Inspector (agent=${meta.agentName}) ===\n")
    sb.append(s"Reading from index $startIdx\n\n")

    val oplog = OplogApi.GetOplog(meta.agentId, startIdx)
    oplog.getNext() match {
      case None =>
        sb.append("No entries found.\n")
      case Some(entries) =>
        entries.zipWithIndex.foreach { case (entry, i) =>
          sb.append(s"[$i] ${describeEntry(entry)}\n")
        }
    }
    sb.toString()
  }

  override def searchOplog(text: String): Future[String] = Future.successful {
    val sb   = new StringBuilder
    val meta = HostApi.getSelfMetadata()
    sb.append(s"=== Searching oplog for '$text' ===\n")

    val search = OplogApi.SearchOplog(meta.agentId, text)
    search.getNext() match {
      case None =>
        sb.append("No matching entries.\n")
      case Some(results) =>
        results.foreach { case (idx, entry) =>
          sb.append(s"  [oplog#$idx] ${describeEntry(entry)}\n")
        }
    }
    sb.toString()
  }

  private def describeEntry(e: OplogApi.OplogEntry): String = {
    val ts = s"${e.timestamp.seconds}s"
    e match {
      case OplogApi.OplogEntry.Create(p) =>
        s"CREATE @ $ts revision=${p.componentRevision} args=${p.args.mkString(",")}"

      case OplogApi.OplogEntry.HostCall(p) =>
        val reqSummary = summarizeVat(p.request)
        val resSummary = summarizeVat(p.response)
        s"IMPORT @ $ts func=${p.functionName} req=$reqSummary res=$resSummary type=${p.wrappedFunctionType.tag}"

      case OplogApi.OplogEntry.AgentInvocationStarted(p) =>
        val reqCount = p.request.size
        s"EXPORT @ $ts func=${p.functionName} params=$reqCount idem=${p.idempotencyKey}"

      case OplogApi.OplogEntry.AgentInvocationFinished(p) =>
        val resp = p.response.map(summarizeTdv).getOrElse("void")
        s"COMPLETED @ $ts response=$resp fuel=${p.consumedFuel}"

      case OplogApi.OplogEntry.Suspend(t)                => s"SUSPEND @ ${t.seconds}s"
      case OplogApi.OplogEntry.Error(p)                  => s"ERROR @ $ts '${p.error}' retryFrom=${p.retryFrom}"
      case OplogApi.OplogEntry.NoOp(t)                   => s"NOOP @ ${t.seconds}s"
      case OplogApi.OplogEntry.Jump(p)                   => s"JUMP @ $ts range=[${p.jump.start},${p.jump.end}]"
      case OplogApi.OplogEntry.Interrupted(t)            => s"INTERRUPTED @ ${t.seconds}s"
      case OplogApi.OplogEntry.Exited(t)                 => s"EXITED @ ${t.seconds}s"
      case OplogApi.OplogEntry.ChangeRetryPolicy(p)      => s"RETRY_POLICY @ $ts max=${p.newPolicy.maxAttempts}"
      case OplogApi.OplogEntry.BeginAtomicRegion(t)      => s"BEGIN_ATOMIC @ ${t.seconds}s"
      case OplogApi.OplogEntry.EndAtomicRegion(p)        => s"END_ATOMIC @ $ts begin=${p.beginIndex}"
      case OplogApi.OplogEntry.BeginRemoteWrite(t)       => s"BEGIN_REMOTE_WRITE @ ${t.seconds}s"
      case OplogApi.OplogEntry.EndRemoteWrite(p)         => s"END_REMOTE_WRITE @ $ts begin=${p.beginIndex}"
      case OplogApi.OplogEntry.PendingAgentInvocation(p) =>
        val invDesc = p.invocation match {
          case OplogApi.AgentInvocation.ExportedFunction(params) => s"func(${params.functionName})"
          case OplogApi.AgentInvocation.AgentInitialization(key) => s"agent-init(key=$key)"
          case OplogApi.AgentInvocation.SaveSnapshot             => "save-snapshot"
          case OplogApi.AgentInvocation.LoadSnapshot             => "load-snapshot"
          case OplogApi.AgentInvocation.ProcessOplogEntries(key) => s"process-oplog(key=$key)"
          case OplogApi.AgentInvocation.ManualUpdate(rev)        => s"manual-update(rev=$rev)"
        }
        s"PENDING_INVOCATION @ $ts $invDesc"
      case OplogApi.OplogEntry.PendingUpdate(p) =>
        val desc = p.updateDescription match {
          case OplogApi.UpdateDescription.AutoUpdate       => "auto"
          case OplogApi.UpdateDescription.SnapshotBased(d) => s"snapshot(${d.length}B)"
        }
        s"PENDING_UPDATE @ $ts rev=${p.targetRevision} $desc"
      case OplogApi.OplogEntry.SuccessfulUpdate(p) =>
        s"SUCCESS_UPDATE @ $ts rev=${p.targetRevision} size=${p.newComponentSize} plugins=${p.newActivePlugins.size}"
      case OplogApi.OplogEntry.FailedUpdate(p) =>
        s"FAILED_UPDATE @ $ts rev=${p.targetRevision} detail=${p.details.getOrElse("none")}"
      case OplogApi.OplogEntry.GrowMemory(p)     => s"GROW_MEMORY @ $ts delta=${p.delta}"
      case OplogApi.OplogEntry.CreateResource(p) => s"CREATE_RES @ $ts id=${p.resourceId} ${p.owner}.${p.name}"
      case OplogApi.OplogEntry.DropResource(p)   => s"DROP_RES @ $ts id=${p.resourceId} ${p.owner}.${p.name}"
      case OplogApi.OplogEntry.Log(p)            =>
        val level = p.level match {
          case OplogApi.LogLevel.Stdout   => "STDOUT"
          case OplogApi.LogLevel.Stderr   => "STDERR"
          case OplogApi.LogLevel.Trace    => "TRACE"
          case OplogApi.LogLevel.Debug    => "DEBUG"
          case OplogApi.LogLevel.Info     => "INFO"
          case OplogApi.LogLevel.Warn     => "WARN"
          case OplogApi.LogLevel.Error    => "ERROR"
          case OplogApi.LogLevel.Critical => "CRITICAL"
        }
        s"LOG @ $ts [$level] ${p.context}: ${p.message}"
      case OplogApi.OplogEntry.Restart(t)                 => s"RESTART @ ${t.seconds}s"
      case OplogApi.OplogEntry.ActivatePlugin(p)          => s"ACTIVATE_PLUGIN @ $ts ${p.plugin.name}@${p.plugin.version}"
      case OplogApi.OplogEntry.DeactivatePlugin(p)        => s"DEACTIVATE_PLUGIN @ $ts ${p.plugin.name}@${p.plugin.version}"
      case OplogApi.OplogEntry.Revert(p)                  => s"REVERT @ $ts range=[${p.start},${p.end}]"
      case OplogApi.OplogEntry.CancelPendingInvocation(p) => s"CANCEL @ $ts idem=${p.idempotencyKey}"
      case OplogApi.OplogEntry.StartSpan(p)               =>
        s"START_SPAN @ $ts id=${p.spanId} parent=${p.parent.getOrElse("none")} attrs=${p.attributes.size}"
      case OplogApi.OplogEntry.FinishSpan(p)             => s"FINISH_SPAN @ $ts id=${p.spanId}"
      case OplogApi.OplogEntry.SetSpanAttribute(p)       => s"SET_SPAN_ATTR @ $ts span=${p.spanId} key=${p.key}"
      case OplogApi.OplogEntry.ChangePersistenceLevel(p) =>
        s"CHANGE_PL @ $ts level=${p.persistenceLevel.tag}"
      case OplogApi.OplogEntry.BeginRemoteTransaction(p)       => s"BEGIN_TX @ $ts id=${p.transactionId}"
      case OplogApi.OplogEntry.PreCommitRemoteTransaction(p)   => s"PRE_COMMIT_TX @ $ts begin=${p.beginIndex}"
      case OplogApi.OplogEntry.PreRollbackRemoteTransaction(p) => s"PRE_ROLLBACK_TX @ $ts begin=${p.beginIndex}"
      case OplogApi.OplogEntry.CommittedRemoteTransaction(p)   => s"COMMITTED_TX @ $ts begin=${p.beginIndex}"
      case OplogApi.OplogEntry.RolledBackRemoteTransaction(p)  => s"ROLLED_BACK_TX @ $ts begin=${p.beginIndex}"
      case OplogApi.OplogEntry.Snapshot(t, data, mime)         => s"SNAPSHOT @ ${t.seconds}s ${data.length}B mime=$mime"
      case OplogApi.OplogEntry.OplogProcessorCheckpoint(p)     =>
        s"OPLOG_CHECKPOINT @ $ts plugin=${p.plugin.name} confirmed=${p.confirmedUpTo}"
    }
  }

  private def summarizeVat(vat: WitValueTypes.ValueAndType): String = {
    val nodeCount = vat.value.nodes.size
    val typeCount = vat.typ.nodes.size
    val firstNode = vat.value.nodes.headOption.map(describeNode).getOrElse("empty")
    s"VAT($nodeCount nodes, $typeCount types, first=$firstNode)"
  }

  private def summarizeTdv(tdv: OplogApi.TypedDataValue): String =
    s"TDV(value=${tdv.value.take(50)}, schema=${tdv.schema.take(50)})"

  private def describeNode(n: WitValueTypes.WitNode): String = n match {
    case WitValueTypes.WitNode.RecordValue(f)     => s"record(${f.size})"
    case WitValueTypes.WitNode.VariantValue(c, _) => s"variant($c)"
    case WitValueTypes.WitNode.EnumValue(c)       => s"enum($c)"
    case WitValueTypes.WitNode.FlagsValue(f)      => s"flags(${f.size})"
    case WitValueTypes.WitNode.TupleValue(e)      => s"tuple(${e.size})"
    case WitValueTypes.WitNode.ListValue(e)       => s"list(${e.size})"
    case WitValueTypes.WitNode.OptionValue(v)     => s"option(${v.isDefined})"
    case WitValueTypes.WitNode.ResultValue(o, e)  => s"result(ok=${o.isDefined},err=${e.isDefined})"
    case WitValueTypes.WitNode.PrimU8(v)          => s"u8($v)"
    case WitValueTypes.WitNode.PrimU16(v)         => s"u16($v)"
    case WitValueTypes.WitNode.PrimU32(v)         => s"u32($v)"
    case WitValueTypes.WitNode.PrimU64(v)         => s"u64($v)"
    case WitValueTypes.WitNode.PrimS8(v)          => s"s8($v)"
    case WitValueTypes.WitNode.PrimS16(v)         => s"s16($v)"
    case WitValueTypes.WitNode.PrimS32(v)         => s"s32($v)"
    case WitValueTypes.WitNode.PrimS64(v)         => s"s64($v)"
    case WitValueTypes.WitNode.PrimFloat32(v)     => s"f32($v)"
    case WitValueTypes.WitNode.PrimFloat64(v)     => s"f64($v)"
    case WitValueTypes.WitNode.PrimChar(v)        => s"char($v)"
    case WitValueTypes.WitNode.PrimBool(v)        => s"bool($v)"
    case WitValueTypes.WitNode.PrimString(v)      => s"string($v)"
    case WitValueTypes.WitNode.Handle(u, r)       => s"handle($u,$r)"
  }
}
