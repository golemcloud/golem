///|
test "generated tool descriptors preserve HTTP declarations and graft override" {
  let report = __golem_tool_def_Report().to_tool()
  guard report.http_mount is Some(mount) else { fail("missing mount") }
  assert_true(mount.path_prefix is [@coreHttp.PathSegment::Literal("reports")])
  let nodes = report.commands.nodes
  assert_true(
    nodes[1].http
    is Some(
      @toolCommon.CommandHttp::Endpoint(
        { http_method: @coreHttp.HttpMethod::Post, mode: None, .. }
      )
    ),
  )
  assert_true(
    nodes[2].http
    is Some(
      @toolCommon.CommandHttp::Endpoint(
        { http_method: @coreHttp.HttpMethod::Get, .. }
      )
    ),
  )
  assert_true(
    nodes[3].http
    is Some(
      @toolCommon.CommandHttp::Prefix(
        { path_suffix: [@coreHttp.PathSegment::Literal("admin")], .. }
      )
    ),
  )
  assert_true(
    nodes[4].http
    is Some(
      @toolCommon.CommandHttp::Endpoint(
        { http_method: @coreHttp.HttpMethod::Delete, .. }
      )
    ),
  )
  let child = __golem_tool_def_Administration().to_tool()
  let retained = __golem_tool_def_Retained().to_tool()
  assert_true(
    child.http_mount
    is Some({ path_prefix: [@coreHttp.PathSegment::Literal("discarded")], .. }),
  )
  assert_true(
    child.commands.nodes[0].http
    is Some(
      @toolCommon.CommandHttp::Prefix(
        { path_suffix: [@coreHttp.PathSegment::Literal("own")], .. }
      )
    ),
  )
  assert_eq(retained.commands.nodes[1].http, child.commands.nodes[0].http)
  assert_true(
    retained.http_mount
    is Some({ path_prefix: [@coreHttp.PathSegment::Literal("retained")], .. }),
  )
  let options = __golem_tool_def_HttpOptions().to_tool()
  guard options.commands.nodes[1].http
    is Some(@toolCommon.CommandHttp::Endpoint(endpoint)) else {
    fail("missing options")
  }
  assert_eq(endpoint.route_id, Some("custom"))
  assert_true(endpoint.mode is Some(@coreHttp.HttpRouteMode::DURABLE_STREAMS))
  assert_true(
    endpoint.query_vars
    is [{ query_param_name: "format", variable_name: "format", }],
  )
  assert_true(
    endpoint.header_vars
    is [{ header_name: "X-Token", variable_name: "token", }],
  )
  assert_true(
    endpoint.response_headers is [{ name: "X-Result", value: "ready", }],
  )
  guard endpoint.durable_streams is Some(streams) else {
    fail("missing streams")
  }
  assert_true(
    streams.slots
    is [
      {
        source: @coreHttp.DurableStreamSlotSource::Input("stdin"),
        name: Some("upload"),
        content_type: Some("text/plain"),
      },
      {
        source: @coreHttp.DurableStreamSlotSource::Output("stdout"),
        name: Some("download"),
        content_type: Some("text/csv"),
      },
    ],
  )
  assert_true(
    streams.load
    is Some(
      {
        max_concurrent_readers_per_stream: Some(7),
        max_append_requests_per_second_per_stream: Some(11),
      }
    ),
  )
}
