package singlemodule

import scala.scalajs.js

// fastLinkJS keeps checked casts and array bounds checks here; fullLinkJS removes them.
object Main {
  def main(args: Array[String]): Unit = {
    val values: Array[Any] = Array(1, "two", 3.0)
    val strings            = values.collect { case s: String => s }
    val first              = values(0).asInstanceOf[Int]
    js.Dynamic.global.console.log(s"$first ${strings.mkString(",")} ${values.length}")
  }
}
