///|
test "shared mapping corpus compiles ordered structural metadata" {
  let corpus = @json.parse(
    @fs.read_file_to_string(
      "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json",
    ),
  )
  let cases = corpus.value("cases").unwrap().as_array().unwrap()
  let mut count = 0
  for case in cases {
    if case.value("suite") != Some("mapping") {
      continue
    }
    count += 1
    let id = case.value("id").unwrap().as_string().unwrap()
    let pairs = case
      .value("input")
      .unwrap()
      .value("mappings")
      .unwrap()
      .as_array()
      .unwrap()
      .map(p => {
        let p = p.as_array().unwrap()
        (p[0].as_string().unwrap(), p[1].as_string().unwrap())
      })
    let expected = case.value("expect").unwrap()
    try @lib.compile_http_file_mappings(pairs) catch {
      @lib.AgentParseError(category) =>
        assert_eq(Some(category.to_json()), expected.value("error"), msg=id)
    } noraise {
      actual => {
        let projection = actual
          .map(m => {
            match m {
              @lib.Exact(path, target) =>
                (
                  {
                    "Exact": {
                      "public_path": path.to_json(),
                      "file_path": target.to_json(),
                    },
                  } : Json)
              @lib.Subtree(path, target) =>
                (
                  {
                    "Subtree": {
                      "public_prefix": path.to_json(),
                      "filesystem_root": target.to_json(),
                    },
                  } : Json)
            }
          })
          .to_json()
        assert_eq(Some(projection), expected.value("compiled"), msg=id)
      }
    }
  }
  assert_eq(count, 11)
}

///|
fn router_source(annotations : String, methods : String) -> String {
  "#derive.http_router\n#derive.mount(\"/site\")\n" +
  annotations +
  "\nstruct Site {}\nfn Site::new() -> Site { {} }\n" +
  methods
}

///|
test "router roles emit ordinary metadata and omit ordinary clients" {
  let handler =
    #|#derive.http_handler
    #|pub fn Site::serve(self : Self, request : @http.HttpRequest) -> @http.HttpResponse { ... }
  let provider =
    #|#derive.openapi_provider
    #|pub fn Site::document(self : Self) -> String { ... }
  for methods in ["", handler, provider, handler + "\n" + provider] {
    let agents = @lib.parse_agents([
      (
        "router.mbt",
        router_source(
          "#derive.static_file(\"/assets/*\", \"/one/$1\")\n#derive.static_file(\"/assets/*\", \"/two/$1\")",
          methods,
        ),
      ),
    ])
    assert_true(agents[0].is_http_router())
    assert_eq(agents[0].mode, @lib.Ephemeral)
    assert_eq(agents[0].snapshotting, @lib.Disabled)
    let output = @formatter.impls_to_string(
      @lib.generate_agent_registration(agents),
    )
    assert_true(output.contains("AgentTypeKind::HTTP_ROUTER"))
    assert_true(output.contains("filesystem_root: \"/one\""))
    assert_true(output.contains("filesystem_root: \"/two\""))
    assert_eq(
      output.contains("HttpMethod::Any"),
      methods.contains("http_handler"),
    )
    assert_eq(
      output.contains("openapi_provider_method: Some(\"document\")"),
      methods.contains("openapi_provider"),
    )
    assert_eq(@formatter.impls_to_string(@lib.generate_client_code(agents)), "")
  }
}

///|
test "router declarations reject invalid ownership roles and paths" {
  let sources = [
    router_source("#derive.agent", ""),
    router_source("#derive.http_router(\"extra\")", ""),
    router_source("#derive.mount(\"/other\")", ""),
    router_source("#derive.expose_files(\"/*\", \"/live/$1\")", ""),
    router_source("", "pub fn Site::extra(self : Self) -> String { ... }"),
    router_source(
      "", "#derive.http_handler\nfn Site::serve(self : Self, request : @http.HttpRequest) -> @http.HttpResponse { ... }",
    ),
    router_source(
      "", "#derive.openapi_provider\npub fn Site::document() -> String { ... }",
    ),
    router_source(
      "", "#derive.openapi_provider(\"x\")\npub fn Site::document(self : Self) -> String { ... }",
    ),
    router_source(
      "", "#derive.openapi_provider\n#derive.openapi_provider\npub fn Site::document(self : Self) -> String { ... }",
    ),
    router_source(
      "", "#derive.http_handler\n#derive.endpoint_auth(false)\npub fn Site::serve(self : Self, request : @http.HttpRequest) -> @http.HttpResponse { ... }",
    ),
    router_source(
      "", "#derive.http_handler\n#derive.openapi_provider\npub fn Site::serve(self : Self, request : @http.HttpRequest) -> @http.HttpResponse { ... }",
    ),
    "#derive.http_router\n#derive.mount(\"/site/{id}\")\nstruct Site {}\nfn Site::new(id : String) -> Site { {} }",
    "#derive.http_router\n#derive.mount(\"/site//x\")\nstruct Site {}\nfn Site::new() -> Site { {} }",
    "#derive.agent(\"ephemeral\")\n#derive.mount(\"/site\")\n#derive.expose_files(\"/x\", \"/file\")\nstruct Site {}\nfn Site::new() -> Site { {} }",
    "#derive.agent\n#derive.mount(\"/site/{id}/{id}\")\n#derive.expose_files(\"/x\", \"/file\")\nstruct Site {}\nfn Site::new(id : String) -> Site { {} }",
  ]
  for source in sources {
    try @lib.parse_agents([("invalid.mbt", source)]) catch {
      @lib.AgentParseError(_) => ()
    } noraise {
      _ => fail("invalid router/file declaration accepted: " + source)
    }
  }
}

