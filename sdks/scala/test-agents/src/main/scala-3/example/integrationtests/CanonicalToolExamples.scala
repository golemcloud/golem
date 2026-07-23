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

package example.integrationtests

import golem.runtime.annotations.*
import golem.tool.{ToolInputStream, ToolOutputStream}
import zio.blocks.schema.Schema

type GolemPath = _root_.golem.schema.GolemPath
type Url       = _root_.golem.schema.Url
type Instant   = _root_.java.time.Instant

implicit val optionUrlIntoSchema: _root_.golem.schema.IntoSchema[Option[Url]] =
  new _root_.golem.schema.IntoSchema[Option[Url]] {
    private val inner: _root_.golem.schema.IntoSchema[Url] = _root_.golem.schema.IntoSchema[Url]

    override lazy val graph: _root_.golem.schema.SchemaGraph =
      inner.graph.copy(root = _root_.golem.schema.t.option(inner.graph.root))

    override def toValue(value: Option[Url]): _root_.golem.schema.SchemaValue =
      _root_.golem.schema.SchemaValue.OptionValue(value.map(inner.toValue))
  }

sealed trait GrepColorMode
case object always extends GrepColorMode
case object never  extends GrepColorMode
case object auto   extends GrepColorMode
object GrepColorMode {
  implicit val schema: Schema[GrepColorMode] = Schema.derived
}

final case class GrepHit(file: GolemPath, line: Int, text: String)
object GrepHit {
  implicit val schema: Schema[GrepHit] = Schema.derived
}

enum GrepFailure {
  @error(kind = "usage-error", exitCode = 2)
  case InvalidPattern(reason: String)

  @error(kind = "runtime-error", exitCode = 1)
  case NoMatch
}

/** Search files for a regex pattern. */
@toolDefinition(name = "grep", version = "2.0.0")
trait GrepTool {

  /** Search files for a regex pattern. Bare `grep` runs this body. */
  @arg("case-sensitive", scope = "global", short = 'i', kind = "flag", doc = "match case exactly")
  @arg("color", scope = "global", default = "auto", doc = "when to colorize matches")
  @arg("pattern", scope = "positional", regex = "^.+$", doc = "regular expression")
  @arg("extra-patterns", scope = "option", short = 'e', repeatable = "either", delim = ',')
  @arg("max-count", scope = "option", short = 'n', min = 1)
  @arg("files", scope = "tail", acceptsStdio = true, doc = "files to search")
  def grep(
    caseSensitive: Boolean,
    color: GrepColorMode,
    pattern: String,
    extraPatterns: Seq[String],
    maxCount: Option[Int],
    files: Seq[GolemPath],
    stdin: ToolInputStream,
    stdout: ToolOutputStream
  ): Either[GrepFailure, Seq[GrepHit]]

  /** In-place text replacement. */
  def replace(
    caseSensitive: Boolean,
    color: GrepColorMode,
    pattern: String,
    replacement: String,
    files: Seq[GolemPath]
  ): Either[GrepFailure, Long]
}

@toolImplementation()
final class GrepToolImpl extends GrepTool {
  def grep(
    caseSensitive: Boolean,
    color: GrepColorMode,
    pattern: String,
    extraPatterns: Seq[String],
    maxCount: Option[Int],
    files: Seq[GolemPath],
    stdin: ToolInputStream,
    stdout: ToolOutputStream
  ): Either[GrepFailure, Seq[GrepHit]] = Right(Seq.empty)

  def replace(
    caseSensitive: Boolean,
    color: GrepColorMode,
    pattern: String,
    replacement: String,
    files: Seq[GolemPath]
  ): Either[GrepFailure, Long] = Right(0L)
}

sealed trait GitOutputMode
case object human     extends GitOutputMode
case object porcelain extends GitOutputMode
case object json      extends GitOutputMode
object GitOutputMode {
  implicit val schema: Schema[GitOutputMode] = Schema.derived
}

final case class CommitResult(hash: String, filesChanged: Int, insertions: Int, deletions: Int)
object CommitResult {
  implicit val schema: Schema[CommitResult] = Schema.derived
}

final case class LogEntry(hash: String, author: String, date: Instant, message: String)
object LogEntry {
  implicit val schema: Schema[LogEntry] = Schema.derived
}

enum CommitFailure {
  @error(kind = "runtime-error", exitCode = 1)
  case NothingStaged

  @error(kind = "runtime-error", exitCode = 128)
  case DirtyMerge

