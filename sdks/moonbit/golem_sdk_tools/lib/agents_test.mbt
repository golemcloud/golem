///|
test "parse_agents finds simple agent struct" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|///| Creates a new counter
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(
    agents[0].name,
    content=(
      #|"Counter"
    ),
  )
  debug_inspect(
    agents[0].description,
    content=(
      #|"A counter agent"
    ),
  )
  debug_inspect(
    agents[0].constructor_description,
    content=(
      #|"Creates a new counter"
    ),
  )
  debug_inspect(agents[0].constructor_params.length(), content="1")
  debug_inspect(
    agents[0].constructor_params[0].0,
    content=(
      #|"name"
    ),
  )
  debug_inspect(agents[0].constructor_params[0].1, content="Simple(\"String\")")
  debug_inspect(agents[0].methods.length(), content="0")
}

///|
test "parse_agents finds agent with multiple params" {
  let content =
    #|///| A greeter agent
    #|#derive.agent
    #|pub(all) struct Greeter {
    #|  greeting : String
    #|  count : UInt64
    #|}
    #|
    #|///| Creates a new greeter
    #|pub fn Greeter::new(greeting : String, count : UInt64) -> Greeter {
    #|  { greeting, count }
    #|}
  let agents = parse_agents([("greeter.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(
    agents[0].name,
    content=(
      #|"Greeter"
    ),
  )
  debug_inspect(agents[0].constructor_params.length(), content="2")
  debug_inspect(
    agents[0].constructor_params[0].0,
    content=(
      #|"greeting"
    ),
  )
  debug_inspect(agents[0].constructor_params[0].1, content="Simple(\"String\")")
  debug_inspect(
    agents[0].constructor_params[1].0,
    content=(
      #|"count"
    ),
  )
  debug_inspect(agents[0].constructor_params[1].1, content="Simple(\"UInt64\")")
  debug_inspect(agents[0].methods.length(), content="0")
}

///|
test "parse_agents detects public methods" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
    #|
    #|///| Increments the counter
    #|pub fn Counter::increment(self : Self) -> Unit {
    #|  self.value += 1
    #|}
    #|
    #|///| Decrements the counter
    #|pub fn Counter::decrement(self : Self) -> Unit {
    #|  self.value -= 1
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents[0].methods.length(), content="2")
  debug_inspect(
    agents[0].methods[0].name,
    content=(
      #|"increment"
    ),
  )
  debug_inspect(
    agents[0].methods[0].description,
    content=(
      #|"Increments the counter"
    ),
  )
  debug_inspect(agents[0].methods[0].params.length(), content="0")
  debug_inspect(agents[0].methods[0].return_type, content="None")
  debug_inspect(
    agents[0].methods[1].name,
    content=(
      #|"decrement"
    ),
  )
  debug_inspect(
    agents[0].methods[1].description,
    content=(
      #|"Decrements the counter"
    ),
  )
}

///|
test "parse_agents detects method with params and return type" {
  let content =
    #|///| A calculator
    #|#derive.agent
    #|pub(all) struct Calc {
    #|  mut result : Int
    #|}
    #|
    #|pub fn Calc::new() -> Calc {
    #|  { result: 0 }
    #|}
    #|
    #|///| Adds a value
    #|pub fn Calc::add(self : Self, value : Int) -> Int {
    #|  self.result += value
    #|  self.result
    #|}
  let agents = parse_agents([("calc.mbt", content)])
  debug_inspect(agents[0].methods.length(), content="1")
  debug_inspect(
    agents[0].methods[0].name,
    content=(
      #|"add"
    ),
  )
  debug_inspect(
    agents[0].methods[0].description,
    content=(
      #|"Adds a value"
    ),
  )
  debug_inspect(agents[0].methods[0].params.length(), content="1")
  debug_inspect(
    agents[0].methods[0].params[0].0,
    content=(
      #|"value"
    ),
  )
  debug_inspect(agents[0].methods[0].params[0].1, content="Simple(\"Int\")")
  debug_inspect(
    agents[0].methods[0].return_type,
    content="Some(Simple(\"Int\"))",
  )
}

///|
test "parse_agents skips private methods" {
  let content =
    #|///| An agent
    #|#derive.agent
    #|pub(all) struct MyAgent {
    #|  mut x : Int
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { x: 0 }
    #|}
    #|
    #|fn MyAgent::helper(self : Self) -> Unit {
    #|  ()
    #|}
    #|
    #|///| Public method
    #|pub fn MyAgent::do_thing(self : Self) -> Unit {
    #|  ()
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].methods.length(), content="1")
  debug_inspect(
    agents[0].methods[0].name,
    content=(
      #|"do_thing"
    ),
  )
}

