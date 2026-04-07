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

import golem.{Guards, HostApi}
import golem.host.RetryApi
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class GuardsDemoImpl(@unused private val name: String) extends GuardsDemo {

  override def guardsBlockDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Block-scoped Guards Demo ===\n")

    // withRetryPolicy
    val origPolicies = RetryApi.getRetryPolicies()
    sb.append(s"original retry policies count=${origPolicies.size}\n")
    sb.append(s"result=retry-skipped (named retry policies require host setup)\n")

    // withPersistenceLevel
    val origLevel = HostApi.getOplogPersistenceLevel()
    sb.append(s"original persistence=$origLevel\n")
    val levelResult = Guards.withPersistenceLevel(HostApi.PersistenceLevel.PersistNothing) {
      val inner = HostApi.getOplogPersistenceLevel()
      sb.append(s"inside withPersistenceLevel: level=$inner\n")
      "level-ok"
    }
    val afterLevel = HostApi.getOplogPersistenceLevel()
    sb.append(s"after withPersistenceLevel: level=$afterLevel, result=$levelResult\n")

    // withIdempotenceMode
    val origIdem = HostApi.getIdempotenceMode()
    sb.append(s"original idempotence=$origIdem\n")
    val idemResult = Guards.withIdempotenceMode(!origIdem) {
      val inner = HostApi.getIdempotenceMode()
      sb.append(s"inside withIdempotenceMode: mode=$inner\n")
      "idem-ok"
    }
    val afterIdem = HostApi.getIdempotenceMode()
    sb.append(s"after withIdempotenceMode: mode=$afterIdem, result=$idemResult\n")

    // atomically
    val atomicResult = Guards.atomically {
      sb.append("inside atomically block\n")
      "atomic-ok"
    }
    sb.append(s"after atomically: result=$atomicResult\n")

    sb.result()
  }

  override def guardsResourceDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Resource-style Guards Demo ===\n")

    // useRetryPolicy
    val origPolicies = RetryApi.getRetryPolicies()
    sb.append(s"original retry policies count=${origPolicies.size}\n")
    sb.append(s"retry-skipped (named retry policies require host setup)\n")

    // usePersistenceLevel
    val origLevel = HostApi.getOplogPersistenceLevel()
    sb.append(s"original persistence=$origLevel\n")
    val levelGuard = Guards.usePersistenceLevel(HostApi.PersistenceLevel.PersistRemoteSideEffects)
    val innerLevel = HostApi.getOplogPersistenceLevel()
    sb.append(s"after usePersistenceLevel: level=$innerLevel\n")
    levelGuard.close()
    val afterLevel = HostApi.getOplogPersistenceLevel()
    sb.append(s"after close(): level=$afterLevel\n")

    // useIdempotenceMode
    val origIdem = HostApi.getIdempotenceMode()
    sb.append(s"original idempotence=$origIdem\n")
    val idemGuard = Guards.useIdempotenceMode(!origIdem)
    val innerIdem = HostApi.getIdempotenceMode()
    sb.append(s"after useIdempotenceMode: mode=$innerIdem\n")
    idemGuard.drop()
    val afterIdem = HostApi.getIdempotenceMode()
    sb.append(s"after drop(): mode=$afterIdem\n")

    // markAtomicOperation
    val atomicGuard = Guards.markAtomicOperation()
    sb.append("markAtomicOperation: guard created\n")
    atomicGuard.drop()
    sb.append("markAtomicOperation: guard dropped\n")

    sb.result()
  }

  override def oplogDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Oplog Management Demo ===\n")

    val idx = HostApi.getOplogIndex()
    sb.append(s"current oplog index=$idx\n")

    val beginIdx = HostApi.markBeginOperation()
    sb.append(s"markBeginOperation returned=$beginIdx\n")

    HostApi.markEndOperation(beginIdx)
    sb.append("markEndOperation completed\n")

    val afterIdx = HostApi.getOplogIndex()
    sb.append(s"oplog index after atomic region=$afterIdx\n")

    HostApi.oplogCommit(1)
    sb.append("oplogCommit(1) completed\n")

    sb.result()
  }
}
