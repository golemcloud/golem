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
  inspect(agents.length(), content="1")
  inspect(agents[0].name, content="Counter")
  inspect(agents[0].description, content="A counter agent")
  inspect(agents[0].constructor_description, content="Creates a new counter")
  inspect(agents[0].constructor_params.length(), content="1")
  inspect(agents[0].constructor_params[0].0, content="name")
  inspect(agents[0].constructor_params[0].1, content="Simple(\"String\")")
  inspect(agents[0].methods.length(), content="0")
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
  inspect(agents.length(), content="1")
  inspect(agents[0].name, content="Greeter")
  inspect(agents[0].constructor_params.length(), content="2")
  inspect(agents[0].constructor_params[0].0, content="greeting")
  inspect(agents[0].constructor_params[0].1, content="Simple(\"String\")")
  inspect(agents[0].constructor_params[1].0, content="count")
  inspect(agents[0].constructor_params[1].1, content="Simple(\"UInt64\")")
  inspect(agents[0].methods.length(), content="0")
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
  inspect(agents[0].methods.length(), content="2")
  inspect(agents[0].methods[0].name, content="increment")
  inspect(agents[0].methods[0].description, content="Increments the counter")
  inspect(agents[0].methods[0].params.length(), content="0")
  inspect(agents[0].methods[0].return_type, content="None")
  inspect(agents[0].methods[1].name, content="decrement")
  inspect(agents[0].methods[1].description, content="Decrements the counter")
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
  inspect(agents[0].methods.length(), content="1")
  inspect(agents[0].methods[0].name, content="add")
  inspect(agents[0].methods[0].description, content="Adds a value")
  inspect(agents[0].methods[0].params.length(), content="1")
  inspect(agents[0].methods[0].params[0].0, content="value")
  inspect(agents[0].methods[0].params[0].1, content="Simple(\"Int\")")
  inspect(agents[0].methods[0].return_type, content="Some(Simple(\"Int\"))")
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
  inspect(agents[0].methods.length(), content="1")
  inspect(agents[0].methods[0].name, content="do_thing")
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
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "Counter",
      #|    agent_type: {
      #|      type_name: "Counter",
      #|      description: "A counter agent",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a new counter",
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "name",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|      methods: [],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Disabled,
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 1 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 1 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let name :String = @schema.from_element_value_as(elements[0])
      #|        let agent = Counter::new(name)
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for Counter with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
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
        },
        {
          name: "add",
          description: "Adds a value",
          prompt_hint: None,
          params: [("value", Simple("UInt64"))],
          return_type: Some(Simple("UInt64")),
          param_schema_options: {},
        },
      ],
      has_init: false,
      constructor_schema_options: {},
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "Counter",
      #|    agent_type: {
      #|      type_name: "Counter",
      #|      description: "A counter agent",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a new counter",
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "name",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|      methods: [
      #|        {
      #|          name: "increment",
      #|          description: "Increments the counter",
      #|          http_endpoint: [],
      #|          prompt_hint: None,
      #|          input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|          output_schema: @rpcTypes.DataSchema::Tuple([]),
      #|        },
      #|        {
      #|          name: "add",
      #|          description: "Adds a value",
      #|          http_endpoint: [],
      #|          prompt_hint: None,
      #|          input_schema: @rpcTypes.DataSchema::Tuple([
      #|            (
      #|              "value",
      #|              @schema.schema_of_tag(
      #|                (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|              ),
      #|            ),
      #|          ]),
      #|          output_schema: @rpcTypes.DataSchema::Tuple([
      #|            (
      #|              "result",
      #|              @schema.schema_of_tag(
      #|                (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|              ),
      #|            ),
      #|          ]),
      #|        },
      #|      ],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Disabled,
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 1 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 1 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let name :String = @schema.from_element_value_as(elements[0])
      #|        let agent = Counter::new(name)
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for Counter with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      "increment" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        self.increment()
      #|        Ok(@rpcTypes.DataValue::Tuple([]))
      #|      }
      #|      "add" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 1 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 1 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let value :UInt64 = @schema.from_element_value_as(elements[0])
      #|        let result = self.add(value)
      #|        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))
      #|      }
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    agents[0].constructor_prompt_hint,
    content="Some(\"Provide a unique name for the counter\")",
  )
  inspect(agents[0].methods.length(), content="2")
  inspect(
    agents[0].methods[0].prompt_hint,
    content="Some(\"Call this to increase the value by one\")",
  )
  inspect(agents[0].methods[1].prompt_hint, content="None")
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
        },
      ],
      has_init: false,
      constructor_schema_options: {},
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "Counter",
      #|    agent_type: {
      #|      type_name: "Counter",
      #|      description: "A counter agent",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a new counter",
      #|        prompt_hint: Some("Provide a unique name"),
      #|        input_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "name",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|      methods: [
      #|        {
      #|          name: "increment",
      #|          description: "Increments the counter",
      #|          http_endpoint: [],
      #|          prompt_hint: Some("Call to increase"),
      #|          input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|          output_schema: @rpcTypes.DataSchema::Tuple([]),
      #|        },
      #|      ],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Disabled,
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 1 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 1 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let name :String = @schema.from_element_value_as(elements[0])
      #|        let agent = Counter::new(name)
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for Counter with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      "increment" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        self.increment()
      #|        Ok(@rpcTypes.DataValue::Tuple([]))
      #|      }
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|fn init{
      #|  {
      #|    type_name: "Counter",
      #|    description: "A counter agent",
      #|    source_language: "moonbit",
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new counter",
      #|      prompt_hint: None,
      #|      input_schema: @rpcTypes.DataSchema::Tuple([
      #|        (
      #|          "name",
      #|          @schema.schema_of_tag(
      #|            (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|          ),
      #|        ),
      #|      ]),
      #|    },
      #|    methods: [],
      #|    dependencies: [],
      #|    mode: @common.AgentMode::DURABLE,
      #|    http_mount: None,
      #|    snapshotting: @common.Snapshotting::Disabled,
      #|    config: [],
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|fn init{
      #|  {
      #|    type_name: "Counter",
      #|    description: "A counter agent",
      #|    source_language: "moonbit",
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new counter",
      #|      prompt_hint: None,
      #|      input_schema: @rpcTypes.DataSchema::Tuple([
      #|        (
      #|          "name",
      #|          @schema.schema_of_tag(
      #|            (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|          ),
      #|        ),
      #|      ]),
      #|    },
      #|    methods: [
      #|      {
      #|        name: "increment",
      #|        description: "Increments the counter by one",
      #|        http_endpoint: [],
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|        output_schema: @rpcTypes.DataSchema::Tuple([]),
      #|      },
      #|      {
      #|        name: "add",
      #|        description: "Adds a value to the counter",
      #|        http_endpoint: [],
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "value",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|            ),
      #|          ),
      #|        ]),
      #|        output_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "result",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|    ],
      #|    dependencies: [],
      #|    mode: @common.AgentMode::DURABLE,
      #|    http_mount: None,
      #|    snapshotting: @common.Snapshotting::Disabled,
      #|    config: [],
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|fn init{
      #|  {
      #|    type_name: "SmartAgent",
      #|    description: "A smart agent",
      #|    source_language: "moonbit",
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a smart agent",
      #|      prompt_hint: Some("Provide a JSON config string"),
      #|      input_schema: @rpcTypes.DataSchema::Tuple([
      #|        (
      #|          "config",
      #|          @schema.schema_of_tag(
      #|            (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|          ),
      #|        ),
      #|      ]),
      #|    },
      #|    methods: [
      #|      {
      #|        name: "process",
      #|        description: "Processes input text",
      #|        http_endpoint: [],
      #|        prompt_hint: Some("Pass natural language text"),
      #|        input_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "text",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|            ),
      #|          ),
      #|        ]),
      #|        output_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "result",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|    ],
      #|    dependencies: [],
      #|    mode: @common.AgentMode::DURABLE,
      #|    http_mount: None,
      #|    snapshotting: @common.Snapshotting::Disabled,
      #|    config: [],
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|fn init{
      #|  {
      #|    type_name: "Processor",
      #|    description: "A data processor",
      #|    source_language: "moonbit",
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new Processor",
      #|      prompt_hint: None,
      #|      input_schema: @rpcTypes.DataSchema::Tuple([
      #|        (
      #|          "name",
      #|          @schema.schema_of_tag(
      #|            (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|          ),
      #|        ),
      #|      ]),
      #|    },
      #|    methods: [
      #|      {
      #|        name: "batch",
      #|        description: "Processes a batch of items",
      #|        http_endpoint: [],
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "items",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),
      #|            ),
      #|          ),
      #|          (
      #|            "count",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64?]),
      #|            ),
      #|          ),
      #|        ]),
      #|        output_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "result",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[Array[String]]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|    ],
      #|    dependencies: [],
      #|    mode: @common.AgentMode::DURABLE,
      #|    http_mount: None,
      #|    snapshotting: @common.Snapshotting::Disabled,
      #|    config: [],
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|fn init{
      #|  {
      #|    type_name: "Worker",
      #|    description: "A simple worker",
      #|    source_language: "moonbit",
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a default worker",
      #|      prompt_hint: None,
      #|      input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|    },
      #|    methods: [
      #|      {
      #|        name: "run",
      #|        description: "Does work",
      #|        http_endpoint: [],
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|        output_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "result",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[Int]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|    ],
      #|    dependencies: [],
      #|    mode: @common.AgentMode::DURABLE,
      #|    http_mount: None,
      #|    snapshotting: @common.Snapshotting::Disabled,
      #|    config: [],
      #|  }
      #|}
      #|
      #|
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
  inspect(
    output,
    content=(
      #|fn init{
      #|  {
      #|    type_name: "MultiAgent",
      #|    description: "A multi-file agent",
      #|    source_language: "moonbit",
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new MultiAgent",
      #|      prompt_hint: None,
      #|      input_schema: @rpcTypes.DataSchema::Tuple([
      #|        (
      #|          "id",
      #|          @schema.schema_of_tag(
      #|            (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|          ),
      #|        ),
      #|      ]),
      #|    },
      #|    methods: [
      #|      {
      #|        name: "get_id",
      #|        description: "Gets the ID",
      #|        http_endpoint: [],
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|        output_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "result",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|    ],
      #|    dependencies: [],
      #|    mode: @common.AgentMode::DURABLE,
      #|    http_mount: None,
      #|    snapshotting: @common.Snapshotting::Disabled,
      #|    config: [],
      #|  }
      #|}
      #|
      #|
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
      },
      {
        name: "get_value",
        description: "Gets the current value",
        prompt_hint: None,
        params: [],
        return_type: Some(Simple("UInt64")),
        param_schema_options: {},
      },
      {
        name: "add",
        description: "Adds a value",
        prompt_hint: None,
        params: [("value", Simple("UInt64"))],
        return_type: Some(Simple("UInt64")),
        param_schema_options: {},
      },
    ],
    has_init: false,
    constructor_schema_options: {},
    has_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
  }
  let impl_ = build_raw_agent_impl(agent)
  let output = @formatter.impls_to_string(@list.List::from_array([impl_]))
  inspect(
    output,
    content=(
      #|///|
      #|impl @agents.RawAgent for Counter with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      "increment" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        self.increment()
      #|        Ok(@rpcTypes.DataValue::Tuple([]))
      #|      }
      #|      "get_value" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let result = self.get_value()
      #|        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))
      #|      }
      #|      "add" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 1 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 1 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let value :UInt64 = @schema.from_element_value_as(elements[0])
      #|        let result = self.add(value)
      #|        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))
      #|      }
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
    ),
  )
}

