package component_name.exports.__pack__.__name___exports

import scala.scalajs.js
import scala.scalajs.js.annotation.*
import scala.scalajs.js.JSConverters._
import component_name.bindings.wit.*

object GlobalState {
  var value: Long = 0
}

@JSExportTopLevel("componentNameApi")
object ComponentNameApi extends component_name.bindings.exports.__pack__.__name___exports.component_name_api.ComponentNameApi {
  @JSExport("add")
  override def add(value: Long): Unit = {
    GlobalState.value += value
  }

  @JSExport("get")
  override def get(): Long = {
    GlobalState.value
  }
}
