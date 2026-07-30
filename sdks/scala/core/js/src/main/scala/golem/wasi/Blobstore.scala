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

package golem.wasi

import golem.host.js._

import scala.scalajs.js
import scala.scalajs.js.annotation.{JSImport, JSName}
import scala.scalajs.js.typedarray.Uint8Array
import scala.collection.mutable.ListBuffer
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

/**
 * Scala.js facade for WASI blobstore (`wasi:blobstore`).
 *
 * WIT interfaces:
 * {{{
 *   // wasi:blobstore/types
 *   resource outgoing-value { new-outgoing-value: static func() -> outgoing-value; outgoing-value-write-body: func(stream<u8>) -> result }
 *   resource incoming-value { incoming-value-consume-sync: func() -> result<list<u8>, error>; size: func() -> u64 }
 *   record container-metadata { name: container-name, created-at: timestamp }
 *   record object-metadata { name: object-name, container: container-name, created-at: timestamp, size: object-size }
 *   record object-id { container: container-name, object: object-name }
 *
 *   // wasi:blobstore/container
 *   resource container { name, info, get-data, write-data, list-objects, delete-object, has-object, object-info, clear }
 *
 *   // wasi:blobstore/blobstore
 *   create-container, get-container, delete-container, container-exists, copy-object, move-object
 * }}}
 */
object Blobstore {

  // --- Native imports ---

  @js.native
  @JSImport("wasi:blobstore/blobstore", JSImport.Namespace)
  private object BlobstoreModule extends js.Object {
    def createContainer(name: String): js.Any       = js.native
    def getContainer(name: String): js.Any          = js.native
    def deleteContainer(name: String): Unit         = js.native
    def containerExists(name: String): Boolean      = js.native
    def copyObject(src: js.Any, dest: js.Any): Unit = js.native
    def moveObject(src: js.Any, dest: js.Any): Unit = js.native
  }

  @js.native
  @JSImport("wasi:blobstore/container", JSImport.Namespace)
  private object ContainerModule extends js.Object

  @js.native
  @JSImport("wasi:blobstore/types", "OutgoingValue")
  private object BlobstoreOutgoingValueClass extends js.Object {
    def newOutgoingValue(): JsBlobstoreOutgoingValue = js.native
  }

  @js.native
  @JSImport("wasi:blobstore/types", JSImport.Namespace)
  private object TypesModule extends js.Object

  // --- Data types ---

  final case class ContainerMetadata(name: String, createdAt: BigInt)
  final case class ObjectMetadata(name: String, container: String, createdAt: BigInt, size: Long)
  final case class ObjectId(container: String, name: String)

  // --- Container resource ---

  final class Container private[Blobstore] (private val underlying: JsBlobstoreContainer) {

    def name(): String =
      underlying.name()

    def info(): ContainerMetadata = {
      val raw = underlying.info()
      ContainerMetadata(raw.name, BigInt(raw.createdAt.toString))
    }

    def getData(objectName: String, start: Long, end: Long): Array[Byte] = {
      val iv  = underlying.getData(objectName, js.BigInt(start.toString), js.BigInt(end.toString))
      val raw = iv.incomingValueConsumeSync()
      uint8ArrayToBytes(raw)
    }

    def writeData(objectName: String, data: Array[Byte]): Unit = {
      val ov    = BlobstoreOutgoingValueClass.newOutgoingValue()
      val bytes = bytesToUint8Array(data)
      ov.outgoingValueWriteBody(new ByteStream(bytes).asInstanceOf[JsByteStream])
      underlying.writeData(objectName, ov)
    }

    def listObjects(): Future[List[String]] = {
      val iterator = underlying.listObjects().asyncIterator()
      val names    = ListBuffer.empty[String]

      def next(): Future[List[String]] =
        golem.FutureInterop.fromPromise(iterator.next()).flatMap { result =>
          if (result.done) Future.successful(names.toList)
          else {
            names += result.value
            next()
          }
        }

      next()
    }

    def deleteObject(name: String): Unit =
      underlying.deleteObject(name)

    def deleteObjects(names: List[String]): Unit = {
      val arr = js.Array[String]()
      names.foreach(arr.push(_))
      underlying.deleteObjects(arr)
    }

    def hasObject(name: String): Boolean =
      underlying.hasObject(name)

    def objectInfo(name: String): ObjectMetadata = {
      val raw = underlying.objectInfo(name)
      ObjectMetadata(
        name = raw.name,
        container = raw.container,
        createdAt = BigInt(raw.createdAt.toString),
        size = BigInt(raw.size.toString).toLong
      )
    }

    def clear(): Unit =
      underlying.clear()
  }

  // --- Top-level functions ---

  def createContainer(name: String): Container =
    new Container(BlobstoreModule.createContainer(name).asInstanceOf[JsBlobstoreContainer])

  def getContainer(name: String): Container =
    new Container(BlobstoreModule.getContainer(name).asInstanceOf[JsBlobstoreContainer])

  def deleteContainer(name: String): Unit =
    BlobstoreModule.deleteContainer(name)

  def containerExists(name: String): Boolean =
    BlobstoreModule.containerExists(name)

  def copyObject(src: ObjectId, dest: ObjectId): Unit =
    BlobstoreModule.copyObject(objectIdToJs(src).asInstanceOf[js.Any], objectIdToJs(dest).asInstanceOf[js.Any])

  def moveObject(src: ObjectId, dest: ObjectId): Unit =
    BlobstoreModule.moveObject(objectIdToJs(src).asInstanceOf[js.Any], objectIdToJs(dest).asInstanceOf[js.Any])

  // --- Helpers ---

  private def objectIdToJs(oid: ObjectId): JsObjectId =
    JsObjectId(oid.container, oid.name)

  private def uint8ArrayToBytes(arr: Uint8Array): Array[Byte] = {
    val bytes = new Array[Byte](arr.length)
    var i     = 0
    while (i < arr.length) { bytes(i) = arr(i).toByte; i += 1 }
    bytes
  }

  private def bytesToUint8Array(data: Array[Byte]): Uint8Array = {
    val jsArr = js.Array[Short]()
    data.foreach(b => jsArr.push((b.toInt & 0xff).toShort))
    new Uint8Array(jsArr.asInstanceOf[js.Iterable[Short]])
  }

  private final class ByteIterator(data: Uint8Array) extends js.Object {
    private var index = 0

    def next(): js.Promise[JsByteIteratorResult] = {
      val result =
        if (index >= data.length)
          js.Dynamic.literal("done" -> true)
        else {
          val value = data(index).toInt
          index += 1
          js.Dynamic.literal("done" -> false, "value" -> value)
        }
      js.Promise.resolve(result.asInstanceOf[JsByteIteratorResult])
    }
  }

  private final class ByteStream(data: Uint8Array) extends js.Object {
    @JSName(js.Symbol.asyncIterator)
    def asyncIterator(): JsByteIterator =
      new ByteIterator(data).asInstanceOf[JsByteIterator]
  }

  // --- Raw access (for forward compatibility) ---

  def blobstoreRaw: Any = BlobstoreModule
  def containerRaw: Any = ContainerModule
  def typesRaw: Any     = TypesModule
}