///|
test "regular file owners retain typed endpoints clients and mapping order" {
  let source =
    #|#derive.agent
    #|#derive.mount("/files/{name}")
    #|#derive.expose_files("/*", "/one/$1")
    #|#derive.expose_files("/special", "/two")
    #|struct Files { name : String }
    #|fn Files::new(name : String) -> Files { { name } }
    #|#derive.endpoint(get="/special")
    #|pub fn Files::special(self : Self) -> String { self.name }
  let agents = @lib.parse_agents([("files.mbt", source)])
  assert_false(agents[0].is_http_router())
  assert_eq(agents[0].http_mount.unwrap().filesystem_bindings, [
    @lib.Subtree([], "/one"),
    @lib.Exact(["special"], "/two"),
  ])
  assert_true(
    @formatter.impls_to_string(@lib.generate_client_code(agents)).contains(
      "FilesClient",
    ),
  )
}

///|
test "root live mount and snapshot disabled JSON router" {
  let agents = @lib.parse_agents([
    (
      "root.mbt", "#derive.agent\n#derive.mount(\"/\")\n#derive.expose_files(\"/*\", \"/$1\")\nstruct Files {}\nfn Files::new() -> Files { {} }",
    ),
  ])
  assert_eq(agents[0].http_mount.unwrap().filesystem_bindings, [
    @lib.Subtree([], "/"),
  ])
  let agents = @lib.parse_agents([
    (
      "router.mbt", "#derive.http_router\n#derive.mount(\"/\")\nstruct Site {} derive(ToJson, FromJson)\nfn Site::new() -> Site { {} }",
    ),
  ])
  let output = @formatter.impls_to_string(
    @lib.generate_agent_registration(agents),
  )
  assert_false(output.contains("impl @agents.Snapshottable"))
  assert_true(output.contains("load_snapshot: None"))
}

///|
fn corpus_field(value : Json, name : String) -> Json {
  match value {
    Object(fields) => fields.get(name).unwrap_or(null)
    _ => null
  }
}

///|
fn corpus_array(value : Json) -> Array[Json] {
  match value {
    Array(items) => items
    _ => []
  }
}

///|
fn corpus_string(value : Json) -> String {
  match value {
    String(text) => text
    _ => abort("expected corpus string")
  }
}

///|
fn corpus_type(value : Json) -> String {
  match corpus_string(value) {
    "string" => "String"
    "HttpRequest" => "@http.HttpRequest"
    "HttpResponse" => "@http.HttpResponse"
    other => other
  }
}

