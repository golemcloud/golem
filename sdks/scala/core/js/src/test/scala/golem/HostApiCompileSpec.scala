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

package golem

import golem.host.js.JsEnvironmentId
import zio.test._

import scala.scalajs.js

object HostApiCompileSpec extends ZIOSpecDefault {

  def spec = suite("HostApiCompileSpec")(
    // ---------------------------------------------------------------------------
    // PersistenceLevel
    // ---------------------------------------------------------------------------

    test("all PersistenceLevel variants construct") {
      val levels: List[HostApi.PersistenceLevel] = List(
        HostApi.PersistenceLevel.PersistNothing,
        HostApi.PersistenceLevel.PersistRemoteSideEffects,
        HostApi.PersistenceLevel.Smart,
        HostApi.PersistenceLevel.Unknown("future-level")
      )
      assertTrue(levels.size == 4)
    },

    test("PersistenceLevel.fromTag roundtrips all known tags") {
      assertTrue(
        HostApi.PersistenceLevel.fromTag("persist-nothing") == HostApi.PersistenceLevel.PersistNothing,
        HostApi.PersistenceLevel.fromTag("persist-remote-side-effects") ==
          HostApi.PersistenceLevel.PersistRemoteSideEffects,
        HostApi.PersistenceLevel.fromTag("smart") == HostApi.PersistenceLevel.Smart
      )
    },

    test("PersistenceLevel.fromTag returns Unknown for unrecognized tags") {
      val result = HostApi.PersistenceLevel.fromTag("new-level")
      assertTrue(result == HostApi.PersistenceLevel.Unknown("new-level"))
    },

    test("PersistenceLevel exhaustive pattern match compiles") {
      def describe(level: HostApi.PersistenceLevel): String = level match {
        case HostApi.PersistenceLevel.PersistNothing           => "nothing"
        case HostApi.PersistenceLevel.PersistRemoteSideEffects => "remote"
        case HostApi.PersistenceLevel.Smart                    => "smart"
        case HostApi.PersistenceLevel.Unknown(tag)             => s"unknown($tag)"
      }
      assertTrue(describe(HostApi.PersistenceLevel.Smart) == "smart")
    },

    // ---------------------------------------------------------------------------
    // ForkResult
    // ---------------------------------------------------------------------------

    test("ForkResult.Original and Forked variants compile") {
      val phantomId                    = Uuid(BigInt(1), BigInt(2))
      val original: HostApi.ForkResult = HostApi.ForkResult.Original(phantomId)
      val forked: HostApi.ForkResult   = HostApi.ForkResult.Forked(phantomId)
      assertTrue(
        original.forkedPhantomId == phantomId,
        forked.forkedPhantomId == phantomId
      )
    },

    test("ForkResult pattern match compiles") {
      val result: HostApi.ForkResult =
        HostApi.ForkResult.Original(Uuid(BigInt(0), BigInt(0)))
      val label = result match {
        case HostApi.ForkResult.Original(_) => "original"
        case HostApi.ForkResult.Forked(_)   => "forked"
      }
      assertTrue(label == "original")
    },

    // ---------------------------------------------------------------------------
    // AgentMetadata
    // ---------------------------------------------------------------------------

    test("AgentMetadata construction with all fields") {
      val meta = HostApi.AgentMetadata(
        agentId = null.asInstanceOf[HostApi.AgentIdLiteral],
        args = List("arg1", "arg2"),
        env = Map("KEY" -> "VALUE"),
        configVars = Map("cfg" -> "val"),
        status = null.asInstanceOf[HostApi.AgentStatus],
        componentRevision = BigInt(3),
        retryCount = BigInt(0),
        agentType = "my-agent",
        agentName = "instance-1",
        componentId = null.asInstanceOf[HostApi.ComponentIdLiteral],
        environmentId = null.asInstanceOf[JsEnvironmentId]
      )
      assertTrue(
        meta.args == List("arg1", "arg2"),
        meta.env == Map("KEY" -> "VALUE"),
        meta.agentType == "my-agent",
        meta.agentName == "instance-1",
        meta.componentRevision == BigInt(3)
      )
    },

    // ---------------------------------------------------------------------------
    // AgentStatus
    // ---------------------------------------------------------------------------

    test("all AgentStatus variants accessible") {
      val statuses = List(
        HostApi.AgentStatus.Running,
        HostApi.AgentStatus.Idle,
        HostApi.AgentStatus.Suspended,
        HostApi.AgentStatus.Interrupted,
        HostApi.AgentStatus.Retrying,
        HostApi.AgentStatus.Failed,
        HostApi.AgentStatus.Exited
      )
      assertTrue(statuses.size == 7)
    },

    // ---------------------------------------------------------------------------
    // UpdateMode
    // ---------------------------------------------------------------------------

    test("UpdateMode variants accessible") {
      val modes = List(HostApi.UpdateMode.Automatic, HostApi.UpdateMode.SnapshotBased)
      assertTrue(modes.size == 2)
    },

    // ---------------------------------------------------------------------------
    // Filter types
    // ---------------------------------------------------------------------------

    test("FilterComparator variants accessible") {
      val comparators = List(
        HostApi.FilterComparator.Equal,
        HostApi.FilterComparator.NotEqual,
        HostApi.FilterComparator.GreaterEqual,
        HostApi.FilterComparator.Greater,
        HostApi.FilterComparator.LessEqual,
        HostApi.FilterComparator.Less
      )
      assertTrue(comparators.size == 6)
    },

    test("StringFilterComparator variants accessible") {
      val comparators = List(
        HostApi.StringFilterComparator.Equal,
        HostApi.StringFilterComparator.NotEqual,
        HostApi.StringFilterComparator.Like,
        HostApi.StringFilterComparator.NotLike,
        HostApi.StringFilterComparator.StartsWith
      )
      assertTrue(comparators.size == 5)
    },

    // ---------------------------------------------------------------------------
    // RevertAgentTarget
    // ---------------------------------------------------------------------------

    test("RevertAgentTarget factory methods") {
      val byIndex: HostApi.RevertAgentTarget = HostApi.RevertAgentTarget.RevertToOplogIndex(BigInt(42))
      val byCount: HostApi.RevertAgentTarget = HostApi.RevertAgentTarget.RevertLastInvocations(BigInt(3))
      assertTrue(byIndex != null, byCount != null)
    },

    test("Uuid construction") {
      val u = Uuid(BigInt(123456789L), BigInt(987654321L))
      assertTrue(
        u.highBits == BigInt(123456789L),
        u.lowBits == BigInt(987654321L)
      )
    },

    // ---------------------------------------------------------------------------
    // RegisteredAgentType (Scala case class, no js.Object)
    // ---------------------------------------------------------------------------

    test("RegisteredAgentType construction") {
      val rat = HostApi.RegisteredAgentType(
        typeName = "my-agent",
        implementedBy = null.asInstanceOf[HostApi.ComponentIdLiteral]
      )
      assertTrue(rat.typeName == "my-agent")
    },

    // ---------------------------------------------------------------------------
    // AgentIdParts (Scala case class, no js.Dynamic)
    // ---------------------------------------------------------------------------

    test("AgentIdParts construction") {
      val parts = HostApi.AgentIdParts(
        agentTypeName = "counter",
        phantom = Some(Uuid(BigInt(1), BigInt(2)))
      )
      assertTrue(
        parts.agentTypeName == "counter",
        parts.phantom.isDefined
      )
    },

    test("AgentIdParts with no phantom") {
      val parts = HostApi.AgentIdParts("counter", None)
      assertTrue(parts.phantom.isEmpty)
    },

    // ---------------------------------------------------------------------------
    // Filter construction APIs
    // ---------------------------------------------------------------------------

    test("AgentNameFilter construction with StringFilterComparator") {
      val f: HostApi.AgentNameFilter = HostApi.AgentNameFilter(HostApi.StringFilterComparator.Equal, "my-agent")
      assertTrue(f != null)
    },

    test("AgentStatusFilter construction with FilterComparator") {
      val f: HostApi.AgentStatusFilter =
        HostApi.AgentStatusFilter(HostApi.FilterComparator.Equal, HostApi.AgentStatus.Running)
      assertTrue(f != null)
    },

    test("AgentVersionFilter construction") {
      val f: HostApi.AgentVersionFilter = HostApi.AgentVersionFilter(HostApi.FilterComparator.GreaterEqual, BigInt(2))
      assertTrue(f != null)
    },

    test("AgentCreatedAtFilter construction") {
      val f: HostApi.AgentCreatedAtFilter =
        HostApi.AgentCreatedAtFilter(HostApi.FilterComparator.Less, BigInt(1700000000))
      assertTrue(f != null)
    },

    test("AgentEnvFilter construction") {
      val f: HostApi.AgentEnvFilter = HostApi.AgentEnvFilter("ENV_VAR", HostApi.StringFilterComparator.Like, "prod%")
      assertTrue(f != null)
    },

    test("AgentConfigVarsFilter construction") {
      val f: HostApi.AgentConfigVarsFilter =
        HostApi.AgentConfigVarsFilter("config-key", HostApi.StringFilterComparator.StartsWith, "prefix")
      assertTrue(f != null)
    },

    test("AgentPropertyFilter.name wraps AgentNameFilter") {
      val nameFilter                      = HostApi.AgentNameFilter(HostApi.StringFilterComparator.Equal, "test")
      val pf: HostApi.AgentPropertyFilter = HostApi.AgentPropertyFilter.name(nameFilter)
      assertTrue(pf != null)
    },

    test("AgentPropertyFilter.status wraps AgentStatusFilter") {
      val statusFilter                    = HostApi.AgentStatusFilter(HostApi.FilterComparator.Equal, HostApi.AgentStatus.Idle)
      val pf: HostApi.AgentPropertyFilter = HostApi.AgentPropertyFilter.status(statusFilter)
      assertTrue(pf != null)
    },

    test("AgentPropertyFilter.version wraps AgentVersionFilter") {
      val versionFilter                   = HostApi.AgentVersionFilter(HostApi.FilterComparator.GreaterEqual, BigInt(1))
      val pf: HostApi.AgentPropertyFilter = HostApi.AgentPropertyFilter.version(versionFilter)
      assertTrue(pf != null)
    },

    test("AgentPropertyFilter.createdAt wraps AgentCreatedAtFilter") {
      val createdAtFilter                 = HostApi.AgentCreatedAtFilter(HostApi.FilterComparator.Greater, BigInt(0))
      val pf: HostApi.AgentPropertyFilter = HostApi.AgentPropertyFilter.createdAt(createdAtFilter)
      assertTrue(pf != null)
    },

    test("AgentPropertyFilter.env wraps AgentEnvFilter") {
      val envFilter                       = HostApi.AgentEnvFilter("KEY", HostApi.StringFilterComparator.NotEqual, "val")
      val pf: HostApi.AgentPropertyFilter = HostApi.AgentPropertyFilter.env(envFilter)
      assertTrue(pf != null)
    },

    test("AgentPropertyFilter.wasiConfigVars wraps AgentConfigVarsFilter") {
      val configFilter                    = HostApi.AgentConfigVarsFilter("cfg", HostApi.StringFilterComparator.Equal, "v")
      val pf: HostApi.AgentPropertyFilter = HostApi.AgentPropertyFilter.wasiConfigVars(configFilter)
      assertTrue(pf != null)
    },

    test("AgentAllFilter combines multiple AgentPropertyFilters") {
      val nameFilter =
        HostApi.AgentPropertyFilter.name(HostApi.AgentNameFilter(HostApi.StringFilterComparator.Equal, "a"))
      val statusFilter = HostApi.AgentPropertyFilter.status(
        HostApi.AgentStatusFilter(HostApi.FilterComparator.Equal, HostApi.AgentStatus.Running)
      )
      val all: HostApi.AgentAllFilter = HostApi.AgentAllFilter(List(nameFilter, statusFilter))
      assertTrue(all != null)
    },

    test("AgentAnyFilter combines multiple AgentAllFilters") {
      val all1 = HostApi.AgentAllFilter(
        List(HostApi.AgentPropertyFilter.name(HostApi.AgentNameFilter(HostApi.StringFilterComparator.Equal, "a")))
      )
      val all2 = HostApi.AgentAllFilter(
        List(HostApi.AgentPropertyFilter.name(HostApi.AgentNameFilter(HostApi.StringFilterComparator.Equal, "b")))
      )
      val any: HostApi.AgentAnyFilter = HostApi.AgentAnyFilter(List(all1, all2))
      assertTrue(any != null)
    },

    // ---------------------------------------------------------------------------
    // Literal companion object construction
    // ---------------------------------------------------------------------------

    test("UuidLiteral companion constructs from js.BigInts") {
      val uuid: HostApi.UuidLiteral = HostApi.UuidLiteral(js.BigInt(123), js.BigInt(456))
      assertTrue(uuid != null)
    },

    test("ComponentIdLiteral companion constructs from UuidLiteral") {
      val uuid                            = HostApi.UuidLiteral(js.BigInt(1), js.BigInt(2))
      val cid: HostApi.ComponentIdLiteral = HostApi.ComponentIdLiteral(uuid)
      assertTrue(cid != null)
    },

    test("AgentIdLiteral companion constructs from ComponentIdLiteral and name") {
      val uuid                        = HostApi.UuidLiteral(js.BigInt(1), js.BigInt(2))
      val cid                         = HostApi.ComponentIdLiteral(uuid)
      val aid: HostApi.AgentIdLiteral = HostApi.AgentIdLiteral(cid, "my-agent")
      assertTrue(aid != null)
    },

    test("PromiseIdLiteral companion constructs from AgentIdLiteral and oplog index") {
      val uuid = HostApi.UuidLiteral(js.BigInt(1), js.BigInt(2))
      val cid  = HostApi.ComponentIdLiteral(uuid)
      val aid  = HostApi.AgentIdLiteral(cid, "my-agent")
      val pid  = HostApi.PromiseIdLiteral(aid, js.BigInt(42))
      assertTrue(pid != null)
    }
  )
}
