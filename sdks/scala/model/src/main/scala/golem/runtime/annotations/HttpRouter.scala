// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.annotations

import scala.annotation.StaticAnnotation

/**
 * A parameterless ephemeral agent with one literal mount and no snapshotting.
 */
final class httpRouter(
  val typeName: String,
  val mount: String,
  val staticBindings: Array[(String, String)] = Array.empty,
  val auth: Boolean = false,
  val cors: Array[String] = Array.empty
) extends StaticAnnotation

/** The router's sole Any-bound method: request: HttpRequest => HttpResponse. */
final class httpHandler extends StaticAnnotation

/**
 * A parameterless method returning OpenAPI 3.1.0 JSON text, optionally
 * asynchronously.
 */
final class openApiProvider extends StaticAnnotation
