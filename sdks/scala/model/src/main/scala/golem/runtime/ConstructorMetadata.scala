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

package golem.runtime

/**
 * Metadata for an agent's constructor: identity fields plus the schema-native
 * description of its parameters ([[InputMetadata]]).
 */
final case class ConstructorMetadata(
  name: Option[String],
  description: String,
  promptHint: Option[String],
  input: InputMetadata = InputMetadata.empty
)