///|
test "generate_agent_registration for simple agent" {
  let agents : Array[AgentInfo] = [
    {
      name: "Counter",
      description: "A counter agent",
      mode: Durable,
      constructor_params: [("name", Simple("String"))],
      constructor_description: "Creates a new counter",
      constructor_prompt_hint: None,
      methods: [],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let counter_type_def :@agents.AgentTypeDef = {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"name\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Counter\",\n      agent_type: counter_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = @agents.decode_invocation_input(input, 1, \"Counter::new\")\n          let name :String = @agents.decode_param(params, 0, \"Counter::new\")\n          let agent = Counter::new(name)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "generate_agent_registration with methods" {
  let agents : Array[AgentInfo] = [
    {
      name: "Counter",
      description: "A counter agent",
      mode: Durable,
      constructor_params: [("name", Simple("String"))],
      constructor_description: "Creates a new counter",
      constructor_prompt_hint: None,
      methods: [
        {
          name: "increment",
          description: "Increments the counter",
          prompt_hint: None,
          params: [],
          return_type: None,
          http_endpoint: None,
          read_only: None,
        },
        {
          name: "add",
          description: "Adds a value",
          prompt_hint: None,
          params: [("value", Simple("UInt64"))],
          return_type: Some(Simple("UInt64")),
          http_endpoint: None,
          read_only: None,
        },
      ],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let counter_type_def :@agents.AgentTypeDef = {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"name\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"increment\",\n          description: \"Increments the counter\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [],\n          output: None,\n          read_only: None,\n        },\n        {\n          name: \"add\",\n          description: \"Adds a value\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [\n            {\n              name: \"value\",\n              source: @agents.InputParamSource::UserSupplied(\n                @schema.into_schema_graph(\n                  (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n                ),\n              ),\n            },\n          ],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Counter\",\n      agent_type: counter_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = @agents.decode_invocation_input(input, 1, \"Counter::new\")\n          let name :String = @agents.decode_param(params, 0, \"Counter::new\")\n          let agent = Counter::new(name)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"increment\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"Counter.increment\")\n        self.increment()\n        Ok(None)\n      }\n      \"add\" => {\n        let params = @agents.decode_invocation_input(input, 1, \"Counter.add\")\n        let value :UInt64 = @agents.decode_param(params, 0, \"Counter.add\")\n        let result = self.add(value)\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "parse_agents extracts #derive.read_only annotations" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
    #|
    #|///| Bare read-only (default cache policy: until-write)
    #|#derive.read_only
    #|pub fn Counter::get_value(self : Self) -> UInt64 {
    #|  self.value
    #|}
    #|
    #|///| Read-only with explicit no_cache
    #|#derive.read_only(cache="no_cache")
    #|pub fn Counter::peek(self : Self) -> UInt64 {
    #|  self.value
    #|}
    #|
    #|///| Read-only with explicit until-write
    #|#derive.read_only(cache="until-write")
    #|pub fn Counter::name(self : Self) -> String {
    #|  self.name
    #|}
    #|
    #|///| Read-only with TTL in nanoseconds
    #|#derive.read_only(cache="ttl", ttl="30000000000")
    #|pub fn Counter::cached_value(self : Self) -> UInt64 {
    #|  self.value
    #|}
    #|
    #|///| Plain method, no read_only
    #|pub fn Counter::increment(self : Self) -> Unit {
    #|  self.value += 1
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents[0].methods.length(), content="5")
  debug_inspect(
    agents[0].methods[0].read_only,
    content="Some({ cache_policy: UntilWrite })",
  )
  debug_inspect(
    agents[0].methods[1].read_only,
    content="Some({ cache_policy: NoCache })",
  )
  debug_inspect(
    agents[0].methods[2].read_only,
    content="Some({ cache_policy: UntilWrite })",
  )
  debug_inspect(
    agents[0].methods[3].read_only,
    content="Some({ cache_policy: Ttl(30000000000) })",
  )
  debug_inspect(agents[0].methods[4].read_only, content="None")
}

///|
/// Verifies that `#derive.read_only` is parsed and that `uses_principal` should be
/// derived from the method's parameter list (presence of `Principal`), never accepted
/// from user input.
test "parse_agents derives uses_principal from Principal parameter (not user input)" {
  let content =
    #|///| A read-only agent
    #|#derive.agent
    #|pub(all) struct ReadAgent {
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn ReadAgent::new() -> ReadAgent {
    #|  { value: 0 }
    #|}
    #|
    #|///| Plain read-only without Principal
    #|#derive.read_only
    #|pub fn ReadAgent::get_value(self : Self) -> UInt64 {
    #|  self.value
    #|}
    #|
    #|///| Read-only with Principal parameter
    #|#derive.read_only
    #|pub fn ReadAgent::get_value_for(
    #|  self : Self,
    #|  caller : Principal,
    #|) -> UInt64 {
    #|  self.value
    #|}
  let agents = parse_agents([("read_agent.mbt", content)])
  debug_inspect(agents[0].methods.length(), content="2")
  let has_principal_0 = agents[0].methods[0].params
    .iter()
    .any(fn(p) { is_principal_type(p.1) })
  let has_principal_1 = agents[0].methods[1].params
    .iter()
    .any(fn(p) { is_principal_type(p.1) })
  debug_inspect(has_principal_0, content="false")
  debug_inspect(has_principal_1, content="true")
}

///|
test "parse_agents rejects ephemeral + read_only combination" {
  let content =
    #|///| An ephemeral agent
    #|#derive.agent("ephemeral")
    #|pub(all) struct Stateless {
    #|  name : String
    #|}
    #|
    #|pub fn Stateless::new(name : String) -> Stateless {
    #|  { name }
    #|}
    #|
    #|///| This should fail because ephemeral + read_only is not allowed
    #|#derive.read_only
    #|pub fn Stateless::peek(self : Self) -> String {
    #|  self.name
    #|}
  let result = try? parse_agents([("stateless.mbt", content)])
  match result {
    Err(AgentParseError(msg)) => {
      assert_true(msg.contains("read-only"))
      assert_true(msg.contains("ephemeral"))
      assert_true(msg.contains("Stateless"))
    }
    Ok(_) => abort("Expected parse_agents to fail on ephemeral + read_only")
  }
}

///|
/// Verifies that `generate_agent_registration` emits the `read_only` field on
/// the registered `AgentMethod`, with cache policy from the annotation and
/// `uses_principal` derived from the presence of a `Principal` parameter.
test "generate_agent_registration emits read_only with derived uses_principal" {
  let content =
    #|///| A read-only agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new() -> Counter {
    #|  { value: 0 }
    #|}
    #|
    #|///| Plain read-only without Principal
    #|#derive.read_only(cache="ttl", ttl="30000000000")
    #|pub fn Counter::get_value(self : Self) -> UInt64 {
    #|  self.value
    #|}
    #|
    #|///| Read-only with Principal parameter
    #|#derive.read_only
    #|pub fn Counter::get_value_for(
    #|  self : Self,
    #|  caller : Principal,
    #|) -> UInt64 {
    #|  self.value
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let counter_type_def :@agents.AgentTypeDef = {\n      type_name: \"Counter\",\n      description: \"A read-only agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new Counter\",\n        prompt_hint: None,\n        input: [],\n      },\n      methods: [\n        {\n          name: \"get_value\",\n          description: \"Plain read-only without Principal\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          read_only: Some(\n            (\n              {\n                cache_policy: @common.CachePolicy::Ttl(30000000000UL),\n                uses_principal: false,\n              }\n              : @common.ReadOnlyConfig\n            ),\n          ),\n        },\n        {\n          name: \"get_value_for\",\n          description: \"Read-only with Principal parameter\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [\n            {\n              name: \"caller\",\n              source: @agents.InputParamSource::AutoInjected(\n                @common.AutoInjectedKind::PRINCIPAL,\n              ),\n            },\n          ],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          read_only: Some(\n            (\n              {\n                cache_policy: @common.CachePolicy::UntilWrite,\n                uses_principal: true,\n              }\n              : @common.ReadOnlyConfig\n            ),\n          ),\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Counter\",\n      agent_type: counter_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let _ = @agents.decode_invocation_input(input, 0, \"Counter::new\")\n          let agent = Counter::new()\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"get_value\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"Counter.get_value\")\n        let result = self.get_value()\n        Ok(Some(@agents.encode_output(result)))\n      }\n      \"get_value_for\" => {\n        let _ = \n          @agents.decode_invocation_input(input, 0, \"Counter.get_value_for\")\n        let result = self.get_value_for(principal)\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "parse_agents extracts prompt_hint from constructor and method" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|///| Creates a new counter
    #|#derive.prompt_hint("Provide a unique name for the counter")
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
    #|
    #|///| Increments the counter
    #|#derive.prompt_hint("Call this to increase the value by one")
    #|pub fn Counter::increment(self : Self) -> Unit {
    #|  self.value += 1
    #|}
    #|
    #|///| Decrements the counter
    #|pub fn Counter::decrement(self : Self) -> Unit {
    #|  self.value -= 1
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(
    agents[0].constructor_prompt_hint,
    content="Some(\"Provide a unique name for the counter\")",
  )
  debug_inspect(agents[0].methods.length(), content="2")
  debug_inspect(
    agents[0].methods[0].prompt_hint,
    content="Some(\"Call this to increase the value by one\")",
  )
  debug_inspect(agents[0].methods[1].prompt_hint, content="None")
}

///|
test "generate_agent_registration with prompt_hints" {
  let agents : Array[AgentInfo] = [
    {
      name: "Counter",
      description: "A counter agent",
      mode: Durable,
      constructor_params: [("name", Simple("String"))],
      constructor_description: "Creates a new counter",
      constructor_prompt_hint: Some("Provide a unique name"),
      methods: [
        {
          name: "increment",
          description: "Increments the counter",
          prompt_hint: Some("Call to increase"),
          params: [],
          return_type: None,
          http_endpoint: None,
          read_only: None,
        },
      ],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let counter_type_def :@agents.AgentTypeDef = {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: Some(\"Provide a unique name\"),\n        input: [\n          {\n            name: \"name\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"increment\",\n          description: \"Increments the counter\",\n          http_endpoint: [],\n          prompt_hint: Some(\"Call to increase\"),\n          input: [],\n          output: None,\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Counter\",\n      agent_type: counter_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = @agents.decode_invocation_input(input, 1, \"Counter::new\")\n          let name :String = @agents.decode_param(params, 0, \"Counter::new\")\n          let agent = Counter::new(name)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"increment\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"Counter.increment\")\n        self.increment()\n        Ok(None)\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
/// Parses agent sources and renders the full generated `golem_agents.mbt`
/// (registration `init` + `RawAgent` dispatch) for the new schema model.
fn parse_and_build_agent_type(files : Array[(String, String)]) -> String raise {
  let agents = parse_agents(files)
  guard agents.length() == 1 else {
    fail("Expected exactly 1 agent, got \{agents.length()}")
  }
  let impls = generate_agent_registration(agents)
  @formatter.impls_to_string(impls)
}

///|
test "end-to-end: simple counter agent type" {
  let source =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|///| Creates a new counter
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
  let output = parse_and_build_agent_type([("counter.mbt", source)])
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let counter_type_def :@agents.AgentTypeDef = {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"name\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Counter\",\n      agent_type: counter_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = @agents.decode_invocation_input(input, 1, \"Counter::new\")\n          let name :String = @agents.decode_param(params, 0, \"Counter::new\")\n          let agent = Counter::new(name)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "end-to-end: agent with methods" {
  let source =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|///| Creates a new counter
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
    #|
    #|///| Increments the counter by one
    #|pub fn Counter::increment(self : Self) -> Unit {
    #|  self.value += 1
    #|}
    #|
    #|///| Adds a value to the counter
    #|pub fn Counter::add(self : Self, value : UInt64) -> UInt64 {
    #|  self.value += value
    #|  self.value
    #|}
  let output = parse_and_build_agent_type([("counter.mbt", source)])
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let counter_type_def :@agents.AgentTypeDef = {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"name\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"increment\",\n          description: \"Increments the counter by one\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [],\n          output: None,\n          read_only: None,\n        },\n        {\n          name: \"add\",\n          description: \"Adds a value to the counter\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [\n            {\n              name: \"value\",\n              source: @agents.InputParamSource::UserSupplied(\n                @schema.into_schema_graph(\n                  (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n                ),\n              ),\n            },\n          ],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Counter\",\n      agent_type: counter_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = @agents.decode_invocation_input(input, 1, \"Counter::new\")\n          let name :String = @agents.decode_param(params, 0, \"Counter::new\")\n          let agent = Counter::new(name)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"increment\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"Counter.increment\")\n        self.increment()\n        Ok(None)\n      }\n      \"add\" => {\n        let params = @agents.decode_invocation_input(input, 1, \"Counter.add\")\n        let value :UInt64 = @agents.decode_param(params, 0, \"Counter.add\")\n        let result = self.add(value)\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "end-to-end: agent with prompt hints" {
  let source =
    #|///| A smart agent
    #|#derive.agent
    #|pub(all) struct SmartAgent {
    #|  config : String
    #|}
    #|
    #|///| Creates a smart agent
    #|#derive.prompt_hint("Provide a JSON config string")
    #|pub fn SmartAgent::new(config : String) -> SmartAgent {
    #|  { config }
    #|}
    #|
    #|///| Processes input text
    #|#derive.prompt_hint("Pass natural language text")
    #|pub fn SmartAgent::process(self : Self, text : String) -> String {
    #|  self.config + text
    #|}
  let output = parse_and_build_agent_type([("smart.mbt", source)])
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let smartAgent_type_def :@agents.AgentTypeDef = {\n      type_name: \"SmartAgent\",\n      description: \"A smart agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a smart agent\",\n        prompt_hint: Some(\"Provide a JSON config string\"),\n        input: [\n          {\n            name: \"config\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"process\",\n          description: \"Processes input text\",\n          http_endpoint: [],\n          prompt_hint: Some(\"Pass natural language text\"),\n          input: [\n            {\n              name: \"text\",\n              source: @agents.InputParamSource::UserSupplied(\n                @schema.into_schema_graph(\n                  (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n                ),\n              ),\n            },\n          ],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"SmartAgent\",\n      agent_type: smartAgent_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = \n            @agents.decode_invocation_input(input, 1, \"SmartAgent::new\")\n          let config :String = \n            @agents.decode_param(params, 0, \"SmartAgent::new\")\n          let agent = SmartAgent::new(config)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for SmartAgent with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"process\" => {\n        let params = \n          @agents.decode_invocation_input(input, 1, \"SmartAgent.process\")\n        let text :String = @agents.decode_param(params, 0, \"SmartAgent.process\")\n        let result = self.process(text)\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "end-to-end: agent with complex param types" {
  let source =
    #|///| A data processor
    #|#derive.agent
    #|pub(all) struct Processor {
    #|  name : String
    #|}
    #|
    #|pub fn Processor::new(name : String) -> Processor {
    #|  { name }
    #|}
    #|
    #|///| Processes a batch of items
    #|pub fn Processor::batch(self : Self, items : Array[String], count : UInt64?) -> Array[String] {
    #|  []
    #|}
  let output = parse_and_build_agent_type([("processor.mbt", source)])
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let processor_type_def :@agents.AgentTypeDef = {\n      type_name: \"Processor\",\n      description: \"A data processor\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new Processor\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"name\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"batch\",\n          description: \"Processes a batch of items\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [\n            {\n              name: \"items\",\n              source: @agents.InputParamSource::UserSupplied(\n                @schema.into_schema_graph(\n                  (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),\n                ),\n              ),\n            },\n            {\n              name: \"count\",\n              source: @agents.InputParamSource::UserSupplied(\n                @schema.into_schema_graph(\n                  (@schema.TypeTag::{  } : @schema.TypeTag[UInt64?]),\n                ),\n              ),\n            },\n          ],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Processor\",\n      agent_type: processor_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = \n            @agents.decode_invocation_input(input, 1, \"Processor::new\")\n          let name :String = @agents.decode_param(params, 0, \"Processor::new\")\n          let agent = Processor::new(name)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Processor with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"batch\" => {\n        let params = \n          @agents.decode_invocation_input(input, 2, \"Processor.batch\")\n        let items :Array[String] = \n          @agents.decode_param(params, 0, \"Processor.batch\")\n        let count :UInt64? = @agents.decode_param(params, 1, \"Processor.batch\")\n        let result = self.batch(items, count)\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "end-to-end: agent with no constructor params" {
  let source =
    #|///| A simple worker
    #|#derive.agent
    #|pub(all) struct Worker {
    #|  mut count : Int
    #|}
    #|
    #|///| Creates a default worker
    #|pub fn Worker::new() -> Worker {
    #|  { count: 0 }
    #|}
    #|
    #|///| Does work
    #|pub fn Worker::run(self : Self) -> Int {
    #|  self.count += 1
    #|  self.count
    #|}
  let output = parse_and_build_agent_type([("worker.mbt", source)])
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let worker_type_def :@agents.AgentTypeDef = {\n      type_name: \"Worker\",\n      description: \"A simple worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a default worker\",\n        prompt_hint: None,\n        input: [],\n      },\n      methods: [\n        {\n          name: \"run\",\n          description: \"Does work\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Int]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Worker\",\n      agent_type: worker_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let _ = @agents.decode_invocation_input(input, 0, \"Worker::new\")\n          let agent = Worker::new()\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"run\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"Worker.run\")\n        let result = self.run()\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "end-to-end: agent split across multiple files" {
  let struct_file =
    #|///| A multi-file agent
    #|#derive.agent
    #|pub(all) struct MultiAgent {
    #|  id : UInt64
    #|}
    #|
    #|pub fn MultiAgent::new(id : UInt64) -> MultiAgent {
    #|  { id }
    #|}
  let methods_file =
    #|///| Gets the ID
    #|pub fn MultiAgent::get_id(self : Self) -> UInt64 {
    #|  self.id
    #|}
  let output = parse_and_build_agent_type([
    ("multi_agent.mbt", struct_file),
    ("multi_agent_methods.mbt", methods_file),
  ])
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let multiAgent_type_def :@agents.AgentTypeDef = {\n      type_name: \"MultiAgent\",\n      description: \"A multi-file agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new MultiAgent\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"id\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"get_id\",\n          description: \"Gets the ID\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"MultiAgent\",\n      agent_type: multiAgent_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = \n            @agents.decode_invocation_input(input, 1, \"MultiAgent::new\")\n          let id :UInt64 = @agents.decode_param(params, 0, \"MultiAgent::new\")\n          let agent = MultiAgent::new(id)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for MultiAgent with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"get_id\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"MultiAgent.get_id\")\n        let result = self.get_id()\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "build_raw_agent_impl for counter-like agent" {
  let agent : AgentInfo = {
    name: "Counter",
    description: "A counter agent",
    mode: Durable,
    constructor_params: [("name", Simple("String"))],
    constructor_description: "Creates a new counter",
    constructor_prompt_hint: None,
    methods: [
      {
        name: "increment",
        description: "Increments the counter",
        prompt_hint: None,
        params: [],
        return_type: None,
        http_endpoint: None,
        read_only: None,
      },
      {
        name: "get_value",
        description: "Gets the current value",
        prompt_hint: None,
        params: [],
        return_type: Some(Simple("UInt64")),
        http_endpoint: None,
        read_only: None,
      },
      {
        name: "add",
        description: "Adds a value",
        prompt_hint: None,
        params: [("value", Simple("UInt64"))],
        return_type: Some(Simple("UInt64")),
        http_endpoint: None,
        read_only: None,
      },
    ],
    has_init: false,
    has_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: None,
  }
  let output = @formatter.impls_to_string(generate_agent_registration([agent]))
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let counter_type_def :@agents.AgentTypeDef = {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"name\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"increment\",\n          description: \"Increments the counter\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [],\n          output: None,\n          read_only: None,\n        },\n        {\n          name: \"get_value\",\n          description: \"Gets the current value\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          read_only: None,\n        },\n        {\n          name: \"add\",\n          description: \"Adds a value\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [\n            {\n              name: \"value\",\n              source: @agents.InputParamSource::UserSupplied(\n                @schema.into_schema_graph(\n                  (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n                ),\n              ),\n            },\n          ],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Counter\",\n      agent_type: counter_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = @agents.decode_invocation_input(input, 1, \"Counter::new\")\n          let name :String = @agents.decode_param(params, 0, \"Counter::new\")\n          let agent = Counter::new(name)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"increment\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"Counter.increment\")\n        self.increment()\n        Ok(None)\n      }\n      \"get_value\" => {\n        let _ = @agents.decode_invocation_input(input, 0, \"Counter.get_value\")\n        let result = self.get_value()\n        Ok(Some(@agents.encode_output(result)))\n      }\n      \"add\" => {\n        let params = @agents.decode_invocation_input(input, 1, \"Counter.add\")\n        let value :UInt64 = @agents.decode_param(params, 0, \"Counter.add\")\n        let result = self.add(value)\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "encode_qualified creates Simple with @pkg.id format" {
  debug_inspect(
    encode_qualified("types", "Config"),
    content="Simple(\"@types.Config\")",
  )
  debug_inspect(
    encode_qualified("my_pkg", "MyType"),
    content="Simple(\"@my_pkg.MyType\")",
  )
}

///|
test "decode_qualified parses @pkg.id format" {
  debug_inspect(
    decode_qualified("@types.Config"),
    content="Some((\"types\", \"Config\"))",
  )
  debug_inspect(
    decode_qualified("@my_pkg.MyType"),
    content="Some((\"my_pkg\", \"MyType\"))",
  )
  debug_inspect(decode_qualified("String"), content="None")
  debug_inspect(decode_qualified("UserId"), content="None")
  debug_inspect(decode_qualified("@"), content="None")
  debug_inspect(decode_qualified("@."), content="None")
  debug_inspect(decode_qualified("@pkg."), content="None")
  debug_inspect(decode_qualified("@.Type"), content="None")
}

///|
test "encode/decode_qualified roundtrips" {
  let cases = [("types", "Config"), ("my_pkg", "MyType"), ("a", "B")]
  for pair in cases {
    let (pkg, id) = pair
    let encoded = encode_qualified(pkg, id)
    match encoded {
      Simple(name) =>
        debug_inspect(
          decode_qualified(name),
          content="Some((\"\{pkg}\", \"\{id}\"))",
        )
      _ => fail("encode_qualified should return Simple")
    }
  }
}

///|
test "parse_agents with cross-package param types" {
  let content =
    #|///| An agent using cross-package types
    #|#derive.agent
    #|pub(all) struct OrderAgent {
    #|  state : @types.State
    #|}
    #|
    #|pub fn OrderAgent::new(config : @types.Config) -> OrderAgent {
    #|  { state: @types.State::default() }
    #|}
    #|
    #|///| Processes an order
    #|pub fn OrderAgent::process(self : Self, order : @types.Order) -> @types.Result {
    #|  @types.Result::ok()
    #|}
  let agents = parse_agents([("order.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].constructor_params.length(), content="1")
  debug_inspect(
    agents[0].constructor_params[0].1,
    content="Simple(\"@types.Config\")",
  )
  debug_inspect(agents[0].methods.length(), content="1")
  debug_inspect(
    agents[0].methods[0].params[0].1,
    content="Simple(\"@types.Order\")",
  )
  debug_inspect(
    agents[0].methods[0].return_type,
    content="Some(Simple(\"@types.Result\"))",
  )
}

///|
test "parse_agents with type alias in params" {
  let content =
    #|///| Agent using type aliases
    #|#derive.agent
    #|pub(all) struct AliasAgent {
    #|  id : UserId
    #|}
    #|
    #|pub fn AliasAgent::new(id : UserId) -> AliasAgent {
    #|  { id }
    #|}
    #|
    #|///| Looks up a user
    #|pub fn AliasAgent::lookup(self : Self, name : UserName) -> UserId {
    #|  self.id
    #|}
  let agents = parse_agents([("alias.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].constructor_params[0].1, content="Simple(\"UserId\")")
  debug_inspect(
    agents[0].methods[0].params[0].1,
    content="Simple(\"UserName\")",
  )
  debug_inspect(
    agents[0].methods[0].return_type,
    content="Some(Simple(\"UserId\"))",
  )
}

///|
test "parse_agents with cross-package type in compound positions" {
  let content =
    #|///| Agent with nested cross-package types
    #|#derive.agent
    #|pub(all) struct NestedAgent {
    #|  items : Array[@types.Item]
    #|}
    #|
    #|pub fn NestedAgent::new() -> NestedAgent {
    #|  { items: [] }
    #|}
    #|
    #|///| Gets optional info
    #|pub fn NestedAgent::get_info(self : Self) -> @types.Info? {
    #|  None
    #|}
    #|
    #|///| Returns a result with cross-package types
    #|pub fn NestedAgent::try_fetch(self : Self) -> Result[@types.Data, @types.Error] {
    #|  Err(@types.Error::new())
    #|}
  let agents = parse_agents([("nested.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(
    agents[0].methods[0].return_type,
    content="Some(Optional(Simple(\"@types.Info\")))",
  )
  debug_inspect(
    agents[0].methods[1].return_type,
    content="Some(ResultType(Simple(\"@types.Data\"), Simple(\"@types.Error\")))",
  )
}

///|
test "param_type_to_type_string for various types" {
  debug_inspect(
    param_type_to_type_string(Simple("String")),
    content=(
      #|"String"
    ),
  )
  debug_inspect(
    param_type_to_type_string(Simple("UInt64")),
    content=(
      #|"UInt64"
    ),
  )
  debug_inspect(
    param_type_to_type_string(Optional(Simple("String"))),
    content=(
      #|"String?"
    ),
  )
  debug_inspect(
    param_type_to_type_string(List(Simple("Int"))),
    content=(
      #|"Array[Int]"
    ),
  )
  debug_inspect(
    param_type_to_type_string(ResultType(Simple("String"), Simple("Int"))),
    content=(
      #|"Result[String, Int]"
    ),
  )
  debug_inspect(
    param_type_to_type_string(Optional(List(Simple("String")))),
    content=(
      #|"Array[String]?"
    ),
  )
}

///|
test "generate_agent_registration with cross-package types" {
  let agents : Array[AgentInfo] = [
    {
      name: "OrderAgent",
      description: "An order agent",
      mode: Durable,
      constructor_params: [("config", Simple("@types.Config"))],
      constructor_description: "Creates an order agent",
      constructor_prompt_hint: None,
      methods: [
        {
          name: "process",
          description: "Processes an order",
          prompt_hint: None,
          params: [("order", Simple("@types.Order"))],
          return_type: Some(Simple("@types.Result")),
          http_endpoint: None,
          read_only: None,
        },
      ],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let orderAgent_type_def :@agents.AgentTypeDef = {\n      type_name: \"OrderAgent\",\n      description: \"An order agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates an order agent\",\n        prompt_hint: None,\n        input: [\n          {\n            name: \"config\",\n            source: @agents.InputParamSource::UserSupplied(\n              @schema.into_schema_graph(\n                (@schema.TypeTag::{  } : @schema.TypeTag[@types.Config]),\n              ),\n            ),\n          },\n        ],\n      },\n      methods: [\n        {\n          name: \"process\",\n          description: \"Processes an order\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input: [\n            {\n              name: \"order\",\n              source: @agents.InputParamSource::UserSupplied(\n                @schema.into_schema_graph(\n                  (@schema.TypeTag::{  } : @schema.TypeTag[@types.Order]),\n                ),\n              ),\n            },\n          ],\n          output: Some(\n            @schema.into_schema_graph(\n              (@schema.TypeTag::{  } : @schema.TypeTag[@types.Result]),\n            ),\n          ),\n          read_only: None,\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"OrderAgent\",\n      agent_type: orderAgent_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let params = \n            @agents.decode_invocation_input(input, 1, \"OrderAgent::new\")\n          let config :@types.Config = \n            @agents.decode_param(params, 0, \"OrderAgent::new\")\n          let agent = OrderAgent::new(config)\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for OrderAgent with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      \"process\" => {\n        let params = \n          @agents.decode_invocation_input(input, 1, \"OrderAgent.process\")\n        let order :@types.Order = \n          @agents.decode_param(params, 0, \"OrderAgent.process\")\n        let result = self.process(order)\n        Ok(Some(@agents.encode_output(result)))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "param_type_to_type_string for cross-package types" {
  debug_inspect(
    param_type_to_type_string(Simple("@types.Config")),
    content=(
      #|"@types.Config"
    ),
  )
  debug_inspect(
    param_type_to_type_string(Optional(Simple("@types.Info"))),
    content=(
      #|"@types.Info?"
    ),
  )
  debug_inspect(
    param_type_to_type_string(List(Simple("@types.Item"))),
    content=(
      #|"Array[@types.Item]"
    ),
  )
  debug_inspect(
    param_type_to_type_string(
      ResultType(Simple("@types.Data"), Simple("@types.Error")),
    ),
    content=(
      #|"Result[@types.Data, @types.Error]"
    ),
  )
}

///|
test "param_type_to_type_string for tuples" {
  debug_inspect(
    param_type_to_type_string(Tuple([Simple("String"), Simple("Int")])),
    content=(
      #|"(String, Int)"
    ),
  )
  debug_inspect(
    param_type_to_type_string(
      Tuple([Simple("String"), Simple("Int"), Simple("Bool")]),
    ),
    content=(
      #|"(String, Int, Bool)"
    ),
  )
}

///|
test "parse_agents with tuple return type" {
  let content =
    #|///| Agent with tuples
    #|#derive.agent
    #|pub(all) struct TupleAgent {
    #|  mut x : Int
    #|}
    #|
    #|pub fn TupleAgent::new() -> TupleAgent {
    #|  { x: 0 }
    #|}
    #|
    #|///| Returns a pair
    #|pub fn TupleAgent::get_pair(self : Self) -> (String, Int) {
    #|  ("hello", self.x)
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].methods.length(), content="1")
  debug_inspect(
    agents[0].methods[0].name,
    content=(
      #|"get_pair"
    ),
  )
  debug_inspect(
    agents[0].methods[0].return_type,
    content=(
      #|Some(([Simple("String"), Simple("Int")]))
    ),
  )
}

///|
test "parse_agents defaults to durable mode" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|}
    #|
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents[0].mode, content="Durable")
}

///|
test "parse_agents with explicit durable mode" {
  let content =
    #|///| A counter agent
    #|#derive.agent("durable")
    #|pub(all) struct Counter {
    #|  name : String
    #|}
    #|
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents[0].mode, content="Durable")
}

///|
test "parse_agents with ephemeral mode" {
  let content =
    #|///| An ephemeral agent
    #|#derive.agent("ephemeral")
    #|pub(all) struct EphemeralAgent {
    #|  name : String
    #|}
    #|
    #|pub fn EphemeralAgent::new(name : String) -> EphemeralAgent {
    #|  { name }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].mode, content="Ephemeral")
}

///|
test "generate_agent_registration with ephemeral mode" {
  let agents : Array[AgentInfo] = [
    {
      name: "Worker",
      description: "An ephemeral worker",
      mode: Ephemeral,
      constructor_params: [],
      constructor_description: "Creates a worker",
      constructor_prompt_hint: None,
      methods: [],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let worker_type_def :@agents.AgentTypeDef = {\n      type_name: \"Worker\",\n      description: \"An ephemeral worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input: [],\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::EPHEMERAL,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Worker\",\n      agent_type: worker_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let _ = @agents.decode_invocation_input(input, 0, \"Worker::new\")\n          let agent = Worker::new()\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "parse_agent_with_multimodal_return" {
  let source =
    #|///| Echo agent
    #|#derive.agent
    #|pub(all) struct EchoAgent {
    #|  value : String
    #|}
    #|
    #|pub fn EchoAgent::new() -> EchoAgent {
    #|  { value: "" }
    #|}
    #|
    #|///| Echo multimodal
    #|pub fn EchoAgent::echo(self : Self, input : Multimodal[TextOrBinary]) -> Multimodal[TextOrBinary] {
    #|  input
    #|}
  let result = parse_agents([("test.mbt", source)])
  let m = result[0].methods[0]
  debug_inspect(
    m.params,
    content=(
      #|[("input", Parameterized("Multimodal", [Simple("TextOrBinary")]))]
    ),
  )
  debug_inspect(
    m.return_type,
    content=(
      #|Some(Parameterized("Multimodal", [Simple("TextOrBinary")]))
    ),
  )
}

///|
test "parse_agent recognizes @multimodal.Multimodal as MultimodalType" {
  let source =
    #|///| Vision agent
    #|#derive.agent
    #|pub(all) struct VisionAgent {
    #|  value : String
    #|}
    #|
    #|pub fn VisionAgent::new() -> VisionAgent {
    #|  { value: "" }
    #|}
    #|
    #|///| Analyze
    #|pub fn VisionAgent::analyze(self : Self, input : @multimodal.Multimodal[TextOrImage]) -> String {
    #|  ""
    #|}
  let result = parse_agents([("test.mbt", source)])
  let m = result[0].methods[0]
  debug_inspect(
    m.params,
    content=(
      #|[("input", MultimodalType(Simple("TextOrImage")))]
    ),
  )
}

///|
test "parse_agent allows multimodal mixed with a regular parameter" {
  // The new model drops the old "multimodal must be the only param" rule
  // (Rust/TS iterate all params with no such restriction).
  let source =
    #|///| Vision agent
    #|#derive.agent
    #|pub(all) struct VisionAgent {
    #|  value : String
    #|}
    #|
    #|pub fn VisionAgent::new() -> VisionAgent {
    #|  { value: "" }
    #|}
    #|
    #|///| Describe with a prefix
    #|pub fn VisionAgent::describe(self : Self, prefix : String, input : @multimodal.Multimodal[TextOrImage]) -> String {
    #|  ""
    #|}
  // parse_agents raises AgentParseError if mixed multimodal params were
  // rejected; calling it directly makes the test fail in that case.
  let agents = parse_agents([("test.mbt", source)])
  debug_inspect(
    agents[0].methods[0].params,
    content=(
      #|[
      #|  ("prefix", Simple("String")),
      #|  ("input", MultimodalType(Simple("TextOrImage"))),
      #|]
    ),
  )
}

///|
test "parse_agent rejects multimodal nested inside a container" {
  let source =
    #|///| Vision agent
    #|#derive.agent
    #|pub(all) struct VisionAgent {
    #|  value : String
    #|}
    #|
    #|pub fn VisionAgent::new() -> VisionAgent {
    #|  { value: "" }
    #|}
    #|
    #|///| Bad: multimodal inside Array
    #|pub fn VisionAgent::analyze(self : Self, input : Array[@multimodal.Multimodal[TextOrImage]]) -> String {
    #|  ""
    #|}
  let result : Result[_, _] = try? parse_agents([("test.mbt", source)])
  debug_inspect(
    result,
    content=(
      #|Err(AgentParseError("Multimodal cannot be nested inside Array for parameter 'input'; it must be a direct parameter or return type"))
    ),
  )
}

///|
test "parse_agents detects Snapshottable impl" {
  let content =
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new() -> Counter {
    #|  { value: 0 }
    #|}
    #|
    #|impl @agents.Snapshottable for Counter with save_snapshot(self) {
    #|  b""
    #|}
    #|
    #|impl @agents.Snapshottable for Counter with load_snapshot(self, _bytes) {
    #|  Ok(())
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].has_snapshottable, content="true")
}

///|
test "parse_agents detects no Snapshottable when not implemented" {
  let content =
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new() -> Counter {
    #|  { value: 0 }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].has_snapshottable, content="false")
}

///|
test "parse_agents detects Snapshottable across files regardless of order" {
  // Snapshottable impl in first file, agent struct in second file
  let impl_file =
    #|impl @agents.Snapshottable for Counter with save_snapshot(self) {
    #|  b""
    #|}
    #|
    #|impl @agents.Snapshottable for Counter with load_snapshot(self, _bytes) {
    #|  Ok(())
    #|}
  let agent_file =
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  mut value : UInt64
    #|}
    #|
    #|pub fn Counter::new() -> Counter {
    #|  { value: 0 }
    #|}
  let agents = parse_agents([
    ("snapshottable.mbt", impl_file),
    ("counter.mbt", agent_file),
  ])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].has_snapshottable, content="true")
}

///|
test "parse_agents with snapshotting disabled" {
  let content =
    #|#derive.agent(snapshotting="disabled")
    #|pub(all) struct MyAgent {
    #|  value : String
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { value: "" }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].snapshotting, content="Disabled")
}

///|
test "parse_agents with snapshotting default" {
  let content =
    #|#derive.agent(snapshotting="default")
    #|pub(all) struct MyAgent {
    #|  value : String
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { value: "" }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].snapshotting, content="Default")
}

///|
test "parse_agents with snapshotting periodic" {
  let content =
    #|#derive.agent(snapshotting="periodic(100)")
    #|pub(all) struct MyAgent {
    #|  value : String
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { value: "" }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].snapshotting, content="Periodic(100)")
}

///|
test "parse_agents with snapshotting every_n" {
  let content =
    #|#derive.agent(snapshotting="every_n(5)")
    #|pub(all) struct MyAgent {
    #|  value : String
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { value: "" }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].snapshotting, content="EveryNInvocation(5)")
}

///|
test "parse_agents with ephemeral and snapshotting" {
  let content =
    #|#derive.agent("ephemeral", snapshotting="periodic(50)")
    #|pub(all) struct MyAgent {
    #|  value : String
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { value: "" }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].mode, content="Ephemeral")
  debug_inspect(agents[0].snapshotting, content="Periodic(50)")
}

///|
test "parse_agents with snapshotting periodic human-readable" {
  let content =
    #|#derive.agent(snapshotting="periodic(10 minutes)")
    #|pub(all) struct MyAgent {
    #|  value : String
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { value: "" }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].snapshotting, content="Periodic(600)")
}

///|
test "parse_agents with snapshotting periodic compound duration" {
  let content =
    #|#derive.agent(snapshotting="periodic(1 hour 30 minutes)")
    #|pub(all) struct MyAgent {
    #|  value : String
    #|}
    #|
    #|pub fn MyAgent::new() -> MyAgent {
    #|  { value: "" }
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(agents[0].snapshotting, content="Periodic(5400)")
}

///|
test "generate_agent_registration with snapshotting periodic" {
  let agents : Array[AgentInfo] = [
    {
      name: "Worker",
      description: "A worker",
      mode: Durable,
      snapshotting: Periodic(100),
      constructor_params: [],
      constructor_description: "Creates a worker",
      constructor_prompt_hint: None,
      methods: [],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let worker_type_def :@agents.AgentTypeDef = {\n      type_name: \"Worker\",\n      description: \"A worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input: [],\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Enabled(\n        @common.SnapshottingConfig::Periodic(100UL),\n      ),\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Worker\",\n      agent_type: worker_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let _ = @agents.decode_invocation_input(input, 0, \"Worker::new\")\n          let agent = Worker::new()\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "generate_agent_registration with snapshotting default" {
  let agents : Array[AgentInfo] = [
    {
      name: "Worker",
      description: "A worker",
      mode: Durable,
      snapshotting: Default,
      constructor_params: [],
      constructor_description: "Creates a worker",
      constructor_prompt_hint: None,
      methods: [],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let worker_type_def :@agents.AgentTypeDef = {\n      type_name: \"Worker\",\n      description: \"A worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input: [],\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Enabled(\n        @common.SnapshottingConfig::Default,\n      ),\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Worker\",\n      agent_type: worker_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let _ = @agents.decode_invocation_input(input, 0, \"Worker::new\")\n          let agent = Worker::new()\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "generate_agent_registration with snapshotting every_n" {
  let agents : Array[AgentInfo] = [
    {
      name: "Worker",
      description: "A worker",
      mode: Durable,
      snapshotting: EveryNInvocation(5),
      constructor_params: [],
      constructor_description: "Creates a worker",
      constructor_prompt_hint: None,
      methods: [],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\nfn init {\n  try {\n    let worker_type_def :@agents.AgentTypeDef = {\n      type_name: \"Worker\",\n      description: \"A worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input: [],\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Enabled(\n        @common.SnapshottingConfig::EveryNInvocation(5U),\n      ),\n      config: [],\n    }\n    @agents.register_agent({\n      name: \"Worker\",\n      agent_type: worker_type_def.to_agent_type(),\n      construct: fn(input, principal) {\n        let _ = principal\n        try {\n          let _ = @agents.decode_invocation_input(input, 0, \"Worker::new\")\n          let agent = Worker::new()\n          let constructed = @agents.ConstructedAgent::{\n            raw: agent,\n            snapshottable: None,\n            snapshot_format: None,\n          }\n          Ok(constructed)\n        } catch{\n          e => Err(e)\n        }\n      },\n    })\n  } catch{\n    e => {\n      @agents.panic_with_message(\n        \"init\",\n        \"Failed to register agents: \" + e.to_string(),\n      )\n    }\n  }\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  let _ = principal\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"
    ),
  )
}

///|
test "parse_duration_seconds plain number" {
  debug_inspect(parse_duration_seconds("100"), content="100")
}

///|
test "parse_duration_seconds seconds" {
  debug_inspect(parse_duration_seconds("30 seconds"), content="30")
  debug_inspect(parse_duration_seconds("1 second"), content="1")
  debug_inspect(parse_duration_seconds("5 secs"), content="5")
  debug_inspect(parse_duration_seconds("10 sec"), content="10")
  debug_inspect(parse_duration_seconds("2 s"), content="2")
}

///|
test "parse_duration_seconds minutes" {
  debug_inspect(parse_duration_seconds("10 minutes"), content="600")
  debug_inspect(parse_duration_seconds("1 minute"), content="60")
  debug_inspect(parse_duration_seconds("5 mins"), content="300")
  debug_inspect(parse_duration_seconds("3 min"), content="180")
  debug_inspect(parse_duration_seconds("2 m"), content="120")
}

///|
test "parse_duration_seconds hours" {
  debug_inspect(parse_duration_seconds("2 hours"), content="7200")
  debug_inspect(parse_duration_seconds("1 hour"), content="3600")
  debug_inspect(parse_duration_seconds("3 hrs"), content="10800")
  debug_inspect(parse_duration_seconds("1 hr"), content="3600")
  debug_inspect(parse_duration_seconds("1 h"), content="3600")
}

///|
test "parse_duration_seconds days" {
  debug_inspect(parse_duration_seconds("1 day"), content="86400")
  debug_inspect(parse_duration_seconds("2 days"), content="172800")
  debug_inspect(parse_duration_seconds("1 d"), content="86400")
}

///|
test "parse_agents detects derive(ToJson, FromJson)" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|} derive(ToJson, FromJson)
    #|
    #|///| Creates a new counter
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].has_json_derives, content="true")
  debug_inspect(agents[0].has_snapshottable, content="false")
}

///|
test "parse_agents detects derive(ToJson, @json.FromJson)" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|} derive(ToJson, @json.FromJson)
    #|
    #|///| Creates a new counter
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].has_json_derives, content="true")
}

///|
test "parse_agents no json derives without derive annotation" {
  let content =
    #|///| A counter agent
    #|#derive.agent
    #|pub(all) struct Counter {
    #|  name : String
    #|  mut value : UInt64
    #|}
    #|
    #|///| Creates a new counter
    #|pub fn Counter::new(name : String) -> Counter {
    #|  { name, value: 0 }
    #|}
  let agents = parse_agents([("counter.mbt", content)])
  debug_inspect(agents.length(), content="1")
  debug_inspect(agents[0].has_json_derives, content="false")
}

///|
test "generate_agent_registration with json derives generates direct Snapshottable" {
  let agents : Array[AgentInfo] = [
    {
      name: "Counter",
      description: "A counter agent",
      mode: Durable,
      constructor_params: [("name", Simple("String"))],
      constructor_description: "Creates a new counter",
      constructor_prompt_hint: None,
      methods: [],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: true,
      snapshotting: Disabled,
      fields: [("name", false), ("value", true)],
      http_mount: None,
      config_type: None,
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  // Check that the output contains direct Snapshottable impl (not SnapshotCell)
  debug_inspect(output.contains("SnapshotCell"), content="false")
  debug_inspect(output.contains("Snapshottable for Counter"), content="true")
  debug_inspect(output.contains("snapshot_format"), content="true")
  debug_inspect(output.contains("Json"), content="true")
  // Only mut fields are assigned in load_snapshot
  debug_inspect(output.contains("self.name = new_val.name"), content="false")
  debug_inspect(output.contains("self.value = new_val.value"), content="true")
}

///|
test "parse_duration_seconds compound" {
  debug_inspect(parse_duration_seconds("1 hour 30 minutes"), content="5400")
  debug_inspect(
    parse_duration_seconds("1 day 2 hours 30 minutes 15 seconds"),
    content="95415",
  )
  debug_inspect(parse_duration_seconds("2 hours 0 seconds"), content="7200")
}

///|
test "generate_agent_registration emits HTTP endpoint + mount metadata" {
  let agents : Array[AgentInfo] = [
    {
      name: "WeatherAgent",
      description: "A weather agent",
      mode: Durable,
      constructor_params: [("city", Simple("String"))],
      constructor_description: "Creates a weather agent",
      constructor_prompt_hint: None,
      methods: [
        {
          name: "get_temperature",
          description: "Get current temperature",
          prompt_hint: None,
          params: [("unit", Simple("String"))],
          return_type: Some(Simple("Double")),
          http_endpoint: Some({
            method: "get",
            path: "/current?unit={unit}",
            headers: [],
            auth: None,
            cors: [],
          }),
          read_only: None,
        },
        {
          name: "set_temperature",
          description: "Set the temperature",
          prompt_hint: None,
          params: [
            ("temperature", Simple("Double")),
            ("source", Simple("String")),
          ],
          return_type: Some(Simple("String")),
          http_endpoint: Some({
            method: "post",
            path: "/set",
            headers: [("X-Source", "source")],
            auth: None,
            cors: [],
          }),
          read_only: None,
        },
      ],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: Some({
        path: "/api/{city}/weather",
        auth: Some(false),
        phantom: false,
        cors: ["https://app.acme.com"],
        webhook_suffix: None,
      }),
      config_type: None,
    },
  ]
  let output = @formatter.impls_to_string(generate_agent_registration(agents))
  // GET endpoint with a query variable.
  assert_true(output.contains("http_method: @common.HttpMethod::Get"))
  assert_true(
    output.contains("path_suffix: [@common.PathSegment::Literal(\"current\")]"),
  )
  assert_true(
    output.contains(
      "query_vars: [{ query_param_name: \"unit\", variable_name: \"unit\" }]",
    ),
  )
  // POST endpoint with a header variable.
  assert_true(output.contains("http_method: @common.HttpMethod::Post"))
  assert_true(
    output.contains("{ header_name: \"X-Source\", variable_name: \"source\" }"),
  )
  // Mount metadata: path prefix with a path variable, auth, and cors.
  // (the formatter appends a trailing comma to single-field record literals,
  // so match only up to the field value, not the closing brace)
  assert_true(
    output.contains(
      "@common.PathSegment::PathVariable({ variable_name: \"city\"",
    ),
  )
  assert_true(output.contains("auth_details: Some({ required: false"))
  assert_true(output.contains("phantom_agent: false"))
  assert_true(output.contains("allowed_patterns: [\"https://app.acme.com\"]"))
}

///|
test "generate_agent_registration emits remaining/system path vars, required auth, phantom mount and webhook suffix" {
  let agents : Array[AgentInfo] = [
    {
      name: "FileAgent",
      description: "A file agent",
      mode: Durable,
      constructor_params: [],
      constructor_description: "Creates a file agent",
      constructor_prompt_hint: None,
      methods: [
        {
          name: "fetch",
          description: "Fetch a file",
          prompt_hint: None,
          params: [],
          return_type: Some(Simple("String")),
          http_endpoint: Some({
            method: "get",
            path: "/files/{agent-type}/{agent-version}/{*rest}",
            headers: [],
            auth: Some(true),
            cors: [],
          }),
          read_only: None,
        },
      ],
      has_init: false,
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: Some({
        path: "/files",
        auth: Some(true),
        phantom: true,
        cors: [],
        webhook_suffix: Some("/events/{id}"),
      }),
      config_type: None,
    },
  ]
  // The formatter wraps long array literals across multiple lines, so
  // normalize whitespace before matching structural fragments.
  let output = strip_whitespace(
    @formatter.impls_to_string(generate_agent_registration(agents)),
  )
  // System path variables.
  assert_true(
    output.contains(
      "@common.PathSegment::SystemVariable(@common.SystemVariable::AGENT_TYPE",
    ),
  )
  assert_true(
    output.contains(
      "@common.PathSegment::SystemVariable(@common.SystemVariable::AGENT_VERSION",
    ),
  )
  // Remaining-path variable ({*rest}).
  assert_true(
    output.contains(
      "@common.PathSegment::RemainingPathVariable({variable_name:\"rest\"",
    ),
  )
  // Required auth on the endpoint.
  assert_true(output.contains("auth_details:Some({required:true"))
  // Phantom mount + webhook suffix path segments.
  assert_true(output.contains("phantom_agent:true"))
  assert_true(
    output.contains(
      "webhook_suffix:[@common.PathSegment::Literal(\"events\"),@common.PathSegment::PathVariable({variable_name:\"id\"",
    ),
  )
}

///|
/// Removes all ASCII whitespace from a string. Used in tests to match
/// structural fragments without depending on the formatter's line wrapping.
fn strip_whitespace(s : String) -> String {
  let sb = StringBuilder::new()
  for c in s {
    if c != ' ' && c != '\n' && c != '\t' && c != '\r' {
      sb.write_char(c)
    }
  }
  sb.to_string()
}
