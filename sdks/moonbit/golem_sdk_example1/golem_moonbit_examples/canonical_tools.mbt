///|
#derive.golem_schema
pub(all) enum CanonicalGrepColorMode {
  Always
  Never
  Auto
} derive(Eq)

///|
#derive.golem_schema
pub(all) struct CanonicalGrepHit {
  file : @schema.Path
  line : UInt
  text : String
}

///|
enum CanonicalGrepError {
  /// The supplied pattern is not a valid regular expression.
  #derive.error(kind="usage-error", exit_code="2")
  InvalidPattern(reason~ : String)
  /// No line matched.
  #derive.error(kind="runtime-error", exit_code="1")
  NoMatch
}

///|
/// Search files for a regular expression.
#derive.tool("canonical-grep", version="2.0.0")
struct CanonicalGrep {}

///|
/// Search files for a regular expression. Bare `canonical-grep` runs this body.
#derive.arg("case_sensitive", name="case-sensitive", scope="global", short="i", kind="flag", doc="match case exactly")
#derive.arg("color", scope="global", default="Auto", doc="when to colorize matches")
#derive.arg("pattern", scope="positional", regex="^.+$", doc="regular expression")
#derive.arg("extra_patterns", name="extra-patterns", scope="option", short="e", repeatable="either", delim=",")
#derive.arg("max_count", name="max-count", scope="option", short="n", min="1")
#derive.arg("files", scope="tail", kind="file", direction="input", accepts_stdio=true, doc="files to search")
pub fn CanonicalGrep::canonical_grep(
  case_sensitive : Bool,
  color : CanonicalGrepColorMode,
  pattern : String,
  extra_patterns : Array[String],
  max_count : UInt?,
  files : Array[@schema.Path],
  stdin : @streams.InputStream,
  stdout : @streams.OutputStream,
) -> Result[Array[CanonicalGrepHit], CanonicalGrepError] {
  ignore(case_sensitive)
  ignore(color)
  ignore(pattern)
  ignore(extra_patterns)
  ignore(max_count)
  ignore(files)
  ignore(stdin)
  ignore(stdout)
  Ok([])
}

///|
/// Replace matching text in place.
#derive.arg("case_sensitive", name="case-sensitive", scope="global", short="i", kind="flag")
#derive.arg("color", scope="global", default="Auto")
#derive.arg("pattern", scope="positional")
#derive.arg("replacement", scope="positional")
#derive.arg("files", scope="tail", kind="file", direction="inout")
pub fn CanonicalGrep::replace(
  case_sensitive : Bool,
  color : CanonicalGrepColorMode,
  pattern : String,
  replacement : String,
  files : Array[@schema.Path],
) -> Result[UInt64, CanonicalGrepError] {
  ignore(case_sensitive)
  ignore(color)
  ignore(pattern)
  ignore(replacement)
  ignore(files)
  Ok(0UL)
}

///|
#derive.golem_schema
pub(all) enum CanonicalGitOutputMode {
  Human
  Porcelain
  Json
} derive(Eq)

///|
#derive.golem_schema
pub(all) struct CanonicalCommitResult {
  hash : String
  files_changed : UInt
  insertions : UInt
  deletions : UInt
}

///|
#derive.golem_schema
pub(all) struct CanonicalLogEntry {
  hash : String
  author : String
  date : @types.Datetime
  message : String
}

///|
enum CanonicalCommitError {
  #derive.error(kind="runtime-error", exit_code="1")
  NothingStaged
  #derive.error(kind="runtime-error", exit_code="128")
  DirtyMerge
  #derive.error(kind="usage-error", exit_code="129")
  BadAuthorFormat(author~ : String)
}

///|
enum CanonicalLogError {
  #derive.error(kind="usage-error", exit_code="128")
  BadRevision
  #derive.error(kind="usage-error", exit_code="129")
  NotARepository
}

///|
enum CanonicalRemoteError {
  #derive.error(kind="usage-error", exit_code="128")
  NoSuchRemote(name~ : String)
}

///|
enum CanonicalSetUrlError {
  #derive.error(kind="runtime-error", exit_code="1")
  Failed(String)
}

///|
enum CanonicalStashError {
  #derive.error(kind="usage-error", exit_code="128")
  NoSuchStash(name~ : String)
}

