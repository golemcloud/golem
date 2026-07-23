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

package golem.runtime.macros

/**
 * Turns raw Scaladoc comments (as returned by `Symbol.docstring`) into plain
 * description text usable in agent and tool metadata.
 *
 * The comment markers (`/**`, margin `*`, `*/`) are stripped, every line is
 * trimmed, and Scaladoc tag sections (`@param`, `@return`, ...) are dropped.
 * Paragraph structure (blank lines) is preserved.
 */
private[golem] object Scaladoc {

  /**
   * Cleans a raw Scaladoc comment into plain text. Returns `None` when the
   * comment carries no prose (empty, or tags only).
   */
  def clean(raw: String): Option[String] = {
    val body = {
      val noStart = raw.trim.stripPrefix("/**")
      noStart.stripSuffix("*/")
    }

    val lines = body.linesIterator.map(stripMargin).toList

    val prose = lines.takeWhile(line => !isTagLine(line))

    val trimmed = dropBlankEnds(prose)
    if (trimmed.isEmpty) None
    else Some(trimmed.mkString("\n"))
  }

  /**
   * Splits cleaned doc text into a summary (first paragraph, joined with
   * spaces) and a description (remaining paragraphs, newline-separated).
   */
  def summaryAndDescription(cleaned: String): (String, String) = {
    val lines           = cleaned.linesIterator.toList
    val (summary, rest) = lines.span(_.nonEmpty)
    val description     = dropBlankEnds(rest)
    (summary.mkString(" "), description.mkString("\n"))
  }

  private def stripMargin(line: String): String = {
    val t      = line.trim
    val noStar =
      if (t.startsWith("*")) t.drop(1)
      else t
    noStar.trim
  }

  private def isTagLine(line: String): Boolean =
    line.length >= 2 && line.startsWith("@") && line.charAt(1).isLetter

  private def dropBlankEnds(lines: List[String]): List[String] =
    lines.dropWhile(_.isEmpty).reverse.dropWhile(_.isEmpty).reverse
}
