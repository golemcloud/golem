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

import zio.test._

object BlobstoreCompileSpec extends ZIOSpecDefault {
  import Blobstore._

  private val containerMeta = ContainerMetadata("test-container", BigInt(1700000000L))
  private val objectMeta    = ObjectMetadata("file.txt", "test-container", BigInt(1700000000L), 1024L)
  private val objectId1     = ObjectId("container1", "object1")
  private val objectId2     = ObjectId("container2", "object2")

  def spec = suite("BlobstoreCompileSpec")(
    test("ContainerMetadata construction and field access") {
      assertTrue(
        containerMeta.name == "test-container",
        containerMeta.createdAt == BigInt(1700000000L)
      )
    },
    test("ObjectMetadata construction and field access") {
      assertTrue(
        objectMeta.name == "file.txt",
        objectMeta.container == "test-container",
        objectMeta.createdAt == BigInt(1700000000L),
        objectMeta.size == 1024L
      )
    },
    test("ObjectId construction and field access") {
      assertTrue(
        objectId1.container == "container1",
        objectId1.name == "object1",
        objectId2.container == "container2"
      )
    }
  )
}
