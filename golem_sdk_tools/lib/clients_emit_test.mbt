///|
test "generate_client_code - counter agent with no-arg methods" {
  let agent : @lib.AgentInfo = {
    name: "Counter",
    description: "A counter agent",
    mode: @lib.AgentMode::Durable,
    constructor_params: [("name", @lib.ParamType::Simple("String"))],
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
        description: "Gets the value",
        prompt_hint: None,
        params: [],
        return_type: Some(@lib.ParamType::Simple("UInt64")),
        param_schema_options: {},
      },
    ],
    has_init: false,
    constructor_schema_options: {},
    has_snapshottable: false,
  }
  let impls = @lib.generate_client_code([agent])
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|///|
      #|pub(all) struct CounterClient {
      #|  client : @rpc.AgentClient
      #|}
      #|
      #|///|
      #|pub fn CounterClient::get(name : String) -> CounterClient raise @common.AgentError {
      #|  let constructor_data = 
      #|    @common.DataValue::Tuple([@schema.to_element_value_as(name)])
      #|  let client = @rpc.AgentClient::get("Counter", constructor_data)
      #|  { client, }
      #|}
      #|
      #|///|
      #|pub fn CounterClient::new_phantom(name : String, phantom_id : @rpcTypes.Uuid) -> CounterClient raise @common.AgentError {
      #|  let constructor_data = 
      #|    @common.DataValue::Tuple([@schema.to_element_value_as(name)])
      #|  let client = 
      #|    @rpc.AgentClient::new_phantom("Counter", constructor_data, phantom_id)
      #|  { client, }
      #|}
      #|
      #|///|
      #|pub fn CounterClient::get_agent_id(self : CounterClient) -> String {
      #|  self.client.get_agent_id()
      #|}
      #|
      #|///|
      #|pub fn CounterClient::phantom_id(self : CounterClient) -> @rpcTypes.Uuid? raise @common.AgentError {
      #|  self.client.phantom_id()
      #|}
      #|
      #|///|
      #|pub fn CounterClient::drop(self : CounterClient) -> Unit {
      #|  self.client.drop()
      #|}
      #|
      #|///|
      #|pub fn CounterClient::increment(self : CounterClient) -> Unit {
      #|  let _ = self.client.invoke_and_await("counter.{increment}", [])
      #|  ()
      #|}
      #|
      #|///|
      #|pub fn CounterClient::trigger_increment(self : CounterClient) -> Unit {
      #|  self.client.invoke("counter.{increment}", [])
      #|}
      #|
      #|///|
      #|pub fn CounterClient::schedule_increment(
      #|  self : CounterClient,
      #|  scheduled_at : @wallClock.Datetime,
      #|) -> Unit {
      #|  self.client.schedule_invocation(scheduled_at, "counter.{increment}", [])
      #|}
      #|
      #|///|
      #|pub fn CounterClient::get_value(self : CounterClient) -> UInt64 raise @common.AgentError {
      #|  let result = self.client.invoke_and_await("counter.{get-value}", [])
      #|  @rpc.from_wit_value(@rpc.unwrap_wit_tuple(result))
      #|}
      #|
      #|///|
      #|pub fn CounterClient::trigger_get_value(self : CounterClient) -> Unit {
      #|  self.client.invoke("counter.{get-value}", [])
      #|}
      #|
      #|///|
      #|pub fn CounterClient::schedule_get_value(
      #|  self : CounterClient,
      #|  scheduled_at : @wallClock.Datetime,
      #|) -> Unit {
      #|  self.client.schedule_invocation(scheduled_at, "counter.{get-value}", [])
      #|}
      #|
      #|
    ),
  )
}

///|
test "generate_client_code - agent with unstructured text param and return" {
  let agent : @lib.AgentInfo = {
    name: "TextAgent",
    description: "A text agent",
    mode: @lib.AgentMode::Durable,
    constructor_params: [],
    constructor_description: "Creates a text agent",
    constructor_prompt_hint: None,
    methods: [
      {
        name: "process_text",
        description: "Processes text",
        prompt_hint: None,
        params: [("input", @lib.ParamType::Simple("@types.UnstructuredText"))],
        return_type: Some(@lib.ParamType::Simple("@types.UnstructuredText")),
        param_schema_options: {},
      },
      {
        name: "upload_binary",
        description: "Uploads binary",
        prompt_hint: None,
        params: [("data", @lib.ParamType::Simple("UnstructuredBinary"))],
        return_type: None,
        param_schema_options: {},
      },
    ],
    has_init: false,
    constructor_schema_options: {},
    has_snapshottable: false,
  }
  let impls = @lib.generate_client_code([agent])
  let output = @formatter.impls_to_string(impls)
  // Verify the generated code uses the type-specific conversion functions
  assert_true(output.contains("rpc.unstructured_text_to_wit_value"))
  assert_true(output.contains("rpc.wit_value_to_unstructured_text"))
  assert_true(output.contains("rpc.unstructured_binary_to_wit_value"))
  // Should NOT use generic to_wit_value for unstructured types
  assert_false(output.contains("rpc.to_wit_value(input)"))
  assert_false(output.contains("rpc.to_wit_value(data)"))
}

