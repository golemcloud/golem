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
      constructor_params: [("name", Simple("String"))],
      constructor_description: "Creates a new counter",
      constructor_prompt_hint: None,
      methods: [],
      has_init: false,
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
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a new counter",
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([
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
      #|        Ok(Counter::new(name))
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
        },
        {
          name: "add",
          description: "Adds a value",
          prompt_hint: None,
          params: [("value", Simple("UInt64"))],
          return_type: Some(Simple("UInt64")),
        },
      ],
      has_init: false,
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
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a new counter",
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([
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
      #|          prompt_hint: None,
      #|          input_schema: @common.DataSchema::Tuple([]),
      #|          output_schema: @common.DataSchema::Tuple([]),
      #|        },
      #|        {
      #|          name: "add",
      #|          description: "Adds a value",
      #|          prompt_hint: None,
      #|          input_schema: @common.DataSchema::Tuple([
      #|            (
      #|              "value",
      #|              @schema.schema_of_tag(
      #|                (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|              ),
      #|            ),
      #|          ]),
      #|          output_schema: @common.DataSchema::Tuple([
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
      #|        Ok(Counter::new(name))
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
      #|        Ok(@common.DataValue::Tuple([]))
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
      #|        Ok(@common.DataValue::Tuple([@schema.to_element_value_as(result)]))
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
        },
      ],
      has_init: false,
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
      #|      constructor_: {
      #|        name: Some("new"),
      #|        description: "Creates a new counter",
      #|        prompt_hint: Some("Provide a unique name"),
      #|        input_schema: @common.DataSchema::Tuple([
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
      #|          prompt_hint: Some("Call to increase"),
      #|          input_schema: @common.DataSchema::Tuple([]),
      #|          output_schema: @common.DataSchema::Tuple([]),
      #|        },
      #|      ],
      #|      dependencies: [],
      #|      mode: @common.AgentMode::DURABLE,
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
      #|        Ok(Counter::new(name))
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
      #|        Ok(@common.DataValue::Tuple([]))
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
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new counter",
      #|      prompt_hint: None,
      #|      input_schema: @common.DataSchema::Tuple([
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
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new counter",
      #|      prompt_hint: None,
      #|      input_schema: @common.DataSchema::Tuple([
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
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([]),
      #|        output_schema: @common.DataSchema::Tuple([]),
      #|      },
      #|      {
      #|        name: "add",
      #|        description: "Adds a value to the counter",
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([
      #|          (
      #|            "value",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[UInt64]),
      #|            ),
      #|          ),
      #|        ]),
      #|        output_schema: @common.DataSchema::Tuple([
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
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a smart agent",
      #|      prompt_hint: Some("Provide a JSON config string"),
      #|      input_schema: @common.DataSchema::Tuple([
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
      #|        prompt_hint: Some("Pass natural language text"),
      #|        input_schema: @common.DataSchema::Tuple([
      #|          (
      #|            "text",
      #|            @schema.schema_of_tag(
      #|              (@schema.TypeTag::{  } : @schema.TypeTag[String]),
      #|            ),
      #|          ),
      #|        ]),
      #|        output_schema: @common.DataSchema::Tuple([
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
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new Processor",
      #|      prompt_hint: None,
      #|      input_schema: @common.DataSchema::Tuple([
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
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([
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
      #|        output_schema: @common.DataSchema::Tuple([
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
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a default worker",
      #|      prompt_hint: None,
      #|      input_schema: @common.DataSchema::Tuple([]),
      #|    },
      #|    methods: [
      #|      {
      #|        name: "run",
      #|        description: "Does work",
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([]),
      #|        output_schema: @common.DataSchema::Tuple([
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
      #|    constructor_: {
      #|      name: Some("new"),
      #|      description: "Creates a new MultiAgent",
      #|      prompt_hint: None,
      #|      input_schema: @common.DataSchema::Tuple([
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
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([]),
      #|        output_schema: @common.DataSchema::Tuple([
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
      },
      {
        name: "get_value",
        description: "Gets the current value",
        prompt_hint: None,
        params: [],
        return_type: Some(Simple("UInt64")),
      },
      {
        name: "add",
        description: "Adds a value",
        prompt_hint: None,
        params: [("value", Simple("UInt64"))],
        return_type: Some(Simple("UInt64")),
      },
    ],
    has_init: false,
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
      #|        Ok(@common.DataValue::Tuple([]))
      #|      }
      #|      "get_value" => {
      #|        let elements = @extractor.extract_tuple(input)
      #|        guard elements.length() == 0 else {
      #|          raise  @common.AgentError::InvalidInput(
      #|            "Expected 0 elements, got " + elements.length().to_string(),
      #|          )
      #|        }
      #|        let result = self.get_value()
      #|        Ok(@common.DataValue::Tuple([@schema.to_element_value_as(result)]))
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
      #|        Ok(@common.DataValue::Tuple([@schema.to_element_value_as(result)]))
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
