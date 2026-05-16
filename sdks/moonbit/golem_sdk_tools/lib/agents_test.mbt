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
  debug_inspect(agents[0].name, content=(
    #|"Counter"
  ))
  debug_inspect(agents[0].description, content=(
    #|"A counter agent"
  ))
  debug_inspect(agents[0].constructor_description, content=(
    #|"Creates a new counter"
  ))
  debug_inspect(agents[0].constructor_params.length(), content="1")
  debug_inspect(agents[0].constructor_params[0].0, content=(
    #|"name"
  ))
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
  debug_inspect(agents[0].name, content=(
    #|"Greeter"
  ))
  debug_inspect(agents[0].constructor_params.length(), content="2")
  debug_inspect(agents[0].constructor_params[0].0, content=(
    #|"greeting"
  ))
  debug_inspect(agents[0].constructor_params[0].1, content="Simple(\"String\")")
  debug_inspect(agents[0].constructor_params[1].0, content=(
    #|"count"
  ))
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
  debug_inspect(agents[0].methods[0].name, content=(
    #|"increment"
  ))
  debug_inspect(agents[0].methods[0].description, content=(
    #|"Increments the counter"
  ))
  debug_inspect(agents[0].methods[0].params.length(), content="0")
  debug_inspect(agents[0].methods[0].return_type, content="None")
  debug_inspect(agents[0].methods[1].name, content=(
    #|"decrement"
  ))
  debug_inspect(agents[0].methods[1].description, content=(
    #|"Decrements the counter"
  ))
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
  debug_inspect(agents[0].methods[0].name, content=(
    #|"add"
  ))
  debug_inspect(agents[0].methods[0].description, content=(
    #|"Adds a value"
  ))
  debug_inspect(agents[0].methods[0].params.length(), content="1")
  debug_inspect(agents[0].methods[0].params[0].0, content=(
    #|"value"
  ))
  debug_inspect(agents[0].methods[0].params[0].1, content="Simple(\"Int\")")
  debug_inspect(agents[0].methods[0].return_type, content="Some(Simple(\"Int\"))")
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
  debug_inspect(agents[0].methods[0].name, content=(
    #|"do_thing"
  ))
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
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"Counter\",\n    agent_type: {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"name\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n        ]),\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 1 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 1 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let name :String = @schema.from_element_value_as(elements[0])\n        let agent = Counter::new(name)\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
          param_schema_options: {},
          http_endpoint: None,
        },
        {
          name: "add",
          description: "Adds a value",
          prompt_hint: None,
          params: [("value", Simple("UInt64"))],
          return_type: Some(Simple("UInt64")),
          param_schema_options: {},
          http_endpoint: None,
        },
      ],
      has_init: false,
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"Counter\",\n    agent_type: {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"name\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n        ]),\n      },\n      methods: [\n        {\n          name: \"increment\",\n          description: \"Increments the counter\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input_schema: @rpcTypes.DataSchema::Tuple([]),\n          output_schema: @rpcTypes.DataSchema::Tuple([]),\n        },\n        {\n          name: \"add\",\n          description: \"Adds a value\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input_schema: @rpcTypes.DataSchema::Tuple([\n            (\n              \"value\",\n              @schema.schema_of_tag(\n                (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n              ),\n            ),\n          ]),\n          output_schema: @rpcTypes.DataSchema::Tuple([\n            (\n              \"result\",\n              @schema.schema_of_tag(\n                (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n              ),\n            ),\n          ]),\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 1 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 1 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let name :String = @schema.from_element_value_as(elements[0])\n        let agent = Counter::new(name)\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      \"increment\" => {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        self.increment()\n        Ok(@rpcTypes.DataValue::Tuple([]))\n      }\n      \"add\" => {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 1 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 1 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let value :UInt64 = @schema.from_element_value_as(elements[0])\n        let result = self.add(value)\n        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
          param_schema_options: {},
          http_endpoint: None,
        },
      ],
      has_init: false,
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"Counter\",\n    agent_type: {\n      type_name: \"Counter\",\n      description: \"A counter agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a new counter\",\n        prompt_hint: Some(\"Provide a unique name\"),\n        input_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"name\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n        ]),\n      },\n      methods: [\n        {\n          name: \"increment\",\n          description: \"Increments the counter\",\n          http_endpoint: [],\n          prompt_hint: Some(\"Call to increase\"),\n          input_schema: @rpcTypes.DataSchema::Tuple([]),\n          output_schema: @rpcTypes.DataSchema::Tuple([]),\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 1 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 1 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let name :String = @schema.from_element_value_as(elements[0])\n        let agent = Counter::new(name)\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      \"increment\" => {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        self.increment()\n        Ok(@rpcTypes.DataValue::Tuple([]))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

    ),
  )
}