///|
test "generate_client_code - agent with multimodal param" {
  let agent : @lib.AgentInfo = {
    name: "VisionAgent",
    description: "A vision agent",
    mode: @lib.AgentMode::Durable,
    constructor_params: [],
    constructor_description: "Creates a vision agent",
    constructor_prompt_hint: None,
    methods: [
      {
        name: "process",
        description: "Processes multimodal input",
        prompt_hint: None,
        params: [
          (
            "input",
            @lib.ParamType::MultimodalType(
              @lib.ParamType::Simple("TextOrBinary"),
            ),
          ),
        ],
        return_type: None,
        param_schema_options: {},
      },
      {
        name: "analyze",
        description: "Analyzes and returns multimodal",
        prompt_hint: None,
        params: [],
        return_type: Some(
          @lib.ParamType::MultimodalType(@lib.ParamType::Simple("TextOrBinary")),
        ),
        param_schema_options: {},
      },
    ],
    has_init: false,
    constructor_schema_options: {},
    has_snapshottable: false,
  }
  let impls = @lib.generate_client_code([agent])
  let output = @formatter.impls_to_string(impls)
  // Verify multimodal encoding is used for params
  assert_true(output.contains("rpc.multimodal_to_wit_value"))
  // Verify multimodal decoding is used for return
  assert_true(output.contains("rpc.wit_value_to_multimodal"))
  // Should NOT use generic to_wit_value for multimodal types
  assert_false(output.contains("rpc.to_wit_value(input)"))
}

///|
test "generate_client_code - agent with multimodal constructor" {
  let agent : @lib.AgentInfo = {
    name: "ModalAgent",
    description: "Agent with multimodal constructor",
    mode: @lib.AgentMode::Durable,
    constructor_params: [
      (
        "input",
        @lib.ParamType::MultimodalType(@lib.ParamType::Simple("TextOrBinary")),
      ),
    ],
    constructor_description: "Creates with multimodal input",
    constructor_prompt_hint: None,
    methods: [],
    has_init: false,
    constructor_schema_options: {},
    has_snapshottable: false,
  }
  let impls = @lib.generate_client_code([agent])
  let output = @formatter.impls_to_string(impls)
  // Should use to_data_value for multimodal constructor
  assert_true(output.contains("to_data_value"))
  // Should NOT use DataValue::Tuple for multimodal constructor
  assert_false(output.contains("DataValue::Tuple"))
}

///|
test "generate_client_code - agent with params" {
  let agent : @lib.AgentInfo = {
    name: "TaskManager",
    description: "A task manager",
    mode: @lib.AgentMode::Durable,
    constructor_params: [],
    constructor_description: "Creates a task manager",
    constructor_prompt_hint: None,
    methods: [
      {
        name: "add_task",
        description: "Adds a task",
        prompt_hint: None,
        params: [("task", @lib.ParamType::Simple("TaskInfo"))],
        return_type: Some(@lib.ParamType::Simple("UInt64")),
        param_schema_options: {},
      },
    ],
    has_init: false,
    constructor_schema_options: {},
    has_snapshottable: false,
  }
  let impls = @lib.generate_client_code([agent])
  let output = @formatter.impls_to_string(impls)
  inspect(
    output,
    content=(
      #|///|
      #|pub(all) struct TaskManagerClient {
      #|  client : @rpc.AgentClient
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::get() -> TaskManagerClient raise @common.AgentError {
      #|  let constructor_data = @common.DataValue::Tuple([])
      #|  let client = @rpc.AgentClient::get("TaskManager", constructor_data)
      #|  { client, }
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::new_phantom(phantom_id : @rpcTypes.Uuid) -> TaskManagerClient raise @common.AgentError {
      #|  let constructor_data = @common.DataValue::Tuple([])
      #|  let client = 
      #|    @rpc.AgentClient::new_phantom("TaskManager", constructor_data, phantom_id)
      #|  { client, }
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::get_agent_id(self : TaskManagerClient) -> String {
      #|  self.client.get_agent_id()
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::phantom_id(self : TaskManagerClient) -> @rpcTypes.Uuid? raise @common.AgentError {
      #|  self.client.phantom_id()
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::drop(self : TaskManagerClient) -> Unit {
      #|  self.client.drop()
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::add_task(self : TaskManagerClient, task : TaskInfo) -> UInt64 raise @common.AgentError {
      #|  let result = self
      #|    .client
      #|    .invoke_and_await("task-manager.{add-task}", [@rpc.to_wit_value(task)])
      #|  @rpc.from_wit_value(@rpc.unwrap_wit_tuple(result))
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::trigger_add_task(
      #|  self : TaskManagerClient,
      #|  task : TaskInfo,
      #|) -> Unit raise @common.AgentError {
      #|  self.client.invoke("task-manager.{add-task}", [@rpc.to_wit_value(task)])
      #|}
      #|
      #|///|
      #|pub fn TaskManagerClient::schedule_add_task(
      #|  self : TaskManagerClient,
      #|  scheduled_at : @wallClock.Datetime,
      #|  task : TaskInfo,
      #|) -> Unit raise @common.AgentError {
      #|  self
      #|    .client
      #|    .schedule_invocation(scheduled_at, "task-manager.{add-task}", [
      #|      @rpc.to_wit_value(task),
      #|    ])
      #|}
      #|
      #|
    ),
  )
}