///|
/// Stupid content tracker.
#derive.tool("canonical-git")
struct CanonicalGit {}

///|
/// Record changes to the repository.
#derive.command(alias="ci", destructive=true)
#derive.arg("verbose", scope="global", short="v", kind="count-flag", max="3")
#derive.arg("git_dir", name="git-dir", scope="global", kind="dir", env="GIT_DIR", default=".git")
#derive.arg("paginate", scope="global", kind="flag", negatable=true, default="true")
#derive.arg("config", scope="global", short="c", repeatable="repeated")
#derive.arg("message", scope="option", short="m", required=true, alias="msg")
#derive.arg("author", scope="option", env="GIT_AUTHOR_NAME", regex="^.+ <.+@.+>$")
#derive.arg("amend", kind="flag", negatable=true, default="false")
#derive.arg("signoff", kind="flag", negatable=true, default="false")
#derive.arg("reset_author", name="reset-author", kind="flag", default="false")
#derive.arg("output", scope="option", default="Human")
#derive.constraint("implies", lhs="reset-author", rhs="amend")
#derive.constraint("requires_all", value_is="output=Json")
#derive.result("human", formatter="porcelain", formatter="json", default="human")
pub fn CanonicalGit::commit(
  verbose : UInt,
  git_dir : @schema.Path,
  paginate : Bool,
  config : Map[String, String],
  message : String,
  author : String?,
  amend : Bool,
  signoff : Bool,
  reset_author : Bool,
  output : CanonicalGitOutputMode,
) -> Result[CanonicalCommitResult, CanonicalCommitError] {
  ignore(verbose)
  ignore(git_dir)
  ignore(paginate)
  ignore(config)
  ignore(message)
  ignore(author)
  ignore(amend)
  ignore(signoff)
  ignore(reset_author)
  ignore(output)
  Ok({ hash: "", files_changed: 0, insertions: 0, deletions: 0 })
}

///|
/// Manage the set of tracked repositories.
#derive.command(alias="rmt", subtree="CanonicalRemote")
#derive.arg("verbose", scope="global", short="v", kind="count-flag", max="3")
#derive.arg("git_dir", name="git-dir", scope="global", kind="dir", env="GIT_DIR", default=".git")
#derive.arg("paginate", scope="global", kind="flag", negatable=true, default="true")
#derive.arg("config", scope="global", short="c", repeatable="repeated")
pub fn CanonicalGit::remote(
  verbose : UInt,
  git_dir : @schema.Path,
  paginate : Bool,
  config : Map[String, String],
) -> Unit {
  ignore(verbose)
  ignore(git_dir)
  ignore(paginate)
  ignore(config)
}

///|
/// Stash changes in a dirty working directory.
#derive.command(subtree="CanonicalStash")
#derive.arg("verbose", scope="global", short="v", kind="count-flag", max="3")
#derive.arg("git_dir", name="git-dir", scope="global", kind="dir", env="GIT_DIR", default=".git")
pub fn CanonicalGit::stash(verbose : UInt, git_dir : @schema.Path) -> Unit {
  ignore(verbose)
  ignore(git_dir)
}

///|
/// Show commit logs.
#derive.command(read_only=true, idempotent=true)
#derive.arg("max_count", name="max-count", scope="option", short="n", min="0", max="9223372036854775807")
#derive.arg("since", scope="option")
#derive.arg("until", scope="option")
#derive.arg("author", scope="option", repeatable="delimited", delim=",")
#derive.arg("grep", scope="option", repeatable="either", delim=",")
#derive.arg("all_match", name="all-match", kind="flag")
#derive.arg("invert_grep", name="invert-grep", kind="flag")
#derive.arg("oneline", kind="flag")
#derive.arg("graph", kind="flag")
#derive.arg("paths", scope="tail", kind="file", direction="input", separator="--", min="0")
#derive.constraint("all_or_none", "all-match", "grep")
#derive.result("oneline", formatter="short", formatter="medium", formatter="full", default="medium")
pub fn CanonicalGit::log(
  max_count : Int64?,
  since : @types.Datetime?,
  until : @types.Datetime?,
  author : Array[String],
  grep : Array[String],
  all_match : Bool,
  invert_grep : Bool,
  oneline : Bool,
  graph : Bool,
  paths : Array[@schema.Path],
) -> Result[Array[CanonicalLogEntry], CanonicalLogError] {
  ignore(max_count)
  ignore(since)
  ignore(until)
  ignore(author)
  ignore(grep)
  ignore(all_match)
  ignore(invert_grep)
  ignore(oneline)
  ignore(graph)
  ignore(paths)
  Ok([])
}