///|
fn format_expr(expr : @syntax.Expr) -> String {
  let impl_ = @syntax.Impl::TopExpr(
    expr~,
    is_main=false,
    local_types=@list.List::from_array([]),
    is_async=None,
    loc=@basic.Location::{
      start: { fname: "", lnum: 0, bol: 0, cnum: 0 },
      end: { fname: "", lnum: 0, bol: 0, cnum: 0 },
    },
  )
  let output = @formatter.impls_to_string(@list.List::from_array([impl_]))
  // Strip the wrapping "{\n" and "}\n\n" from the init block output
  output
}

///|
fn parse_and_build_agent_type(files : Array[(String, String)]) -> String raise {
  let agents = parse_agents(files)
  guard agents.length() == 1 else {
    fail("Expected exactly 1 agent, got \{agents.length()}")
  }
  let expr = build_agent_type_expr(agents[0])
  format_expr(expr)
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
      #|"fn init{\n  {\n    type_name: \"Counter\",\n    description: \"A counter agent\",\n    source_language: \"moonbit\",\n    constructor_: {\n      name: Some(\"new\"),\n      description: \"Creates a new counter\",\n      prompt_hint: None,\n      input_schema: @rpcTypes.DataSchema::Tuple([\n        (\n          \"name\",\n          @schema.schema_of_tag(\n            (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n          ),\n        ),\n      ]),\n    },\n    methods: [],\n    dependencies: [],\n    mode: @common.AgentMode::DURABLE,\n    http_mount: None,\n    snapshotting: @common.Snapshotting::Disabled,\n    config: [],\n  }\n}\n\n"

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
      #|"fn init{\n  {\n    type_name: \"Counter\",\n    description: \"A counter agent\",\n    source_language: \"moonbit\",\n    constructor_: {\n      name: Some(\"new\"),\n      description: \"Creates a new counter\",\n      prompt_hint: None,\n      input_schema: @rpcTypes.DataSchema::Tuple([\n        (\n          \"name\",\n          @schema.schema_of_tag(\n            (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n          ),\n        ),\n      ]),\n    },\n    methods: [\n      {\n        name: \"increment\",\n        description: \"Increments the counter by one\",\n        http_endpoint: [],\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([]),\n        output_schema: @rpcTypes.DataSchema::Tuple([]),\n      },\n      {\n        name: \"add\",\n        description: \"Adds a value to the counter\",\n        http_endpoint: [],\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"value\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n        ]),\n        output_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"result\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n        ]),\n      },\n    ],\n    dependencies: [],\n    mode: @common.AgentMode::DURABLE,\n    http_mount: None,\n    snapshotting: @common.Snapshotting::Disabled,\n    config: [],\n  }\n}\n\n"

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
      #|"fn init{\n  {\n    type_name: \"SmartAgent\",\n    description: \"A smart agent\",\n    source_language: \"moonbit\",\n    constructor_: {\n      name: Some(\"new\"),\n      description: \"Creates a smart agent\",\n      prompt_hint: Some(\"Provide a JSON config string\"),\n      input_schema: @rpcTypes.DataSchema::Tuple([\n        (\n          \"config\",\n          @schema.schema_of_tag(\n            (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n          ),\n        ),\n      ]),\n    },\n    methods: [\n      {\n        name: \"process\",\n        description: \"Processes input text\",\n        http_endpoint: [],\n        prompt_hint: Some(\"Pass natural language text\"),\n        input_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"text\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n        ]),\n        output_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"result\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n            ),\n          ),\n        ]),\n      },\n    ],\n    dependencies: [],\n    mode: @common.AgentMode::DURABLE,\n    http_mount: None,\n    snapshotting: @common.Snapshotting::Disabled,\n    config: [],\n  }\n}\n\n"

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
      #|"fn init{\n  {\n    type_name: \"Processor\",\n    description: \"A data processor\",\n    source_language: \"moonbit\",\n    constructor_: {\n      name: Some(\"new\"),\n      description: \"Creates a new Processor\",\n      prompt_hint: None,\n      input_schema: @rpcTypes.DataSchema::Tuple([\n        (\n          \"name\",\n          @schema.schema_of_tag(\n            (@schema.TypeTag::{  } : @schema.TypeTag[String]),\n          ),\n        ),\n      ]),\n    },\n    methods: [\n      {\n        name: \"batch\",\n        description: \"Processes a batch of items\",\n        http_endpoint: [],\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"items\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),\n            ),\n          ),\n          (\n            \"count\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64?]),\n            ),\n          ),\n        ]),\n        output_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"result\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),\n            ),\n          ),\n        ]),\n      },\n    ],\n    dependencies: [],\n    mode: @common.AgentMode::DURABLE,\n    http_mount: None,\n    snapshotting: @common.Snapshotting::Disabled,\n    config: [],\n  }\n}\n\n"

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
      #|"fn init{\n  {\n    type_name: \"Worker\",\n    description: \"A simple worker\",\n    source_language: \"moonbit\",\n    constructor_: {\n      name: Some(\"new\"),\n      description: \"Creates a default worker\",\n      prompt_hint: None,\n      input_schema: @rpcTypes.DataSchema::Tuple([]),\n    },\n    methods: [\n      {\n        name: \"run\",\n        description: \"Does work\",\n        http_endpoint: [],\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([]),\n        output_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"result\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[Int]),\n            ),\n          ),\n        ]),\n      },\n    ],\n    dependencies: [],\n    mode: @common.AgentMode::DURABLE,\n    http_mount: None,\n    snapshotting: @common.Snapshotting::Disabled,\n    config: [],\n  }\n}\n\n"

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
      #|"fn init{\n  {\n    type_name: \"MultiAgent\",\n    description: \"A multi-file agent\",\n    source_language: \"moonbit\",\n    constructor_: {\n      name: Some(\"new\"),\n      description: \"Creates a new MultiAgent\",\n      prompt_hint: None,\n      input_schema: @rpcTypes.DataSchema::Tuple([\n        (\n          \"id\",\n          @schema.schema_of_tag(\n            (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n          ),\n        ),\n      ]),\n    },\n    methods: [\n      {\n        name: \"get_id\",\n        description: \"Gets the ID\",\n        http_endpoint: [],\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([]),\n        output_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"result\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),\n            ),\n          ),\n        ]),\n      },\n    ],\n    dependencies: [],\n    mode: @common.AgentMode::DURABLE,\n    http_mount: None,\n    snapshotting: @common.Snapshotting::Disabled,\n    config: [],\n  }\n}\n\n"

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
        param_schema_options: {},
        http_endpoint: None,
      },
      {
        name: "get_value",
        description: "Gets the current value",
        prompt_hint: None,
        params: [],
        return_type: Some(Simple("UInt64")),
        param_schema_options: {},
        http_endpoint: None,
      },
      {
        name: "add",
        description: "Adds a value",
        prompt_hint: None,
        params: [("value", Simple("UInt64"))],
        return_type: Some(Simple("UInt64")),
        param_schema_options: {},
        http_endpoint: None,
      },
    ],
    has_init: false,
    constructor_schema_options: {},
    has_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: None,
  }
  let impl_ = build_raw_agent_impl(agent)
  let output = @formatter.impls_to_string(@list.List::from_array([impl_]))
  debug_inspect(
    output,
    content=(
      #|"///|\nimpl @agents.RawAgent for Counter with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      \"increment\" => {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        self.increment()\n        Ok(@rpcTypes.DataValue::Tuple([]))\n      }\n      \"get_value\" => {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let result = self.get_value()\n        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))\n      }\n      \"add\" => {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 1 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 1 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let value :UInt64 = @schema.from_element_value_as(elements[0])\n        let result = self.add(value)\n        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
        debug_inspect(decode_qualified(name), content="Some((\"\{pkg}\", \"\{id}\"))")
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
  debug_inspect(agents[0].methods[0].params[0].1, content="Simple(\"@types.Order\")")
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
  debug_inspect(agents[0].methods[0].params[0].1, content="Simple(\"UserName\")")
  debug_inspect(agents[0].methods[0].return_type, content="Some(Simple(\"UserId\"))")
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
  debug_inspect(param_type_to_type_string(Simple("String")), content=(
    #|"String"
  ))
  debug_inspect(param_type_to_type_string(Simple("UInt64")), content=(
    #|"UInt64"
  ))
  debug_inspect(
    param_type_to_type_string(Optional(Simple("String"))),
    content=(
      #|"String?"
    ),
  )
  debug_inspect(param_type_to_type_string(List(Simple("Int"))), content=(
    #|"Array[Int]"
  ))
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
          param_schema_options: {},
          http_endpoint: None,
        },
      ],
      has_init: false,
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"OrderAgent\",\n    agent_type: {\n      type_name: \"OrderAgent\",\n      description: \"An order agent\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates an order agent\",\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([\n          (\n            \"config\",\n            @schema.schema_of_tag(\n              (@schema.TypeTag::{  } : @schema.TypeTag[@types.Config]),\n            ),\n          ),\n        ]),\n      },\n      methods: [\n        {\n          name: \"process\",\n          description: \"Processes an order\",\n          http_endpoint: [],\n          prompt_hint: None,\n          input_schema: @rpcTypes.DataSchema::Tuple([\n            (\n              \"order\",\n              @schema.schema_of_tag(\n                (@schema.TypeTag::{  } : @schema.TypeTag[@types.Order]),\n              ),\n            ),\n          ]),\n          output_schema: @rpcTypes.DataSchema::Tuple([\n            (\n              \"result\",\n              @schema.schema_of_tag(\n                (@schema.TypeTag::{  } : @schema.TypeTag[@types.Result]),\n              ),\n            ),\n          ]),\n        },\n      ],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 1 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 1 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let config :@types.Config = @schema.from_element_value_as(elements[0])\n        let agent = OrderAgent::new(config)\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for OrderAgent with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      \"process\" => {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 1 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 1 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let order :@types.Order = @schema.from_element_value_as(elements[0])\n        let result = self.process(order)\n        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
  debug_inspect(agents[0].methods[0].name, content=(
    #|"get_pair"
  ))
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
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"Worker\",\n    agent_type: {\n      type_name: \"Worker\",\n      description: \"An ephemeral worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([]),\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::EPHEMERAL,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Disabled,\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let agent = Worker::new()\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
test "generate_agent_registration with multimodal return uses to_data_value" {
  let agents : Array[AgentInfo] = [
    {
      name: "EchoAgent",
      description: "Echo agent",
      mode: Durable,
      constructor_params: [],
      constructor_description: "Creates an echo agent",
      constructor_prompt_hint: None,
      methods: [
        {
          name: "echo",
          description: "Echo multimodal",
          prompt_hint: None,
          params: [("input", MultimodalType(Simple("TextOrBinary")))],
          return_type: Some(MultimodalType(Simple("TextOrBinary"))),
          param_schema_options: {},
          http_endpoint: None,
        },
      ],
      has_init: false,
      constructor_schema_options: {},
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
      http_mount: None,
      config_type: None,
    },
  ]
  let impl_ = build_raw_agent_impl(agents[0])
  let output = @formatter.impls_to_string(@list.List::from_array([impl_]))
  debug_inspect(
    output,
    content=(
      #|"///|\nimpl @agents.RawAgent for EchoAgent with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      \"echo\" => {\n        let input :@types.Multimodal[TextOrBinary] = \n          @types.Multimodal::from_data_value(input)\n        let result = self.echo(input)\n        Ok(result.to_data_value())\n      }\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

    ),
  )
}