///|
test "shared metadata corpus exercises parser emitter and route validation" {
  let corpus = @json.parse(
    @fs.read_file_to_string(
      "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json",
    ),
  )
  let mut count = 0
  for case in corpus_array(corpus_field(corpus, "cases")) {
    if corpus_field(case, "suite") != "metadata" {
      continue
    }
    let id = corpus_string(corpus_field(case, "id"))
    // These require resolved schema graphs, which component extraction validates.
    if [
        "metadata-buffered-handler", "metadata-streaming-provider", "metadata-nominal-schema-not-enough",
      ].contains(id) {
      continue
    }
    count += 1
    let input = corpus_field(case, "input")
    let expected = corpus_field(case, "expect")
    let router = corpus_field(input, "kind") == "http-router"
    let source = StringBuilder()
    source.write_string(
      if router {
        "#derive.http_router\n"
      } else {
        "#derive.agent\n"
      },
    )
    for mount in corpus_array(corpus_field(input, "mounts")) {
      source.write_string("#derive.mount(\{mount.stringify()})\n")
    }
    if corpus_field(input, "phantom") == true {
      source.write_string("#derive.mount_phantom(true)\n")
    }
    for
      binding in [
        ("static_bindings", "static_file"),
        ("filesystem_bindings", "expose_files"),
      ] {
      let (field, annotation) = binding
      for mapping in corpus_array(corpus_field(input, field)) {
        let pair = corpus_array(mapping)
        source.write_string(
          "#derive.\{annotation}(\{pair[0].stringify()}, \{pair[1].stringify()})\n",
        )
      }
    }
    source.write_string("struct Site {}\nfn Site::new(")
    let params = corpus_array(corpus_field(input, "constructor")).map(p => {
      let pair = corpus_array(p)
      corpus_string(pair[0]) + " : " + corpus_type(pair[1])
    })
    source.write_string(params.join(", ") + ") -> Site { Site::{} }\n")
    for method in corpus_array(corpus_field(input, "methods")) {
      let name = corpus_string(corpus_field(method, "name"))
      if corpus_field(input, "provider") == name.to_json() {
        source.write_string("#derive.openapi_provider\n")
      }
      for binding in corpus_array(corpus_field(method, "bindings")) {
        let pair = corpus_array(binding)
        if pair[0] == "Any" {
          source.write_string("#derive.http_handler\n")
        } else {
          source.write_string("#derive.endpoint(get=\{pair[1].stringify()})\n")
        }
      }
      if corpus_field(method, "endpoint_auth") != null {
        source.write_string("#derive.endpoint_auth(true)\n")
      }
      let params = corpus_array(corpus_field(method, "input"))
        .map(p => {
          let pair = corpus_array(p)
          ", " + corpus_string(pair[0]) + " : " + corpus_type(pair[1])
        })
        .join("")
      source.write_string(
        "pub fn Site::\{name}(self : Self\{params}) -> \{corpus_type(corpus_field(method, "output"))} { ... }\n",
      )
    }
    try {
      let agents = @lib.parse_agents([(id, source.to_string())])
      let agent = {
        ..agents[0],
        mode: if corpus_field(input, "mode") == "durable" {
          @lib.Durable
        } else {
          @lib.Ephemeral
        },
        snapshotting: if corpus_field(input, "snapshot") == true {
          @lib.Default
        } else {
          @lib.Disabled
        },
      }
      @lib.validate_http_routes([agent])
      let emitted = @formatter.impls_to_string(
        @lib.generate_agent_registration([agent]),
      )
      assert_eq(corpus_field(expected, "accepted"), true, msg=id)
      assert_eq(emitted.contains("AgentTypeKind::HTTP_ROUTER"), router, msg=id)
      if corpus_field(expected, "handler") is String(name) {
        assert_true(emitted.contains("\"\{name}\""), msg=id)
        assert_true(emitted.contains("HttpMethod::Any"), msg=id)
      }
      if corpus_field(expected, "provider") is String(name) {
        assert_eq(
          agent.http_mount.unwrap().openapi_provider_method,
          Some(name),
          msg=id,
        )
      }
    } catch {
      @lib.AgentParseError(_) =>
        assert_true(corpus_field(expected, "error") != null, msg=id)
      e => raise e
    }
  }
  assert_eq(count, 13)
}

///|
test "shared tooling corpus keeps router provisioning and excludes clients" {
  let corpus = @json.parse(
    @fs.read_file_to_string(
      "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json",
    ),
  )
  let mut count = 0
  for case in corpus_array(corpus_field(corpus, "cases")) {
    if corpus_field(case, "suite") != "tooling" {
      continue
    }
    let input = corpus_field(case, "input")
    let consumer = corpus_string(corpus_field(input, "consumer"))
    if !["bridge-client", "provisioning", "config-secrets-dependencies"].contains(
        consumer,
      ) {
      continue
    }
    count += 1
    let router = corpus_field(input, "kind") == "http-router"
    let source = if router {
      "#derive.http_router\n#derive.mount(\"/\")\nstruct Site {}\nfn Site::new(config : @config.Config[Settings]) -> Site { Site::{} }"
    } else {
      "#derive.agent\n#derive.mount(\"/\")\n#derive.expose_files(\"/*\", \"/public/$1\")\nstruct Site {}\nfn Site::new() -> Site { Site::{} }"
    }
    let agents = @lib.parse_agents([("tooling.mbt", source)])
    let included = if consumer == "bridge-client" {
      @formatter.impls_to_string(@lib.generate_client_code(agents)).contains(
        "SiteClient",
      )
    } else {
      let output = @formatter.impls_to_string(
        @lib.generate_agent_registration(agents),
      )
      assert_true(output.contains("input_schema: @common.InputSchema::Parameters([])"))
      if consumer == "config-secrets-dependencies" {
        assert_true(output.contains("@config.load_config"))
      }
      output.contains("type_name: \"Site\"")
    }
    assert_eq(
      included.to_json(),
      corpus_field(corpus_field(case, "expect"), "included"),
      msg=corpus_string(corpus_field(case, "id")),
    )
  }
  assert_eq(count, 4)
}
