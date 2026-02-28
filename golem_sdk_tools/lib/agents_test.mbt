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
}

///|
test "generate_agent_registration for simple agent" {
  let agents : Array[AgentInfo] = [
    {
      name: "Counter",
      description: "A counter agent",
      constructor_params: [("name", Simple("String"))],
      constructor_description: "Creates a new counter",
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
      #|        name: None,
      #|        description: "Creates a new counter",
      #|        prompt_hint: None,
      #|        input_schema: @common.DataSchema::Tuple([
      #|          ("name", @schema.schema_of(("" : String))),
      #|        ]),
      #|      },
      #|      methods: [],
      #|      dependencies: [],
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
