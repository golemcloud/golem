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

import golem.runtime.annotations.agentImplementation
import golem.wasi

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class HostApiExplorerImpl(@unused private val name: String) extends HostApiExplorer {

  override def exploreConfig(): Future[String]     = Future.successful(exploreConfigSync())
  override def exploreDurability(): Future[String] = Future.successful(exploreDurabilitySync())
  override def exploreContext(): Future[String]    = Future.successful(exploreContextSync())
  override def exploreOplog(): Future[String]      = Future.successful(exploreOplogSync())
  override def exploreKeyValue(): Future[String]   = Future.successful(exploreKeyValueSync())
  override def exploreBlobstore(): Future[String]  = Future.successful(exploreBlobstoreSync())
  override def exploreRdbms(): Future[String]      = Future.successful(exploreRdbmsSync())

  override def exploreAll(): Future[String] = {
    val sb                                        = new StringBuilder
    def run(label: String, body: => String): Unit =
      try sb.append(s"=== $label ===\n$body\n")
      catch { case t: Throwable => sb.append(s"=== $label === ERROR: ${t.getMessage}\n") }

    run("CONFIG", exploreConfigSync())
    run("DURABILITY", exploreDurabilitySync())
    run("CONTEXT", exploreContextSync())
    run("OPLOG", exploreOplogSync())
    run("KEYVALUE", exploreKeyValueSync())
    run("BLOBSTORE", exploreBlobstoreSync())
    run("RDBMS", exploreRdbmsSync())

    Future.successful(sb.toString())
  }

  private def exploreConfigSync(): String = {
    val sb        = new StringBuilder
    val allResult = wasi.Config.getAll()
    sb.append(s"Config.getAll() = $allResult\n")
    val getResult = wasi.Config.get("test-key")
    sb.append(s"Config.get('test-key') = $getResult\n")
    sb.toString()
  }

  private def exploreDurabilitySync(): String = {
    val sb    = new StringBuilder
    val state = golem.host.DurabilityApi.currentDurableExecutionState()
    sb.append(s"DurabilityApi.currentDurableExecutionState() = $state\n")
    sb.append(s"  isLive=${state.isLive}, persistenceLevel=${state.persistenceLevel}\n")
    sb.toString()
  }

  private def exploreContextSync(): String = {
    val sb  = new StringBuilder
    val ctx = golem.host.ContextApi.currentContext()
    sb.append(s"traceId = ${ctx.traceId()}\n")
    sb.append(s"spanId = ${ctx.spanId()}\n")
    sb.append(s"parent = ${ctx.parent()}\n")
    val attrs = ctx.getAttributes(false).sortBy(_.key)
    sb.append(s"attributes (${attrs.size}):\n")
    attrs.foreach(a => sb.append(s"  ${a.key} = ${a.value}\n"))
    val headers = ctx.traceContextHeaders()
    sb.append(s"traceContextHeaders = $headers\n")

    val span = golem.host.ContextApi.startSpan("test-span")
    val ts   = span.startedAt()
    sb.append(s"span.startedAt() = DateTime(${ts.seconds}, ${ts.nanoseconds})\n")
    span.finish()

    sb.toString()
  }

  private def exploreOplogSync(): String = {
    val sb       = new StringBuilder
    val selfMeta = golem.HostApi.getSelfMetadata()
    val reader   = golem.host.OplogApi.GetOplog(selfMeta.agentId, BigInt(0))
    val batch    = reader.getNext()
    sb.append(s"OplogApi.GetOplog entries=${batch.map(_.size).getOrElse(0)}\n")
    batch.foreach { entries =>
      entries.take(5).foreach(e => sb.append(s"  $e\n"))
      if (entries.size > 5) sb.append(s"  ... and ${entries.size - 5} more\n")
    }
    sb.toString()
  }

  private def exploreKeyValueSync(): String = {
    val sb     = new StringBuilder
    val bucket = wasi.KeyValue.Bucket.open("test-bucket")

    bucket.set("hello", "world".getBytes("UTF-8"))
    sb.append(s"KeyValue set('hello', 'world') OK\n")

    val got = bucket.get("hello")
    val str = got.map(b => new String(b, "UTF-8"))
    sb.append(s"KeyValue get('hello') = $str\n")

    val ex = bucket.exists("hello")
    sb.append(s"KeyValue exists('hello') = $ex\n")

    val missing = bucket.get("missing-key")
    sb.append(s"KeyValue get('missing-key') = $missing\n")

    val allKeys = bucket.keys()
    sb.append(s"KeyValue keys() = $allKeys\n")

    bucket.delete("hello")
    sb.append(s"KeyValue delete('hello') OK\n")

    val afterDelete = bucket.exists("hello")
    sb.append(s"KeyValue exists('hello') after delete = $afterDelete\n")

    sb.toString()
  }

  private def exploreBlobstoreSync(): String = {
    val sb = new StringBuilder

    val exists1 = wasi.Blobstore.containerExists("test-blob-container")
    sb.append(s"containerExists('test-blob-container') = $exists1\n")

    val container = wasi.Blobstore.createContainer("test-blob-container")
    sb.append(s"createContainer('test-blob-container') OK, name=${container.name()}\n")

    val exists2 = wasi.Blobstore.containerExists("test-blob-container")
    sb.append(s"containerExists('test-blob-container') = $exists2\n")

    val hasObj1 = container.hasObject("greeting")
    sb.append(s"hasObject('greeting') = $hasObj1\n")

    val objects = container.listObjects()
    sb.append(s"listObjects() = $objects\n")

    wasi.Blobstore.deleteContainer("test-blob-container")
    sb.append(s"deleteContainer('test-blob-container') OK\n")

    sb.toString()
  }

  private def exploreRdbmsSync(): String = {
    val sb = new StringBuilder

    val pgResult = golem.host.Rdbms.Postgres.open("postgresql://invalid:5432/test")
    sb.append(s"Rdbms.Postgres.open() = ${pgResult.left.map(_.getClass.getSimpleName)}\n")

    val myResult = golem.host.Rdbms.Mysql.open("mysql://invalid:3306/test")
    sb.append(s"Rdbms.Mysql.open() = ${myResult.left.map(_.getClass.getSimpleName)}\n")

    sb.toString()
  }

}
