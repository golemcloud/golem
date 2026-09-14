package golem.runtime.annotations

import scala.annotation.StaticAnnotation

final class toolDefinition(
  val name: String = "",
  val version: String = ""
) extends StaticAnnotation

final class toolMiddleware(
  val name: String = "",
  val aliases: Array[String] = Array.empty
) extends StaticAnnotation

final class universalToolMiddleware(
  val name: String = "",
  val aliases: Array[String] = Array.empty
) extends StaticAnnotation

final class internalToolMiddlewareField(
  val canonicalName: String,
  val countFlag: Boolean = false
) extends StaticAnnotation