///|
test "encode_qualified creates Simple with @pkg.id format" {
  inspect(
    encode_qualified("types", "Config"),
    content="Simple(\"@types.Config\")",
  )
  inspect(
    encode_qualified("my_pkg", "MyType"),
    content="Simple(\"@my_pkg.MyType\")",
  )
}

///|
test "decode_qualified parses @pkg.id format" {
  inspect(
    decode_qualified("@types.Config"),
    content="Some((\"types\", \"Config\"))",
  )
  inspect(
    decode_qualified("@my_pkg.MyType"),
    content="Some((\"my_pkg\", \"MyType\"))",
  )
  inspect(decode_qualified("String"), content="None")
  inspect(decode_qualified("UserId"), content="None")
  inspect(decode_qualified("@"), content="None")
  inspect(decode_qualified("@."), content="None")
  inspect(decode_qualified("@pkg."), content="None")
  inspect(decode_qualified("@.Type"), content="None")
}

///|
test "encode/decode_qualified roundtrips" {
  let cases = [("types", "Config"), ("my_pkg", "MyType"), ("a", "B")]
  for pair in cases {
    let (pkg, id) = pair
    let encoded = encode_qualified(pkg, id)
    match encoded {
      Simple(name) =>
        inspect(decode_qualified(name), content="Some((\"\{pkg}\", \"\{id}\"))")
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
  inspect(agents.length(), content="1")
  inspect(agents[0].constructor_params.length(), content="1")
  inspect(
    agents[0].constructor_params[0].1,
    content="Simple(\"@types.Config\")",
  )
  inspect(agents[0].methods.length(), content="1")
  inspect(agents[0].methods[0].params[0].1, content="Simple(\"@types.Order\")")
  inspect(
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
  inspect(agents.length(), content="1")
  inspect(agents[0].constructor_params[0].1, content="Simple(\"UserId\")")
  inspect(agents[0].methods[0].params[0].1, content="Simple(\"UserName\")")
  inspect(agents[0].methods[0].return_type, content="Some(Simple(\"UserId\"))")
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
  inspect(agents.length(), content="1")
  inspect(
    agents[0].methods[0].return_type,
    content="Some(Optional(Simple(\"@types.Info\")))",
  )
  inspect(
    agents[0].methods[1].return_type,
    content="Some(ResultType(Simple(\"@types.Data\"), Simple(\"@types.Error\")))",
  )
}

///|
test "param_type_to_type_string for various types" {
  inspect(param_type_to_type_string(Simple("String")), content="String")
  inspect(param_type_to_type_string(Simple("UInt64")), content="UInt64")
  inspect(
    param_type_to_type_string(Optional(Simple("String"))),
    content="String?",
  )
  inspect(param_type_to_type_string(List(Simple("Int"))), content="Array[Int]")
  inspect(
    param_type_to_type_string(ResultType(Simple("String"), Simple("Int"))),
    content="Result[String, Int]",
  )
  inspect(
    param_type_to_type_string(Optional(List(Simple("String")))),
    content="Array[String]?",
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
        },
      ],
      has_init: false,
      constructor_schema_options: {},
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "OrderAgent",
      #|    agent_type: {
      #|      type_name: "OrderAgent",
      #|      description: "An order agent",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates an order agent",
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([
      #|          (
      #|            "config",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[@types.Config]),
      #|            ),
      #|          ),
      #|        ]),
      #|      },
      #|      methods: [
      #|        {
      #|          name: "process",
      #|          description: "Processes an order",
      #|          http_endpoint: [],
      #|          prompt_hint: None,
      #|          input_schema: @rpcTypes.DataSchema::Tuple([
      #|            (
      #|              "order",
      #|              @schema.schema_of_tag(
      #|                (@schema.TypeTag::{  } : @schema.TypeTag[@types.Order]),
      #|              ),
      #|            ),
      #|          ]),
      #|          output_schema: @rpcTypes.DataSchema::Tuple([
      #|            (
      #|              "result",
      #|              @schema.schema_of_tag(
      #|                (@schema.TypeTag::{  } : @schema.TypeTag[@types.Result]),
      #|              ),
      #|            ),
      #|          ]),
      #|        },
      #|      ],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Disabled,
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 1 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 1 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let config :@types.Config = @schema.from_element_value_as(elements[0])
      #|        let agent = OrderAgent::new(config)
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for OrderAgent with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      "process" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 1 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 1 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let order :@types.Order = @schema.from_element_value_as(elements[0])
      #|        let result = self.process(order)
      #|        Ok(@rpcTypes.DataValue::Tuple([@schema.to_element_value_as(result)]))
      #|      }
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
    ),
  )
}

///|
test "param_type_to_type_string for cross-package types" {
  inspect(
    param_type_to_type_string(Simple("@types.Config")),
    content="@types.Config",
  )
  inspect(
    param_type_to_type_string(Optional(Simple("@types.Info"))),
    content="@types.Info?",
  )
  inspect(
    param_type_to_type_string(List(Simple("@types.Item"))),
    content="Array[@types.Item]",
  )
  inspect(
    param_type_to_type_string(
      ResultType(Simple("@types.Data"), Simple("@types.Error")),
    ),
    content="Result[@types.Data, @types.Error]",
  )
}

///|
test "param_type_to_type_string for tuples" {
  inspect(
    param_type_to_type_string(Tuple([Simple("String"), Simple("Int")])),
    content="(String, Int)",
  )
  inspect(
    param_type_to_type_string(
      Tuple([Simple("String"), Simple("Int"), Simple("Bool")]),
    ),
    content="(String, Int, Bool)",
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
  inspect(agents[0].methods.length(), content="1")
  inspect(agents[0].methods[0].name, content="get_pair")
  inspect(
    agents[0].methods[0].return_type,
    content=(
      #|Some(Tuple([Simple("String"), Simple("Int")]))
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
  inspect(agents[0].mode, content="Durable")
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
  inspect(agents[0].mode, content="Durable")
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
  inspect(agents[0].mode, content="Ephemeral")
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
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "Worker",
      #|    agent_type: {
      #|      type_name: "Worker",
      #|      description: "An ephemeral worker",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a worker",
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|      },
      #|      methods: [],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::EPHEMERAL,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Disabled,
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let agent = Worker::new()
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for Worker with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    m.params,
    content=(
      #|[("input", Parameterized("Multimodal", [Simple("TextOrBinary")]))]
    ),
  )
  inspect(
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
        },
      ],
      has_init: false,
      constructor_schema_options: {},
      has_snapshottable: false,
      has_json_derives: false,
      snapshotting: Disabled,
      fields: [],
    },
  ]
  let impl_ = build_raw_agent_impl(agents[0])
  let output = @formatter.impls_to_string(@list.List::from_array([impl_]))
  inspect(
    output,
    content=(
      #|///|
      #|impl @agents.RawAgent for EchoAgent with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      "echo" => {
      #|        let input :@types.Multimodal[TextOrBinary] = 
      #|          @types.Multimodal::from_data_value(input)
      #|        let result = self.echo(input)
      #|        Ok(result.to_data_value())
      #|      }
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
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
  inspect(
    agents[0].methods[0].param_schema_options.get("input"),
    content="Some({text_languages: [\"en\", \"de\"], binary_mime_types: []})",
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
  inspect(
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
  inspect(agents.length(), content="1")
  inspect(agents[0].has_snapshottable, content="true")
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
  inspect(agents.length(), content="1")
  inspect(agents[0].has_snapshottable, content="false")
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
  inspect(agents.length(), content="1")
  inspect(agents[0].has_snapshottable, content="true")
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
  inspect(agents[0].snapshotting, content="Disabled")
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
  inspect(agents[0].snapshotting, content="Default")
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
  inspect(agents[0].snapshotting, content="Periodic(100)")
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
  inspect(agents[0].snapshotting, content="EveryNInvocation(5)")
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
  inspect(agents[0].mode, content="Ephemeral")
  inspect(agents[0].snapshotting, content="Periodic(50)")
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
  inspect(agents[0].snapshotting, content="Periodic(600)")
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
  inspect(agents[0].snapshotting, content="Periodic(5400)")
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
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "Worker",
      #|    agent_type: {
      #|      type_name: "Worker",
      #|      description: "A worker",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a worker",
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|      },
      #|      methods: [],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Enabled(
      #|        @common.SnapshottingConfig::Periodic(100UL),
      #|      ),
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let agent = Worker::new()
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for Worker with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
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
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "Worker",
      #|    agent_type: {
      #|      type_name: "Worker",
      #|      description: "A worker",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a worker",
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|      },
      #|      methods: [],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Enabled(
      #|        @common.SnapshottingConfig::Default,
      #|      ),
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let agent = Worker::new()
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for Worker with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
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
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|fn init{
      #|  @agents.register_agent({
      #|    name: "Worker",
      #|    agent_type: {
      #|      type_name: "Worker",
      #|      description: "A worker",
      #|      source_language: "moonbit",
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a worker",
      #|        prompt_hint: None,
      #|        input_schema: @rpcTypes.DataSchema::Tuple([]),
      #|      },
      #|      methods: [],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
      #|      http_mount: None,
      #|      snapshotting: @common.Snapshotting::Enabled(
      #|        @common.SnapshottingConfig::EveryNInvocation(5U),
      #|      ),
      #|      config: [],
      #|    },
      #|    construct: fn(input) {
      #|      try {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let agent = Worker::new()
      #|        let constructed = @agents.ConstructedAgent::{
      #|          raw: agent,
      #|          snapshottable: None,
      #|          snapshot_format: None,
      #|        }
      #|        Ok(constructed)
      #|      } catch{
      #|        e => Err(e)
      #|      }
      #|    },
      #|  })
      #|}
      #|
      #|///|
      #|impl @agents.RawAgent for Worker with invoke(self, method_name, input) {
      #|  try {
      #|    match method_name {
      #|      other => Err(@common.AgentError::InvalidMethod(other))
      #|    }
      #|  } catch{
      #|    e => Err(e)
      #|  }
      #|}
      #|
      #|
    ),
  )
}

///|
test "parse_duration_seconds plain number" {
  inspect(parse_duration_seconds("100"), content="100")
}

///|
test "parse_duration_seconds seconds" {
  inspect(parse_duration_seconds("30 seconds"), content="30")
  inspect(parse_duration_seconds("1 second"), content="1")
  inspect(parse_duration_seconds("5 secs"), content="5")
  inspect(parse_duration_seconds("10 sec"), content="10")
  inspect(parse_duration_seconds("2 s"), content="2")
}

///|
test "parse_duration_seconds minutes" {
  inspect(parse_duration_seconds("10 minutes"), content="600")
  inspect(parse_duration_seconds("1 minute"), content="60")
  inspect(parse_duration_seconds("5 mins"), content="300")
  inspect(parse_duration_seconds("3 min"), content="180")
  inspect(parse_duration_seconds("2 m"), content="120")
}

///|
test "parse_duration_seconds hours" {
  inspect(parse_duration_seconds("2 hours"), content="7200")
  inspect(parse_duration_seconds("1 hour"), content="3600")
  inspect(parse_duration_seconds("3 hrs"), content="10800")
  inspect(parse_duration_seconds("1 hr"), content="3600")
  inspect(parse_duration_seconds("1 h"), content="3600")
}

///|
test "parse_duration_seconds days" {
  inspect(parse_duration_seconds("1 day"), content="86400")
  inspect(parse_duration_seconds("2 days"), content="172800")
  inspect(parse_duration_seconds("1 d"), content="86400")
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
  inspect(agents.length(), content="1")
  inspect(agents[0].has_json_derives, content="true")
  inspect(agents[0].has_snapshottable, content="false")
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
  inspect(agents.length(), content="1")
  inspect(agents[0].has_json_derives, content="true")
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
  inspect(agents.length(), content="1")
  inspect(agents[0].has_json_derives, content="false")
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
    },
  ]
  let impls = generate_agent_registration(agents)
  let output = @formatter.impls_to_string(impls)
  // Check that the output contains direct Snapshottable impl (not SnapshotCell)
  inspect(output.contains("SnapshotCell"), content="false")
  inspect(output.contains("Snapshottable for Counter"), content="true")
  inspect(output.contains("snapshot_format"), content="true")
  inspect(output.contains("Json"), content="true")
  // Only mut fields are assigned in load_snapshot
  inspect(output.contains("self.name = new_val.name"), content="false")
  inspect(output.contains("self.value = new_val.value"), content="true")
}

///|
test "parse_duration_seconds compound" {
  inspect(parse_duration_seconds("1 hour 30 minutes"), content="5400")
  inspect(
    parse_duration_seconds("1 day 2 hours 30 minutes 15 seconds"),
    content="95415",
  )
  inspect(parse_duration_seconds("2 hours 0 seconds"), content="7200")
}