///|
test "parse_agents with text_languages annotation" {
  let content =
    #|///| A text agent
    #|#derive.agent
    #|pub(all) struct TextAgent {
    #|  name : String
    #|}
    #|
    #|pub fn TextAgent::new(name : String) -> TextAgent {
    #|  { name }
    #|}
    #|
    #|///| Summarize text
    #|#derive.text_languages("input", "en", "de")
    #|pub fn TextAgent::summarize(self : Self, input : UnstructuredText) -> String {
    #|  ""
    #|}
  let agents = parse_agents([("agent.mbt", content)])
  debug_inspect(
    agents[0].methods[0].param_schema_options.get("input"),
    content=(
      #|Some({ text_languages: ["en", "de"], binary_mime_types: [] })
    ),
  )
}

///|
test "parse_agents rejects restriction on unknown param" {
  let content =
    #|///| A bad agent
    #|#derive.agent
    #|pub(all) struct BadAgent {
    #|  name : String
    #|}
    #|
    #|pub fn BadAgent::new(name : String) -> BadAgent {
    #|  { name }
    #|}
    #|
    #|#derive.text_languages("nonexistent", "en")
    #|pub fn BadAgent::process(self : Self, input : UnstructuredText) -> String {
    #|  ""
    #|}
  let result : Result[_, _] = try? parse_agents([("agent.mbt", content)])
  debug_inspect(
    result,
    content=(
      #|Err(AgentParseError("#derive.text_languages/#derive.mime_types references unknown parameter 'nonexistent' of method 'process'"))
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
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"Worker\",\n    agent_type: {\n      type_name: \"Worker\",\n      description: \"A worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([]),\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Enabled(\n        @common.SnapshottingConfig::Periodic(100UL),\n      ),\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let agent = Worker::new()\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"Worker\",\n    agent_type: {\n      type_name: \"Worker\",\n      description: \"A worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([]),\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Enabled(\n        @common.SnapshottingConfig::Default,\n      ),\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let agent = Worker::new()\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
      constructor_schema_options: {},
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
      #|"fn init{\n  @agents.register_agent({\n    name: \"Worker\",\n    agent_type: {\n      type_name: \"Worker\",\n      description: \"A worker\",\n      source_language: \"moonbit\",\n      constructor_: {\n        name: Some(\"new\"),\n        description: \"Creates a worker\",\n        prompt_hint: None,\n        input_schema: @rpcTypes.DataSchema::Tuple([]),\n      },\n      methods: [],\n      dependencies: [],\n      mode: @common.AgentMode::DURABLE,\n      http_mount: None,\n      snapshotting: @common.Snapshotting::Enabled(\n        @common.SnapshottingConfig::EveryNInvocation(5U),\n      ),\n      config: [],\n    },\n    construct: fn(input, principal) {\n      try {\n        let elements = @extractor.extract_tuple(input)\n        guard elements.length() == 0 else {\n          raise  @common.AgentError::InvalidInput(\n            \"Expected 0 elements, got \" + elements.length().to_string(),\n          )\n        }\n        let agent = Worker::new()\n        let constructed = @agents.ConstructedAgent::{\n          raw: agent,\n          snapshottable: None,\n          snapshot_format: None,\n        }\n        Ok(constructed)\n      } catch{\n        e => Err(e)\n      }\n    },\n  })\n}\n\n///|\nimpl @agents.RawAgent for Worker with invoke(\n  self,\n  method_name,\n  input,\n  principal,\n) {\n  try {\n    match method_name {\n      other => Err(@common.AgentError::InvalidMethod(other))\n    }\n  } catch{\n    e => Err(e)\n  }\n}\n\n"

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
      constructor_schema_options: {},
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
