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

package golem.schema.validation

import golem.schema._
import golem.schema.SchemaTypeBody._

sealed trait RefResolutionError extends Product with Serializable {
  def message: String; override def toString: String = message
}
object RefResolutionError {
  final case class DanglingRef(id: String) extends RefResolutionError {
    def message: String = s"dangling type reference `$id`"
  }
  final case class RecursiveRef(id: String) extends RefResolutionError {
    def message: String = s"recursive type reference `$id`"
  }
}

object RefResolution {
  def resolveRef(graph: SchemaGraph, tpe: SchemaType): Either[RefResolutionError, SchemaType] = {
    var visiting = List.empty[String]
    var current  = tpe
    while (current.body.isInstanceOf[RefType]) {
      val id = current.body.asInstanceOf[RefType].id
      if (visiting.contains(id)) return Left(RefResolutionError.RecursiveRef(id))
      graph.defs.get(id) match {
        case Some(defn) => visiting = id :: visiting; current = defn.body
        case None       => return Left(RefResolutionError.DanglingRef(id))
      }
    }
    Right(current)
  }
}
