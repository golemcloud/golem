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

package golem.wasi

import golem.host.js._

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.js.typedarray.Uint8Array

/**
 * Scala.js facade for WASI keyvalue (`wasi:keyvalue@0.1.0`).
 *
 * Provides typed access to the keyvalue store via `Bucket`, `OutgoingValue`,
 * and `IncomingValue` resources. The Component Model JS binding unwraps
 * `result` types (throwing on `err`, returning `ok` directly) and maps `option`
 * to `undefined`.
 *
 * WIT interfaces:
 * {{{
 *   // wasi:keyvalue/types@0.1.0
 *   resource bucket { open-bucket: static func(name: string) -> result<bucket, error> }
 *   resource outgoing-value { new-outgoing-value: static func() -> outgoing-value; ... }
 *   resource incoming-value { incoming-value-consume-sync: func() -> result<list<u8>, error>; ... }
 *
 *   // wasi:keyvalue/eventual@0.1.0
 *   get: func(bucket, key) -> result<option<incoming-value>, error>
 *   set: func(bucket, key, outgoing-value) -> result<_, error>
 *   delete: func(bucket, key) -> result<_, error>
 *   exists: func(bucket, key) -> result<bool, error>
 *
 *   // wasi:keyvalue/eventual-batch@0.1.0
 *   get-many: func(bucket, keys) -> result<list<option<incoming-value>>, error>
 *   keys: func(bucket) -> result<list<key>, error>
 *   set-many: func(bucket, key-values) -> result<_, error>
 *   delete-many: func(bucket, keys) -> result<_, error>
 * }}}
 */
object KeyValue {

  // --- Native imports ---

  @js.native
  @JSImport("wasi:keyvalue/eventual@0.1.0", JSImport.Namespace)
  private object EventualModule extends js.Object {
    def get(bucket: js.Any, key: String): js.Any              = js.native
    def set(bucket: js.Any, key: String, value: js.Any): Unit = js.native
    def delete_(bucket: js.Any, key: String): Unit            = js.native
    def exists(bucket: js.Any, key: String): Boolean          = js.native
  }

  @js.native
  @JSImport("wasi:keyvalue/eventual-batch@0.1.0", JSImport.Namespace)
  private object EventualBatchModule extends js.Object {
    def getMany(bucket: js.Any, keys: js.Array[String]): js.Array[js.Any] = js.native
    def keys(bucket: js.Any): js.Array[String]                            = js.native
    def setMany(bucket: js.Any, keyValues: js.Array[js.Any]): Unit        = js.native
    def deleteMany(bucket: js.Any, keys: js.Array[String]): Unit          = js.native
  }

  @js.native
  @JSImport("wasi:keyvalue/types@0.1.0", "Bucket")
  private object KvBucketClass extends js.Object {
    def openBucket(name: String): js.Any = js.native
  }

  @js.native
  @JSImport("wasi:keyvalue/types@0.1.0", "OutgoingValue")
  private object KvOutgoingValueClass extends js.Object {
    def newOutgoingValue(): JsKvOutgoingValue = js.native
  }

  @js.native
  @JSImport("wasi:keyvalue/types@0.1.0", JSImport.Namespace)
  private object TypesModule extends js.Object

  @js.native
  @JSImport("wasi:keyvalue/wasi-keyvalue-error@0.1.0", JSImport.Namespace)
  private object ErrorModule extends js.Object

  // --- Bucket resource ---

  final class Bucket private[KeyValue] (private[golem] val underlying: js.Any) {

    def get(key: String): Option[Array[Byte]] = {
      val result = EventualModule.get(underlying, key)
      if (js.isUndefined(result) || result == null) None
      else {
        val iv = new IncomingValue(result.asInstanceOf[JsKvIncomingValue])
        Some(iv.consumeSync())
      }
    }

    def set(key: String, value: Array[Byte]): Unit = {
      val ov = OutgoingValue.create()
      ov.writeSync(value)
      EventualModule.set(underlying, key, ov.underlying)
    }

    def delete(key: String): Unit =
      EventualModule.delete_(underlying, key)

    def exists(key: String): Boolean =
      EventualModule.exists(underlying, key)

    def keys(): List[String] =
      EventualBatchModule.keys(underlying).toList

    def getMany(keys: List[String]): List[Option[Array[Byte]]] = {
      val arr = js.Array(keys: _*)
      EventualBatchModule.getMany(underlying, arr).toList.map { result =>
        if (js.isUndefined(result) || result == null) None
        else Some(new IncomingValue(result.asInstanceOf[JsKvIncomingValue]).consumeSync())
      }
    }

    def deleteMany(keys: List[String]): Unit =
      EventualBatchModule.deleteMany(underlying, js.Array(keys: _*))
  }

  object Bucket {
    def open(name: String): Bucket = {
      val raw = KvBucketClass.openBucket(name)
      new Bucket(raw)
    }
  }

  // --- OutgoingValue resource ---

  final class OutgoingValue private[KeyValue] (private[golem] val underlying: JsKvOutgoingValue) {

    def writeSync(data: Array[Byte]): Unit = {
      val jsArr = js.Array[Short]()
      data.foreach(b => jsArr.push((b.toInt & 0xff).toShort))
      val bytes = new Uint8Array(jsArr.asInstanceOf[js.Iterable[Short]])
      underlying.outgoingValueWriteBodySync(bytes)
    }
  }

  object OutgoingValue {
    def create(): OutgoingValue = {
      val raw = KvOutgoingValueClass.newOutgoingValue()
      new OutgoingValue(raw)
    }
  }

  // --- IncomingValue resource ---

  final class IncomingValue private[KeyValue] (private[golem] val underlying: JsKvIncomingValue) {

    def consumeSync(): Array[Byte] = {
      val arr   = underlying.incomingValueConsumeSync()
      val bytes = new Array[Byte](arr.length)
      var i     = 0
      while (i < arr.length) {
        bytes(i) = arr(i).toByte
        i += 1
      }
      bytes
    }

    def size(): Long = {
      val sizeBigInt = BigInt(underlying.incomingValueSize().toString)
      if (!sizeBigInt.isValidLong)
        throw new IllegalArgumentException(s"Incoming value size $sizeBigInt does not fit into a Long")
      sizeBigInt.toLong
    }
  }

  // --- Raw access (for forward compatibility) ---

  def eventualRaw: Any      = EventualModule
  def eventualBatchRaw: Any = EventualBatchModule
  def typesRaw: Any         = TypesModule
  def errorRaw: Any         = ErrorModule
}