  @error(kind = "usage-error", exitCode = 129)
  case BadAuthorFormat(author: String)
}

enum LogFailure {
  @error(kind = "usage-error", exitCode = 128)
  case BadRevision

  @error(kind = "usage-error", exitCode = 129)
  case NotARepository
}

enum RemoteFailure {
  @error(kind = "usage-error", exitCode = 128)
  case NoSuchRemote(name: String)
}

enum StashFailure {
  @error(kind = "usage-error", exitCode = 128)
  case NoSuchStash(name: String)
}

/** Stupid content tracker. */
@toolDefinition(name = "git")
trait GitTool {

  /** Record changes to the repository. */
  @command(aliases = Array("ci"))
  @annotations(destructive = true)
  @arg("verbose", scope = "global", short = 'v', kind = "count-flag", max = 3)
  @arg("git-dir", scope = "global", env = "GIT_DIR", default = ".git")
  @arg("paginate", scope = "global", kind = "flag", negatable = true, default = true)
  @arg("config", scope = "global", short = 'c', repeatable = "repeated")
  @arg("message", scope = "option", short = 'm', required = true, aliases = Array("msg"))
  @arg("author", scope = "option", env = "GIT_AUTHOR_NAME", regex = "^.+ <.+@.+>$")
  @arg("amend", kind = "flag", negatable = true, default = false)
  @arg("signoff", kind = "flag", negatable = true, default = false)
  @arg("reset-author", kind = "flag", default = false)
  @arg("output", scope = "option", default = "human")
  @constraint(implies = Implies(lhs = "reset-author", rhs = "amend"))
  @constraint(requiresAll = Array(ValueIs("output", "json")))
  @result(formatters = Array("human", "porcelain", "json"), default = "human")
  def commit(
    verbose: Int,
    gitDir: GolemPath,
    paginate: Boolean,
    config: Map[String, String],
    message: String,
    author: Option[String],
    amend: Boolean,
    signoff: Boolean,
    resetAuthor: Boolean,
    output: GitOutputMode
  ): Either[CommitFailure, CommitResult]

  /** Manage set of tracked repositories. Pure dispatcher. */
  @command(aliases = Array("rmt"))
  @arg("verbose", scope = "global", short = 'v', kind = "count-flag", max = 3)
  @arg("git-dir", scope = "global", env = "GIT_DIR", default = ".git")
  @arg("paginate", scope = "global", kind = "flag", negatable = true, default = true)
  @arg("config", scope = "global", short = 'c', repeatable = "repeated")
  def remote(verbose: Int, gitDir: GolemPath, paginate: Boolean, config: Map[String, String]): GitRemoteTool

  /** Stash the changes in a dirty working directory away. */
  @arg("verbose", scope = "global", short = 'v', kind = "count-flag", max = 3)
  @arg("git-dir", scope = "global", env = "GIT_DIR", default = ".git")
  def stash(verbose: Int, gitDir: GolemPath): GitStashTool

  /** Show commit logs. */
  @annotations(readOnly = true, idempotent = true)
  @arg("max-count", scope = "option", short = 'n', min = 0, max = "9223372036854775807")
  @arg("since", scope = "option")
  @arg("until", scope = "option")
  @arg("author", scope = "option", repeatable = "delimited", delim = ',')
  @arg("grep", scope = "option", repeatable = "either", delim = ',')
  @arg("all-match", kind = "flag")
  @arg("invert-grep", kind = "flag")
  @arg("oneline", kind = "flag")
  @arg("graph", kind = "flag")
  @arg("paths", scope = "tail", separator = "--", min = 0)
  @constraint(allOrNone = Array("all-match", "grep"))
  @result(formatters = Array("oneline", "short", "medium", "full"), default = "medium")
  def log(
    maxCount: Option[Long],
    since: Option[Instant],
    until: Option[Instant],
    author: Seq[String],
    grep: Seq[String],
    allMatch: Boolean,
    invertGrep: Boolean,
    oneline: Boolean,
    graph: Boolean,
    paths: Seq[GolemPath]
  ): Either[LogFailure, Seq[LogEntry]]
}

@toolImplementation()
final class GitToolImpl extends GitTool {
  def commit(
    verbose: Int,
    gitDir: GolemPath,
    paginate: Boolean,
    config: Map[String, String],
    message: String,
    author: Option[String],
    amend: Boolean,
    signoff: Boolean,
    resetAuthor: Boolean,
    output: GitOutputMode
  ): Either[CommitFailure, CommitResult] = Right(CommitResult("", 0, 0, 0))

