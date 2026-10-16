///|
#derive.golem_schema
pub(all) struct ArtifactRequest {
  source : String
  labels : Map[String, String]
}

///|
#warnings("-struct_never_constructed")
struct DigestText {}

///|
impl @schema.TextSpec for DigestText with type_id() {
  "gol40.DigestText"
}

///|
impl @schema.TextSpec for DigestText with languages() {
  []
}

///|
impl @schema.TextSpec for DigestText with min_length() {
  Some(8U)
}

///|
impl @schema.TextSpec for DigestText with max_length() {
  Some(64U)
}

///|
impl @schema.TextSpec for DigestText with regex() {
  Some("^[a-f0-9]+$")
}

///|
#derive.golem_schema
pub(all) struct ArtifactReport {
  artifactId : UInt64
  digest : @schema.Text[DigestText]
  labels : Map[String, String]
  warnings : Array[String]
}

///|
#derive.golem_schema
pub(all) struct ValidationFailure {
  field : String
  reason : String
  retryable : Bool
}

///|
#derive.golem_schema(inline=true)
pub(all) enum Region {
  #derive.case(name="eu-west-1")
  EuWest1
  #derive.case(name="us-east-1")
  UsEast1
}

///|
#derive.golem_schema(inline=true)
pub(all) enum Profile {
  #derive.case(name="debug")
  Debug
  #derive.case(name="release")
  Release
}

///|
#derive.golem_schema(inline=true)
pub(all) enum ReportFormat {
  #derive.case(name="json")
  Json
  #derive.case(name="text")
  Text
}

///|
#derive.golem_schema(inline=true)
pub(all) enum ColorMode {
  #derive.case(name="auto")
  Auto
  #derive.case(name="always")
  Always
  #derive.case(name="never")
  Never
}

///|
enum ArtifactError {
  /// Request validation failed
  #derive.error(kind="usage-error", exit_code="2")
  InvalidRequest(ValidationFailure)
  /// Renderer failed
  #derive.error(kind="runtime-error", exit_code="70")
  RenderFailed(stage~ : String, code~ : UInt)
}

///|
#derive.golem_schema(inline=true)
pub(all) enum ArtifactStatus {
  #derive.case(name="queued")
  Queued
  #derive.case(name="ready")
  Ready
  #derive.case(name="failed")
  Failed
}

///|
/// Build and inspect artifacts
/// A deliberately asymmetric conformance tool.
#derive.example("artifact --region eu-west-1 render src/main.wasm --format json", title="Render")
#derive.tool("artifact", version="1.0.0", alias="art")
struct Artifact {}

///|
/// Render one artifact
/// Build an artifact and return a structured report.
#derive.example("artifact render src/main.wasm --format json --tag release --define opt=3 --checksum", title="Release build")
#derive.command(name="render", alias="build", subtree="ArtifactRender")
#derive.arg("region", scope="root-global", short="r", alias="location", value_name="REGION", default="eu-west-1", env="ARTIFACT_REGION", doc="Execution region", description="Inherited by every executable descendant.")
#derive.arg("trace", scope="root-global", short="t", alias="diagnostics", kind="flag", negatable=true, default="false", doc="Emit trace details")
pub fn Artifact::render(region : Region, trace : Bool) -> Unit {
  ignore(region)
  ignore(trace)
}

///|
#derive.tool("artifact-render")
struct ArtifactRender {}

///|
/// Render one artifact
/// Build an artifact and return a structured report.
#derive.command(idempotent=true, destructive=false, read_only=false, open_world=false)
#derive.arg("profile", scope="global", short="p", value_name="PROFILE", default="release", doc="Build profile", description="Inherited by render descendants.")
#derive.arg("request", scope="positional", value_name="REQUEST", doc="Artifact request")
#derive.arg("inputs", scope="tail", value_name="INPUT", kind="file", direction="input", extension="wasm", extension="wat", min="1", max="3", separator="--", verbatim=true, doc="Input modules", description="One to three source modules.")
#derive.arg("format", scope="option", short="f", alias="output-format", value_name="FORMAT", default="json", doc="Report format")
#derive.arg("tag", scope="option", alias="label", value_name="TAG", repeatable="either", delim=",", doc="Tags", description="May be repeated or comma-delimited.")
#derive.arg("define", scope="option", short="D", value_name="KEY=VALUE", repeatable="repeated", doc="Numeric definitions", description="Duplicate keys are rejected.")
#derive.arg("color", scope="option", value_name="WHEN", optional_scalar=true, default="auto", doc="Color mode", description="Bare presence resolves to the default.")
#derive.arg("checksum", kind="flag", short="c", alias="digest", negatable=true, default="false", doc="Include digest")
#derive.arg("verbose", kind="count-flag", short="v", max="3", doc="Verbosity")
#derive.arg("stdin", mime="application/wasm", doc="Optional module bytes")
#derive.arg("stdout", mime="text/plain; charset=utf-8", doc="Progress output")
#derive.arg("stderr", channel="stderr", mime="application/octet-stream", doc="Diagnostic bytes")
#derive.constraint("requires_all", "checksum", "format")
#derive.constraint("implies", lhs_value_is="profile=release", rhs="tag", rhs_quant="any")
#derive.constraint("forbids", lhs_value_is="format=text", lhs_quant="any", rhs="define")
#derive.result("json", formatter="table", default="json", doc="Artifact report", formatter_doc="json=JSON report", formatter_doc="table=Tabular report")
pub async fn ArtifactRender::artifact_render(
  profile : Profile,
  request : ArtifactRequest,
  inputs : Array[@schema.Path],
  format : ReportFormat,
  tag : Array[String],
  define : Map[String, Int64],
  color : ColorMode?,
  checksum : Bool,
  verbose : UInt,
  stdin : @asyncCore.Stream[Byte]?,
  stdout : @tool.ProviderOutput,
  stderr : @tool.ProviderOutput?,
  principal : @tool.Principal,
) -> Result[ArtifactReport, ArtifactError] {
  ignore(profile)
  ignore(request)
  ignore(inputs)
  ignore(format)
  ignore(tag)
  ignore(define)
  ignore(color)
  ignore(checksum)
  ignore(verbose)
  ignore(stdin)
  ignore(stdout)
  ignore(stderr)
  ignore(principal)
  ...
}

///|
/// Inspect render status
#derive.command(alias="show", read_only=true, destructive=false, idempotent=true, open_world=false)
#derive.arg("artifactId", scope="positional", name="artifact-id", value_name="ID", doc="Artifact identifier")
#derive.result("json", default="json", doc="Current status", formatter_doc="json=JSON status")
pub fn ArtifactRender::status(artifactId : UInt64) -> ArtifactStatus {
  ignore(artifactId)
  Ready
}
