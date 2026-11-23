package dynamicimport

import scala.scalajs.js

// Links into a main module plus a separately loaded module, which a Golem component cannot ship.
object Main {
  def main(args: Array[String]): Unit = {
    val _ = js.dynamicImport(Lazy.answer)
  }
}

object Lazy {
  def answer: Int = 42
}
