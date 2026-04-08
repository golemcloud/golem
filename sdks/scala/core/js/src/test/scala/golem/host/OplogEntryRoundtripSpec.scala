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
import zio.test._

import scala.scalajs.js

object OplogEntryRoundtripSpec extends ZIOSpecDefault {
  import OplogApi._

  private def ts(seconds: Int = 1700000000, nanos: Int = 500000000): js.Dynamic =
    js.Dynamic.literal(
      seconds = js.BigInt(seconds.toString),
      nanoseconds = nanos
    )

  private def wrapEntry(tag: String, v: js.Dynamic): js.Dynamic =
    js.Dynamic.literal(tag = tag, `val` = v)

  private def simpleTimestampEntry(v: js.Dynamic): js.Dynamic =
    js.Dynamic.literal(timestamp = v)

  // --- Simple timestamp-only entries ---

  def spec = suite("OplogEntryRoundtripSpec")(
    test("Suspend from dynamic") {
      val raw    = wrapEntry("suspend", js.Dynamic.literal(timestamp = ts()))
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.Suspend],
        parsed.timestamp.seconds == BigInt(1700000000)
      )
    },
    test("NoOp from dynamic") {
      val raw    = wrapEntry("no-op", js.Dynamic.literal(timestamp = ts()))
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.NoOp])
    },
    test("Interrupted from dynamic") {
      val raw    = wrapEntry("interrupted", js.Dynamic.literal(timestamp = ts()))
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.Interrupted])
    },
    test("Exited from dynamic") {
      val raw    = wrapEntry("exited", js.Dynamic.literal(timestamp = ts()))
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.Exited])
    },
    test("BeginAtomicRegion from dynamic") {
      val raw    = wrapEntry("begin-atomic-region", js.Dynamic.literal(timestamp = ts()))
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.BeginAtomicRegion])
    },
    test("BeginRemoteWrite from dynamic") {
      val raw    = wrapEntry("begin-remote-write", js.Dynamic.literal(timestamp = ts()))
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.BeginRemoteWrite])
    },
    test("Restart from dynamic") {
      val raw    = wrapEntry("restart", js.Dynamic.literal(timestamp = ts()))
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.Restart])
    },
    // --- Single-field parameter entries ---

    test("Error from dynamic") {
      val raw = wrapEntry(
        "error",
        js.Dynamic.literal(
          timestamp = ts(),
          error = "something failed",
          retryFrom = js.BigInt("5")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val e      = parsed.asInstanceOf[OplogEntry.Error]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.Error],
        e.params.error == "something failed",
        e.params.retryFrom == BigInt(5)
      )
    },
    test("EndAtomicRegion from dynamic") {
      val raw = wrapEntry(
        "end-atomic-region",
        js.Dynamic.literal(
          timestamp = ts(),
          beginIndex = js.BigInt("10")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.EndAtomicRegion],
        parsed.asInstanceOf[OplogEntry.EndAtomicRegion].params.beginIndex == BigInt(10)
      )
    },
    test("EndRemoteWrite from dynamic") {
      val raw = wrapEntry(
        "end-remote-write",
        js.Dynamic.literal(
          timestamp = ts(),
          beginIndex = js.BigInt("20")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.EndRemoteWrite],
        parsed.asInstanceOf[OplogEntry.EndRemoteWrite].params.beginIndex == BigInt(20)
      )
    },
    test("GrowMemory from dynamic") {
      val raw = wrapEntry(
        "grow-memory",
        js.Dynamic.literal(
          timestamp = ts(),
          delta = js.BigInt("65536")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.GrowMemory],
        parsed.asInstanceOf[OplogEntry.GrowMemory].params.delta == BigInt(65536)
      )
    },
    test("CancelPendingInvocation from dynamic") {
      val raw = wrapEntry(
        "cancel-invocation",
        js.Dynamic.literal(
          timestamp = ts(),
          idempotencyKey = "idem-123"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.CancelPendingInvocation],
        parsed.asInstanceOf[OplogEntry.CancelPendingInvocation].params.idempotencyKey == "idem-123"
      )
    },
    test("FinishSpan from dynamic") {
      val raw = wrapEntry(
        "finish-span",
        js.Dynamic.literal(
          timestamp = ts(),
          spanId = "span-42"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.FinishSpan],
        parsed.asInstanceOf[OplogEntry.FinishSpan].params.spanId == "span-42"
      )
    },
    test("ChangePersistenceLevel from dynamic") {
      val raw = wrapEntry(
        "change-persistence-level",
        js.Dynamic.literal(
          timestamp = ts(),
          persistenceLevel = js.Dynamic.literal(tag = "smart")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.ChangePersistenceLevel],
        parsed
          .asInstanceOf[OplogEntry.ChangePersistenceLevel]
          .params
          .persistenceLevel == HostApi.PersistenceLevel.Smart
      )
    },
    test("BeginRemoteTransaction from dynamic") {
      val raw = wrapEntry(
        "begin-remote-transaction",
        js.Dynamic.literal(
          timestamp = ts(),
          transactionId = "tx-1"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.BeginRemoteTransaction],
        parsed.asInstanceOf[OplogEntry.BeginRemoteTransaction].params.transactionId == "tx-1"
      )
    },
    test("PreCommitRemoteTransaction from dynamic") {
      val raw = wrapEntry(
        "pre-commit-remote-transaction",
        js.Dynamic.literal(
          timestamp = ts(),
          beginIndex = js.BigInt("30")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.PreCommitRemoteTransaction],
        parsed.asInstanceOf[OplogEntry.PreCommitRemoteTransaction].params.beginIndex == BigInt(30)
      )
    },
    test("PreRollbackRemoteTransaction from dynamic") {
      val raw = wrapEntry(
        "pre-rollback-remote-transaction",
        js.Dynamic.literal(
          timestamp = ts(),
          beginIndex = js.BigInt("31")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.PreRollbackRemoteTransaction])
    },
    test("CommittedRemoteTransaction from dynamic") {
      val raw = wrapEntry(
        "committed-remote-transaction",
        js.Dynamic.literal(
          timestamp = ts(),
          beginIndex = js.BigInt("32")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.CommittedRemoteTransaction])
    },
    test("RolledBackRemoteTransaction from dynamic") {
      val raw = wrapEntry(
        "rolled-back-remote-transaction",
        js.Dynamic.literal(
          timestamp = ts(),
          beginIndex = js.BigInt("33")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.RolledBackRemoteTransaction])
    },
    // --- Complex entries ---

    test("Jump from dynamic") {
      val raw = wrapEntry(
        "jump",
        js.Dynamic.literal(
          timestamp = ts(),
          jump = js.Dynamic.literal(start = js.BigInt("0"), end = js.BigInt("10"))
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val j      = parsed.asInstanceOf[OplogEntry.Jump]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.Jump],
        j.params.jump.start == BigInt(0),
        j.params.jump.end == BigInt(10)
      )
    },
    test("SetRetryPolicy from dynamic") {
      val raw = wrapEntry(
        "set-retry-policy",
        js.Dynamic.literal(
          timestamp = ts(),
          name = "default",
          priority = 10,
          predicateJson = """{"nodes":[]}""",
          policyJson = """{"nodes":[]}"""
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val p      = parsed.asInstanceOf[OplogEntry.SetRetryPolicy].params
      assertTrue(
        parsed.isInstanceOf[OplogEntry.SetRetryPolicy],
        p.name == "default",
        p.priority == 10,
        p.predicateJson == """{"nodes":[]}""",
        p.policyJson == """{"nodes":[]}"""
      )
    },
    test("RemoveRetryPolicy from dynamic") {
      val raw = wrapEntry(
        "remove-retry-policy",
        js.Dynamic.literal(
          timestamp = ts(),
          name = "default"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val p      = parsed.asInstanceOf[OplogEntry.RemoveRetryPolicy].params
      assertTrue(
        parsed.isInstanceOf[OplogEntry.RemoveRetryPolicy],
        p.name == "default"
      )
    },
    test("FilesystemStorageUsageUpdate from dynamic") {
      val raw = wrapEntry(
        "filesystem-storage-usage-update",
        js.Dynamic.literal(
          timestamp = ts(),
          delta = js.BigInt("4096")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val p      = parsed.asInstanceOf[OplogEntry.FilesystemStorageUsageUpdate].params
      assertTrue(
        parsed.isInstanceOf[OplogEntry.FilesystemStorageUsageUpdate],
        p.delta == BigInt(4096)
      )
    },
    test("Log from dynamic") {
      val raw = wrapEntry(
        "log",
        js.Dynamic.literal(
          timestamp = ts(),
          level = "info",
          context = "main",
          message = "Agent started"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val l      = parsed.asInstanceOf[OplogEntry.Log]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.Log],
        l.params.level == LogLevel.Info,
        l.params.context == "main",
        l.params.message == "Agent started"
      )
    },
    test("AgentInvocationFinished with response from dynamic (agent-method result)") {
      val tdvDyn = js.Dynamic.literal(
        value = js.Dynamic.literal(tag = "tuple", `val` = js.Array[js.Any]()),
        schema = js.Dynamic.literal(tag = "tuple", `val` = js.Array[js.Any]())
      )
      val raw = wrapEntry(
        "agent-invocation-finished",
        js.Dynamic.literal(
          timestamp = ts(),
          invocationResult = js.Dynamic.literal(
            tag = "agent-method",
            `val` = js.Dynamic.literal(output = tdvDyn)
          ),
          consumedFuel = js.BigInt("1000"),
          componentRevision = js.BigInt("1")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val c      = parsed.asInstanceOf[OplogEntry.AgentInvocationFinished]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.AgentInvocationFinished],
        c.params.response.isDefined == true,
        c.params.consumedFuel == 1000L
      )
    },
    test("AgentInvocationFinished without response from dynamic (manual-update result)") {
      val raw = wrapEntry(
        "agent-invocation-finished",
        js.Dynamic.literal(
          timestamp = ts(),
          invocationResult = js.Dynamic.literal(tag = "manual-update"),
          consumedFuel = js.BigInt("0"),
          componentRevision = js.BigInt("1")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.AgentInvocationFinished],
        parsed.asInstanceOf[OplogEntry.AgentInvocationFinished].params.response == None
      )
    },
    test("HostCall from dynamic") {
      val vatDyn = js.Dynamic.literal(
        value = js.Dynamic.literal(nodes = js.Array(js.Dynamic.literal(tag = "prim-s32", `val` = 42))),
        typ = js.Dynamic.literal(nodes =
          js.Array(
            js.Dynamic
              .literal(name = js.undefined, owner = js.undefined, `type` = js.Dynamic.literal(tag = "prim-s32-type"))
          )
        )
      )
      val raw = wrapEntry(
        "imported-function-invoked",
        js.Dynamic.literal(
          timestamp = ts(),
          functionName = "wasi:io/read",
          request = vatDyn,
          response = vatDyn,
          wrappedFunctionType = js.Dynamic.literal(tag = "read-remote")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val i      = parsed.asInstanceOf[OplogEntry.HostCall]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.HostCall],
        i.params.functionName == "wasi:io/read",
        i.params.wrappedFunctionType == DurabilityApi.DurableFunctionType.ReadRemote
      )
    },
    test("CreateResource from dynamic") {
      val raw = wrapEntry(
        "create-resource",
        js.Dynamic.literal(
          timestamp = ts(),
          resourceId = js.BigInt("1"),
          name = "handle",
          owner = "golem:api"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val cr     = parsed.asInstanceOf[OplogEntry.CreateResource]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.CreateResource],
        cr.params.resourceId == BigInt(1),
        cr.params.name == "handle",
        cr.params.owner == "golem:api"
      )
    },
    test("DropResource from dynamic") {
      val raw = wrapEntry(
        "drop-resource",
        js.Dynamic.literal(
          timestamp = ts(),
          resourceId = js.BigInt("1"),
          name = "handle",
          owner = "golem:api"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.DropResource])
    },
    test("ActivatePlugin from dynamic") {
      val raw = wrapEntry(
        "activate-plugin",
        js.Dynamic.literal(
          timestamp = ts(),
          plugin = js.Dynamic.literal(
            name = "my-plugin",
            version = "1.0",
            parameters = js.Array(js.Tuple2("key", "val"))
          )
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val p      = parsed.asInstanceOf[OplogEntry.ActivatePlugin].params.plugin
      assertTrue(
        parsed.isInstanceOf[OplogEntry.ActivatePlugin],
        p.name == "my-plugin",
        p.parameters == Map("key" -> "val")
      )
    },
    test("DeactivatePlugin from dynamic") {
      val raw = wrapEntry(
        "deactivate-plugin",
        js.Dynamic.literal(
          timestamp = ts(),
          plugin = js.Dynamic.literal(
            name = "my-plugin",
            version = "2.0",
            parameters = js.Array[js.Any]()
          )
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.isInstanceOf[OplogEntry.DeactivatePlugin])
    },
    test("Revert from dynamic") {
      val raw = wrapEntry(
        "revert",
        js.Dynamic.literal(
          timestamp = ts(),
          droppedRegion = js.Dynamic.literal(start = js.BigInt("0"), end = js.BigInt("10"))
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val r      = parsed.asInstanceOf[OplogEntry.Revert]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.Revert],
        r.params.start == BigInt(0),
        r.params.end == BigInt(10)
      )
    },
    test("StartSpan from dynamic with attributes") {
      val raw = wrapEntry(
        "start-span",
        js.Dynamic.literal(
          timestamp = ts(),
          spanId = "span-1",
          parent = "parent-span",
          linkedContextId = "linked",
          attributes = js.Array(
            js.Dynamic.literal(
              key = "env",
              value = js.Dynamic.literal(tag = "string", `val` = "prod")
            )
          )
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val s      = parsed.asInstanceOf[OplogEntry.StartSpan]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.StartSpan],
        s.params.spanId == "span-1",
        s.params.parent == Some("parent-span"),
        s.params.linkedContext == Some("linked"),
        s.params.attributes.size == 1,
        s.params.attributes.head.key == "env"
      )
    },
    test("StartSpan from dynamic without optional fields") {
      val raw = wrapEntry(
        "start-span",
        js.Dynamic.literal(
          timestamp = ts(),
          spanId = "span-2",
          parent = js.undefined,
          linkedContextId = js.undefined,
          attributes = js.Array[js.Any]()
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val s      = parsed.asInstanceOf[OplogEntry.StartSpan]
      assertTrue(
        s.params.parent == None,
        s.params.linkedContext == None,
        s.params.attributes.isEmpty
      )
    },
    test("SetSpanAttribute from dynamic") {
      val raw = wrapEntry(
        "set-span-attribute",
        js.Dynamic.literal(
          timestamp = ts(),
          spanId = "span-1",
          key = "priority",
          value = js.Dynamic.literal(tag = "string", `val` = "high")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val sa     = parsed.asInstanceOf[OplogEntry.SetSpanAttribute]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.SetSpanAttribute],
        sa.params.key == "priority",
        sa.params.value == ContextApi.AttributeValue.StringValue("high")
      )
    },
    test("FailedUpdate with details from dynamic") {
      val raw = wrapEntry(
        "failed-update",
        js.Dynamic.literal(
          timestamp = ts(),
          targetRevision = js.BigInt("3"),
          details = "compile error"
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.FailedUpdate],
        parsed.asInstanceOf[OplogEntry.FailedUpdate].params.details == Some("compile error")
      )
    },
    test("FailedUpdate without details from dynamic") {
      val raw = wrapEntry(
        "failed-update",
        js.Dynamic.literal(
          timestamp = ts(),
          targetRevision = js.BigInt("3"),
          details = js.undefined
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(parsed.asInstanceOf[OplogEntry.FailedUpdate].params.details == None)
    },
    test("PendingUpdate auto-update from dynamic") {
      val raw = wrapEntry(
        "pending-update",
        js.Dynamic.literal(
          timestamp = ts(),
          targetRevision = js.BigInt("5"),
          updateDescription = js.Dynamic.literal(tag = "auto-update")
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      assertTrue(
        parsed.isInstanceOf[OplogEntry.PendingUpdate],
        parsed.asInstanceOf[OplogEntry.PendingUpdate].params.updateDescription == UpdateDescription.AutoUpdate
      )
    },
    test("PendingUpdate snapshot-based from dynamic") {
      val snapshotData = new scala.scalajs.js.typedarray.Uint8Array(js.Array[Short](1, 2, 3))
      val raw          = wrapEntry(
        "pending-update",
        js.Dynamic.literal(
          timestamp = ts(),
          targetRevision = js.BigInt("5"),
          updateDescription = js.Dynamic.literal(
            tag = "snapshot-based",
            `val` = js.Dynamic.literal(payload = snapshotData, mimeType = "application/octet-stream")
          )
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val ud     = parsed.asInstanceOf[OplogEntry.PendingUpdate].params.updateDescription
      assertTrue(
        parsed.isInstanceOf[OplogEntry.PendingUpdate],
        ud.isInstanceOf[UpdateDescription.SnapshotBased],
        ud.asInstanceOf[UpdateDescription.SnapshotBased].data.toList == List[Byte](1, 2, 3)
      )
    },
    test("SuccessfulUpdate from dynamic") {
      val raw = wrapEntry(
        "successful-update",
        js.Dynamic.literal(
          timestamp = ts(),
          targetRevision = js.BigInt("3"),
          newComponentSize = js.BigInt("2048"),
          newActivePlugins = js.Array(
            js.Dynamic.literal(name = "p1", version = "1.0", parameters = js.Array[js.Any]())
          )
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val su     = parsed.asInstanceOf[OplogEntry.SuccessfulUpdate]
      assertTrue(
        parsed.isInstanceOf[OplogEntry.SuccessfulUpdate],
        su.params.newComponentSize == BigInt(2048),
        su.params.newActivePlugins.size == 1,
        su.params.newActivePlugins.head.name == "p1"
      )
    },
    test("PendingAgentInvocation with exported-function from dynamic") {
      val dummyInput = js.Dynamic.literal(
        value = js.Dynamic.literal(tag = "tuple", `val` = js.Array[js.Any]()),
        schema = js.Dynamic.literal(tag = "tuple", `val` = js.Array[js.Any]())
      )
      val raw = wrapEntry(
        "pending-agent-invocation",
        js.Dynamic.literal(
          timestamp = ts(),
          invocation = js.Dynamic.literal(
            tag = "exported-function",
            `val` = js.Dynamic.literal(
              idempotencyKey = "idem-1",
              methodName = "increment",
              functionInput = dummyInput,
              traceId = "trace-1",
              traceStates = js.Array[String](),
              invocationContext = js.Array[js.Any]()
            )
          )
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val inv    = parsed.asInstanceOf[OplogEntry.PendingAgentInvocation].params.invocation
      assertTrue(
        parsed.isInstanceOf[OplogEntry.PendingAgentInvocation],
        inv.isInstanceOf[AgentInvocation.ExportedFunction],
        inv.asInstanceOf[AgentInvocation.ExportedFunction].params.functionName == "increment"
      )
    },
    test("PendingAgentInvocation with manual-update from dynamic") {
      val raw = wrapEntry(
        "pending-agent-invocation",
        js.Dynamic.literal(
          timestamp = ts(),
          invocation = js.Dynamic.literal(
            tag = "manual-update",
            `val` = js.Dynamic.literal(targetRevision = js.BigInt("7"))
          )
        )
      )
      val parsed = OplogEntry.fromJs(raw)
      val inv    = parsed.asInstanceOf[OplogEntry.PendingAgentInvocation].params.invocation
      assertTrue(
        inv.isInstanceOf[AgentInvocation.ManualUpdate],
        inv.asInstanceOf[AgentInvocation.ManualUpdate].componentRevision == BigInt(7)
      )
    },
    // --- Edge cases ---

    test("unknown oplog entry tag throws") {
      val raw = js.Dynamic.literal(tag = "unknown-entry", `val` = js.Dynamic.literal())
      assertTrue(scala.util.Try(OplogEntry.fromJs(raw)).isFailure)
    },
    test("LogLevel.fromString covers all variants") {
      assertTrue(
        LogLevel.fromString("stdout") == LogLevel.Stdout,
        LogLevel.fromString("stderr") == LogLevel.Stderr,
        LogLevel.fromString("trace") == LogLevel.Trace,
        LogLevel.fromString("debug") == LogLevel.Debug,
        LogLevel.fromString("info") == LogLevel.Info,
        LogLevel.fromString("warn") == LogLevel.Warn,
        LogLevel.fromString("error") == LogLevel.Error,
        LogLevel.fromString("critical") == LogLevel.Critical,
        LogLevel.fromString("unknown") == LogLevel.Info
      )
    }
  )
}
