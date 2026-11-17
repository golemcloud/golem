/*
 * Copyright 2024-2026 Golem Cloud
 * Licensed under the Golem Source License v1.1 (the "License");
 */
package golem.host.js.schema

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

@js.native
@JSImport("golem:core/types@2.0.0", "SchemaValueStream")
private[golem] object JsSchemaValueStreamUnwrap extends js.Object {
  def unwrap(value: JsSchemaValueStream): js.Promise[js.Tuple2[JsSchemaValueIterable, js.Promise[JsStreamOutcome]]] =
    js.native
}

@js.native
@JSImport("golem:core/types@2.0.0", "SchemaValueStream")
private[golem] object JsSchemaValueStreamWrap extends js.Object {
  def wrap(
    value: JsSchemaValueIterable,
    outcome: js.Promise[js.UndefOr[JsStreamOutcome]]
  ): js.Promise[JsSchemaValueStream] = js.native
}

@js.native
private[golem] trait JsStreamOutcome extends js.Object {
  def tag: String = js.native
}