  def remote(verbose: Int, gitDir: GolemPath, paginate: Boolean, config: Map[String, String]): GitRemoteTool =
    new GitRemoteToolImpl

  def stash(verbose: Int, gitDir: GolemPath): GitStashTool = new GitStashToolImpl

  def log(
    maxCount: Option[Long],
    since: Option[Instant],
    until: Option[Instant],
    author: Seq[String],
    grep: Seq[String],
    allMatch: Boolean,
    invertGrep: Boolean,
    oneline: Boolean,
    graph: Boolean,
    paths: Seq[GolemPath]
  ): Either[LogFailure, Seq[LogEntry]] = Right(Seq.empty)
}

@toolDefinition(name = "remote")
trait GitRemoteTool {

  /** Add a remote. */
  @annotations(destructive = false, idempotent = false)
  @arg("name", scope = "positional", regex = "^[a-zA-Z][a-zA-Z0-9_-]*$")
  @arg("url", scope = "positional")
  @arg("track", scope = "option", short = 't', repeatable = "repeated")
  @arg("master", scope = "option", short = 'm')
  @arg("tags", kind = "flag", negatable = true, default = true)
  @arg("fetch", kind = "flag", short = 'f', default = false)
  @arg("verbose", kind = "count-flag", max = 3)
  def add(
    verbose: Int,
    name: String,
    url: Url,
    track: Seq[String],
    master: Option[String],
    tags: Boolean,
    fetch: Boolean
  ): Either[RemoteFailure, Unit]

  /** Remove a remote. */
  @command(aliases = Array("rm"))
  @annotations(destructive = true, idempotent = true)
  @arg("name", scope = "positional", regex = "^[a-zA-Z][a-zA-Z0-9_-]*$")
  def remove(name: String): Either[RemoteFailure, Unit]

  /** Change a remote URL. */
  @annotations(destructive = true)
  @arg("name", scope = "positional")
  @arg("newurl", scope = "positional")
  @arg("oldurl", scope = "positional", required = false)
  @arg("push", kind = "flag")
  @arg("add", kind = "flag")
  @arg("delete", kind = "flag")
  @constraint(mutexGroups = Array(Array[Any]("add"), Array[Any]("delete")))
  def setUrl(
    name: String,
    newurl: Url,
    oldurl: Option[Url],
    push: Boolean,
    add: Boolean,
    delete: Boolean
  ): Either[RemoteFailure, Unit]
}

final class GitRemoteToolImpl extends GitRemoteTool {
  def add(
    verbose: Int,
    name: String,
    url: Url,
    track: Seq[String],
    master: Option[String],
    tags: Boolean,
    fetch: Boolean
  ): Either[RemoteFailure, Unit] = Right(())

  def remove(name: String): Either[RemoteFailure, Unit] = Right(())

  def setUrl(
    name: String,
    newurl: Url,
    oldurl: Option[Url],
    push: Boolean,
    add: Boolean,
    delete: Boolean
  ): Either[RemoteFailure, Unit] = Right(())
}

@toolDefinition(name = "stash")
trait GitStashTool {

  /** Stash the changes in a dirty working directory away. */
  @arg("message", scope = "option", short = 'm', required = true)
  @arg("keep-index", kind = "flag", short = 'k', default = false)
  @arg("verbose", kind = "count-flag", max = 3)
  def stash(message: String, keepIndex: Boolean, verbose: Int): Either[StashFailure, Unit]

  /** Remove and apply a single stashed state. */
  @arg("name", scope = "positional", required = false)
  @arg("index", scope = "option", short = 'i')
  def pop(name: Option[String], index: Option[Int]): Either[StashFailure, Unit]

  /** Apply a single stashed state without removing it. */
  @arg("name", scope = "positional", required = false)
  @arg("index", scope = "option", short = 'i')
  def apply(name: Option[String], index: Option[Int]): Either[StashFailure, Unit]
}

final class GitStashToolImpl extends GitStashTool {
  def stash(message: String, keepIndex: Boolean, verbose: Int): Either[StashFailure, Unit] = Right(())
  def pop(name: Option[String], index: Option[Int]): Either[StashFailure, Unit]            = Right(())
  def apply(name: Option[String], index: Option[Int]): Either[StashFailure, Unit]          = Right(())
}