///|
/// Subcommands under `canonical-git remote`.
#derive.tool("remote")
struct CanonicalRemote {}

///|
/// Add a remote.
#derive.command(destructive=false, idempotent=false)
#derive.arg("verbose", scope="global", kind="count-flag", max="3")
#derive.arg("name", scope="positional", regex="^[a-zA-Z][a-zA-Z0-9_-]*$")
#derive.arg("url", scope="positional", scheme="https")
#derive.arg("track", scope="option", short="t", repeatable="repeated")
#derive.arg("master", scope="option", short="m")
#derive.arg("tags", kind="flag", negatable=true, default="true")
#derive.arg("fetch", kind="flag", short="f", default="false")
pub fn CanonicalRemote::add(
  verbose : UInt,
  name : String,
  url : @schema.Url,
  track : Array[String],
  master : String?,
  tags : Bool,
  fetch : Bool,
) -> Result[Unit, CanonicalRemoteError] {
  ignore(verbose)
  ignore(name)
  ignore(url)
  ignore(track)
  ignore(master)
  ignore(tags)
  ignore(fetch)
  Ok(())
}

///|
/// Remove a remote.
#derive.command(alias="rm", destructive=true, idempotent=true)
#derive.arg("name", scope="positional", regex="^[a-zA-Z][a-zA-Z0-9_-]*$")
pub fn CanonicalRemote::remove(
  name : String,
) -> Result[Unit, CanonicalRemoteError] {
  ignore(name)
  Ok(())
}

///|
/// Change a remote URL.
#derive.command(destructive=true)
#derive.arg("name", scope="positional")
#derive.arg("newurl", scope="positional", scheme="https")
#derive.arg("oldurl", scope="positional", required=false, scheme="https")
#derive.arg("push", kind="flag")
#derive.arg("add", kind="flag")
#derive.arg("delete", kind="flag")
#derive.constraint("mutex_groups", "add", "delete")
pub fn CanonicalRemote::set_url(
  name : String,
  newurl : @schema.Url,
  oldurl : @schema.Url?,
  push : Bool,
  add : Bool,
  delete : Bool,
) -> Result[Unit, CanonicalSetUrlError] {
  ignore(name)
  ignore(newurl)
  ignore(oldurl)
  ignore(push)
  ignore(add)
  ignore(delete)
  Ok(())
}

///|
/// Subcommands under `canonical-git stash`, including an implicit root body.
#derive.tool("stash")
struct CanonicalStash {}

///|
/// Stash changes in a dirty working directory.
#derive.arg("verbose", scope="global", kind="count-flag", max="3")
#derive.arg("message", scope="option", short="m", required=true)
#derive.arg("keep_index", name="keep-index", kind="flag", short="k", default="false")
pub fn CanonicalStash::stash(
  verbose : UInt,
  message : String,
  keep_index : Bool,
) -> Result[Unit, CanonicalStashError] {
  ignore(verbose)
  ignore(message)
  ignore(keep_index)
  Ok(())
}

///|
/// Remove and apply a stashed state.
#derive.arg("name", scope="positional", required=false)
#derive.arg("index", scope="option", short="i")
pub fn CanonicalStash::pop(
  name : String?,
  index : UInt?,
) -> Result[Unit, CanonicalStashError] {
  ignore(name)
  ignore(index)
  Ok(())
}

///|
/// Apply a stashed state without removing it.
#derive.arg("name", scope="positional", required=false)
#derive.arg("index", scope="option", short="i")
pub fn CanonicalStash::apply(
  name : String?,
  index : UInt?,
) -> Result[Unit, CanonicalStashError] {
  ignore(name)
  ignore(index)
  Ok(())
}

///|
/// Numeric-bound parity fixture.
#derive.tool("canonical-big-bound")
struct CanonicalBigBound {}

///|
#derive.arg("count", scope="option", min="0", max="18446744073709551615")
pub fn CanonicalBigBound::canonical_big_bound(
  count : UInt64?,
) -> Result[Unit, CanonicalGrepError] {
  ignore(count)
  Ok(())
}
