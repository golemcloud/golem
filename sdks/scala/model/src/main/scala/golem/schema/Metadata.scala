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

package golem.schema

/**
 * Open registry of consumer-facing role annotations. Unknown roles fall back to
 * structural handling. Mirrors `golem:core/types@2.0.0` `role`.
 */
sealed trait Role extends Product with Serializable

object Role {
  case object Multimodal               extends Role
  final case class Other(name: String) extends Role
}

/**
 * Typed metadata envelope. Holds non-validation, non-rendering-critical
 * information (docs, aliases, examples, deprecation, role). Per-scalar
 * validation constraints live on the relevant scalar's typed substructure, not
 * here. Mirrors `golem:core/types@2.0.0` `metadata-envelope`.
 *
 * `examples` are canonical-encoded JSON strings so metadata is self-contained
 * on the type side and does not have to cross-reference an accompanying value
 * tree.
 */
final case class MetadataEnvelope(
  doc: Option[String] = None,
  aliases: List[String] = Nil,
  examples: List[String] = Nil,
  deprecated: Option[String] = None,
  role: Option[Role] = None
)

object MetadataEnvelope {

  /** The canonical empty metadata envelope (no docs/aliases/examples/role). */
  val empty: MetadataEnvelope = MetadataEnvelope()
}
