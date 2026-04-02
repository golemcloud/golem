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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.typedarray.Uint8Array

// ---------------------------------------------------------------------------
// wasi:blobstore  –  JS facade traits
// ---------------------------------------------------------------------------

// --- ContainerMetadata ---

@js.native
sealed trait JsContainerMetadata extends js.Object {
  def name: String         = js.native
  def createdAt: js.BigInt = js.native
}

// --- ObjectMetadata ---

@js.native
sealed trait JsObjectMetadata extends js.Object {
  def name: String         = js.native
  def container: String    = js.native
  def createdAt: js.BigInt = js.native
  def size: js.BigInt      = js.native
}

// --- ObjectId ---

@js.native
sealed trait JsObjectId extends js.Object {
  def container: String = js.native
  @js.annotation.JSName("object")
  def objectName: String = js.native
}

object JsObjectId {
  def apply(container: String, objectName: String): JsObjectId =
    js.Dynamic.literal("container" -> container, "object" -> objectName).asInstanceOf[JsObjectId]
}

// --- Resources (Container, StreamObjectNames, OutgoingValue, IncomingValue) ---

@js.native
sealed trait JsBlobstoreContainer extends js.Object {
  def name(): String                                                                    = js.native
  def info(): JsContainerMetadata                                                       = js.native
  def getData(name: String, start: js.BigInt, end: js.BigInt): JsBlobstoreIncomingValue = js.native
  def writeData(name: String, data: JsBlobstoreOutgoingValue): Unit                     = js.native
  def listObjects(): JsStreamObjectNames                                                = js.native
  def deleteObject(name: String): Unit                                                  = js.native
  def deleteObjects(names: js.Array[String]): Unit                                      = js.native
  def hasObject(name: String): Boolean                                                  = js.native
  def objectInfo(name: String): JsObjectMetadata                                        = js.native
  def clear(): Unit                                                                     = js.native
}

@js.native
sealed trait JsStreamObjectNames extends js.Object {
  def readStreamObjectNames(len: js.BigInt): js.Tuple2[js.Array[String], Boolean] = js.native
  def skipStreamObjectNames(num: js.BigInt): js.Tuple2[js.BigInt, Boolean]        = js.native
}

@js.native
sealed trait JsBlobstoreOutgoingValue extends js.Object {
  def outgoingValueWriteBody(): JsBlobstoreOutputStream = js.native
}

@js.native
sealed trait JsBlobstoreIncomingValue extends js.Object {
  def incomingValueConsumeSync(): Uint8Array = js.native
  def size(): js.BigInt                      = js.native
}

@js.native
sealed trait JsBlobstoreOutputStream extends js.Object {
  def blockingWriteAndFlush(data: Uint8Array): Unit = js.native
}
