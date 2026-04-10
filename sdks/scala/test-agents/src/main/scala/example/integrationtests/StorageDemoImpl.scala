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
import golem.wasi.{Blobstore, Config, KeyValue}

import scala.annotation.unused
import scala.concurrent.Future

@agentImplementation()
final class StorageDemoImpl(@unused private val name: String) extends StorageDemo {

  override def keyValueDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== KeyValue Demo ===\n")

    val bucket: KeyValue.Bucket = KeyValue.Bucket.open("demo-bucket")

    val testData = "hello world".getBytes("UTF-8")
    bucket.set("key1", testData)
    sb.append("set('key1', 'hello world') done\n")

    val value: Option[Array[Byte]] = bucket.get("key1")
    sb.append(s"get('key1') = ${value.map(b => new String(b, "UTF-8"))}\n")

    val exists: Boolean = bucket.exists("key1")
    sb.append(s"exists('key1') = $exists\n")

    bucket.set("key2", "value2".getBytes("UTF-8"))
    bucket.set("key3", "value3".getBytes("UTF-8"))

    val keys: List[String] = bucket.keys()
    sb.append(s"keys() = ${keys.mkString(", ")}\n")

    val many: List[Option[Array[Byte]]] = bucket.getMany(List("key1", "key2", "missing"))
    sb.append(s"getMany(['key1','key2','missing']): ${many.map(_.map(_.length).getOrElse(-1)).mkString(", ")}\n")

    bucket.delete("key1")
    sb.append(s"delete('key1') done, exists now = ${bucket.exists("key1")}\n")

    bucket.deleteMany(List("key2", "key3"))
    sb.append(s"deleteMany(['key2','key3']) done, keys now = ${bucket.keys().mkString(", ")}\n")

    sb.toString()
  }

  override def blobstoreDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Blobstore Demo ===\n")

    val container: Blobstore.Container = Blobstore.createContainer("demo-container")
    sb.append(s"Created container: ${container.name()}\n")

    val info: Blobstore.ContainerMetadata = container.info()
    sb.append(s"Container info: name=${info.name} createdAt=${info.createdAt}\n")

    val testData = "Hello, Blobstore!".getBytes("UTF-8")
    container.writeData("test-object.txt", testData)
    sb.append(s"writeData('test-object.txt', ${testData.length} bytes) done\n")

    val hasObj: Boolean = container.hasObject("test-object.txt")
    sb.append(s"hasObject('test-object.txt') = $hasObj\n")

    val objInfo: Blobstore.ObjectMetadata = container.objectInfo("test-object.txt")
    sb.append(s"objectInfo: name=${objInfo.name} container=${objInfo.container} size=${objInfo.size}\n")

    val data: Array[Byte] = container.getData("test-object.txt", 0L, testData.length.toLong)
    sb.append(s"getData: '${new String(data, "UTF-8")}'\n")

    val objects: List[String] = container.listObjects()
    sb.append(s"listObjects: ${objects.mkString(", ")}\n")

    container.deleteObject("test-object.txt")
    sb.append(s"deleteObject done, hasObject now = ${container.hasObject("test-object.txt")}\n")

    container.writeData("obj1.txt", "a".getBytes)
    container.writeData("obj2.txt", "b".getBytes)
    container.deleteObjects(List("obj1.txt", "obj2.txt"))
    sb.append("Bulk delete done.\n")

    container.clear()
    sb.append("Container cleared.\n")

    val existsAfter: Boolean = Blobstore.containerExists("demo-container")
    sb.append(s"containerExists('demo-container') = $existsAfter\n")

    val src  = Blobstore.ObjectId("demo-container", "src-obj")
    val dest = Blobstore.ObjectId("demo-container", "dest-obj")
    sb.append(s"ObjectId types: src=$src dest=$dest\n")

    Blobstore.deleteContainer("demo-container")
    sb.append("Container deleted.\n")

    sb.toString()
  }

  override def configDemo(): Future[String] = Future.successful {
    val sb = new StringBuilder
    sb.append("=== Config Demo ===\n")

    Config.get("test-key") match {
      case Right(Some(value))                     => sb.append(s"Config.get('test-key') = $value\n")
      case Right(None)                            => sb.append("Config.get('test-key') = None (not set)\n")
      case Left(Config.ConfigError.Upstream(msg)) => sb.append(s"Config upstream error: $msg\n")
      case Left(Config.ConfigError.Io(msg))       => sb.append(s"Config IO error: $msg\n")
    }

    Config.getAll() match {
      case Right(entries) =>
        sb.append(s"Config.getAll(): ${entries.size} entries\n")
        entries.foreach { case (k, v) => sb.append(s"  $k = $v\n") }
      case Left(err) =>
        val msg = err match {
          case Config.ConfigError.Upstream(m) => s"upstream: $m"
          case Config.ConfigError.Io(m)       => s"io: $m"
        }
        sb.append(s"Config.getAll() error: $msg\n")
    }

    sb.toString()
  }
}
