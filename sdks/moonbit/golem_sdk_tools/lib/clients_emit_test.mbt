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
        http_endpoint: None,
        read_only: None,
      },
      {
        name: "get_value",
        description: "Gets the value",
        prompt_hint: None,
        params: [],
        return_type: Some(@lib.ParamType::Simple("UInt64")),
        http_endpoint: None,
        read_only: None,
      },
    ],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: None,
  }
  let impls = @lib.generate_client_code([agent])
  let output = @formatter.impls_to_string(impls)
  debug_inspect(
    output,
    content=(
      #|"///|\n///\npub(all) struct CounterClient {\n  client : @rpc.AgentClient\n}\n\n///|\n///\n/// Binds the current durable incarnation without creating it or waiting for initialization.\n///\n/// Use this to schedule work on self from inside the constructor. `upsert` reuses\n/// an initialized agent without rerunning its constructor, but waits for an unfinished\n/// constructor. Calling `upsert` on self from that constructor would therefore deadlock.\n///\n/// Scheduled work runs after construction finishes. This does not make awaited\n/// self-calls reentrant or bypass method permissions. A different client type raises\n/// an error. Replay preserves the recorded incarnation.\npub fn CounterClient::bind_self() -> CounterClient raise {\n  { client: @rpc.AgentClient::bind_self(\"Counter\"), }\n}\n\n///|\n///\n/// Connects to the agent that the constructor parameters key. The host creates\n/// the agent when it is missing and reuses it otherwise.\npub fn CounterClient::upsert(name : String) -> CounterClient raise {\n  let constructor_input = @agents.encode_wire_input([name])\n  let client = @rpc.AgentClient::upsert(\"Counter\", constructor_input)\n  { client, }\n}\n\n///|\n///\n/// Connects to the existing agent with the given ID. Never creates an agent: an\n/// unknown ID, or the ID of an agent of another type, raises `RpcError::NotFound`.\npub fn CounterClient::get_by_id(agent_id : @schema.AgentId) -> CounterClient raise @agentHost.RpcError {\n  let client = @rpc.AgentClient::get_by_id(\"Counter\", agent_id)\n  { client, }\n}\n\n///|\n///\n/// Creates a new agent and returns after its initialization. With `keyed`, the new\n/// agent is the agent that the constructor parameters key, and the creation raises\n/// `RpcError::Conflict` when that agent already exists; otherwise the new agent has\n/// no key and a new ID. `env` adds environment variables to the ones the manifest\n/// defines.\npub async fn CounterClient::create(\n  name : String,\n  keyed? : Bool = false,\n  env? : Array[(String, String)] = [],\n) -> CounterClient {\n  let constructor_input = @agents.encode_wire_input([name])\n  let client = \n    @rpc.AgentClient::create(\"Counter\", constructor_input, keyed~, env~)\n  { client, }\n}\n\n///|\n///\n/// The ID of the agent. On a client of `upsert`, the host creates the agent when\n/// it is missing, without invoking a method.\npub async fn CounterClient::agent_id(self : CounterClient) -> @schema.AgentId {\n  self.client.agent_id()\n}\n\n///|\n///\npub fn CounterClient::drop(self : CounterClient) -> Unit {\n  self.client.drop()\n}\n\n///|\n///\npub fn CounterClient::set_default_scope_card(\n  self : CounterClient,\n  card : @permissions.ScopeCard?,\n) -> Unit {\n  self.client.set_default_scope_card(card)\n}\n\n///|\n///\npub async fn[T] CounterClient::scoped_upsert(\n  name : String,\n  f : async (CounterClient) -> T,\n) -> T {\n  let client = CounterClient::upsert(name)\n  defer client.drop()\n  f(client)\n}\n\n///|\n///\npub async fn[T] CounterClient::scoped_get_by_id(\n  agent_id : @schema.AgentId,\n  f : async (CounterClient) -> T,\n) -> T {\n  let client = CounterClient::get_by_id(agent_id)\n  defer client.drop()\n  f(client)\n}\n\n///|\n///\npub async fn[T] CounterClient::scoped_create(\n  name : String,\n  keyed? : Bool = false,\n  env? : Array[(String, String)] = [],\n  f : async (CounterClient) -> T,\n) -> T {\n  let client = CounterClient::create(name, keyed~, env~)\n  defer client.drop()\n  f(client)\n}\n\n///|\n///\npub async fn CounterClient::increment(\n  self : CounterClient,\n  scope_card? : @permissions.ScopeCard,\n) -> Unit {\n  self.client.check_scope_card(scope_card?)\n  let input = @agents.encode_wire_input_async([])\n  let response = self.client.invoke_and_await(\"increment\", input, scope_card?)\n  @agents.discard_optional_wire_value(response.value)\n}\n\n///|\n///\n/// Starts `increment` without waiting for it to run, and returns the handle of\n/// the invocation once the target has accepted it.\npub async fn CounterClient::trigger_increment(self : CounterClient) -> @rpc.Invocation[\n  Unit,\n] {\n  let input = @agents.encode_wire_input([])\n  self\n    .client\n    .trigger(\n      \"increment\", input, output => @agents.discard_optional_wire_value(output)\n    )\n}\n\n///|\n///\n/// Schedules `increment` for `scheduled_at`, and returns the handle of the\n/// invocation, which can cancel it until it starts.\npub async fn CounterClient::schedule_increment(\n  self : CounterClient,\n  scheduled_at : @systemClock.Instant,\n) -> @rpc.Invocation[Unit] {\n  let input = @agents.encode_wire_input([])\n  self\n    .client\n    .schedule(\n      scheduled_at, \"increment\", input, output => {\n        @agents.discard_optional_wire_value(output)\n      },\n    )\n}\n\n///|\n///\n/// Rebuilds the handle of an invocation of `increment` from the ID that\n/// `trigger_increment` or `schedule_increment` returned. Makes no remote call.\npub fn CounterClient::get_increment_invocation(id : @rpc.InvocationId) -> @rpc.Invocation[\n  Unit,\n] {\n  @rpc.Invocation::get(\n    id, output => @agents.discard_optional_wire_value(output)\n  )\n}\n\n///|\n///\npub async fn CounterClient::get_value(\n  self : CounterClient,\n  scope_card? : @permissions.ScopeCard,\n) -> UInt64 {\n  self.client.check_scope_card(scope_card?)\n  let input = @agents.encode_wire_input_async([])\n  let response = self.client.invoke_and_await(\"get_value\", input, scope_card?)\n  let result = response.value\n  match result {\n    Some(tree) => @agents.decode_wire_output(tree, \"Counter.get_value\")\n    None => {\n      raise  @common.AgentError::InvalidType(\n        \"Counter.get_value: expected a return value, got none\",\n      )\n    }\n  }\n}\n\n///|\n///\n/// Starts `get_value` without waiting for it to run, and returns the handle of\n/// the invocation once the target has accepted it.\npub async fn CounterClient::trigger_get_value(self : CounterClient) -> @rpc.Invocation[\n  UInt64,\n] {\n  let input = @agents.encode_wire_input([])\n  self\n    .client\n    .trigger(\n      \"get_value\", input, output => {\n        match output {\n          Some(tree) => @agents.decode_wire_output(tree, \"Counter.get_value\")\n          None => {\n            raise  @common.AgentError::InvalidType(\n              \"Counter.get_value: expected a return value, got none\",\n            )\n          }\n        }\n      },\n    )\n}\n\n///|\n///\n/// Schedules `get_value` for `scheduled_at`, and returns the handle of the\n/// invocation, which can cancel it until it starts.\npub async fn CounterClient::schedule_get_value(\n  self : CounterClient,\n  scheduled_at : @systemClock.Instant,\n) -> @rpc.Invocation[UInt64] {\n  let input = @agents.encode_wire_input([])\n  self\n    .client\n    .schedule(\n      scheduled_at, \"get_value\", input, output => {\n        match output {\n          Some(tree) => @agents.decode_wire_output(tree, \"Counter.get_value\")\n          None => {\n            raise  @common.AgentError::InvalidType(\n              \"Counter.get_value: expected a return value, got none\",\n            )\n          }\n        }\n      },\n    )\n}\n\n///|\n///\n/// Rebuilds the handle of an invocation of `get_value` from the ID that\n/// `trigger_get_value` or `schedule_get_value` returned. Makes no remote call.\npub fn CounterClient::get_get_value_invocation(id : @rpc.InvocationId) -> @rpc.Invocation[\n  UInt64,\n] {\n  @rpc.Invocation::get(\n    id, output => {\n      match output {\n        Some(tree) => @agents.decode_wire_output(tree, \"Counter.get_value\")\n        None => {\n          raise  @common.AgentError::InvalidType(\n            \"Counter.get_value: expected a return value, got none\",\n          )\n        }\n      }\n    },\n  )\n}\n\n"
    ),
  )
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
        http_endpoint: None,
        read_only: None,
      },
    ],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: None,
  }
  let impls = @lib.generate_client_code([agent])
  let output = @formatter.impls_to_string(impls)
  // Verify no with_config methods are generated for agents without config
  assert_false(output.contains("with_config"))
  debug_inspect(
    output,
    content=(
      #|"///|\n///\npub(all) struct TaskManagerClient {\n  client : @rpc.AgentClient\n}\n\n///|\n///\n/// Binds the current durable incarnation without creating it or waiting for initialization.\n///\n/// Use this to schedule work on self from inside the constructor. `upsert` reuses\n/// an initialized agent without rerunning its constructor, but waits for an unfinished\n/// constructor. Calling `upsert` on self from that constructor would therefore deadlock.\n///\n/// Scheduled work runs after construction finishes. This does not make awaited\n/// self-calls reentrant or bypass method permissions. A different client type raises\n/// an error. Replay preserves the recorded incarnation.\npub fn TaskManagerClient::bind_self() -> TaskManagerClient raise {\n  { client: @rpc.AgentClient::bind_self(\"TaskManager\"), }\n}\n\n///|\n///\n/// Connects to the agent that the constructor parameters key. The host creates\n/// the agent when it is missing and reuses it otherwise.\npub fn TaskManagerClient::upsert() -> TaskManagerClient raise {\n  let constructor_input = @agents.encode_wire_input([])\n  let client = @rpc.AgentClient::upsert(\"TaskManager\", constructor_input)\n  { client, }\n}\n\n///|\n///\n/// Connects to the existing agent with the given ID. Never creates an agent: an\n/// unknown ID, or the ID of an agent of another type, raises `RpcError::NotFound`.\npub fn TaskManagerClient::get_by_id(agent_id : @schema.AgentId) -> TaskManagerClient raise @agentHost.RpcError {\n  let client = @rpc.AgentClient::get_by_id(\"TaskManager\", agent_id)\n  { client, }\n}\n\n///|\n///\n/// Creates a new agent and returns after its initialization. With `keyed`, the new\n/// agent is the agent that the constructor parameters key, and the creation raises\n/// `RpcError::Conflict` when that agent already exists; otherwise the new agent has\n/// no key and a new ID. `env` adds environment variables to the ones the manifest\n/// defines.\npub async fn TaskManagerClient::create(\n  keyed? : Bool = false,\n  env? : Array[(String, String)] = [],\n) -> TaskManagerClient {\n  let constructor_input = @agents.encode_wire_input([])\n  let client = \n    @rpc.AgentClient::create(\"TaskManager\", constructor_input, keyed~, env~)\n  { client, }\n}\n\n///|\n///\n/// The ID of the agent. On a client of `upsert`, the host creates the agent when\n/// it is missing, without invoking a method.\npub async fn TaskManagerClient::agent_id(self : TaskManagerClient) -> @schema.AgentId {\n  self.client.agent_id()\n}\n\n///|\n///\npub fn TaskManagerClient::drop(self : TaskManagerClient) -> Unit {\n  self.client.drop()\n}\n\n///|\n///\npub fn TaskManagerClient::set_default_scope_card(\n  self : TaskManagerClient,\n  card : @permissions.ScopeCard?,\n) -> Unit {\n  self.client.set_default_scope_card(card)\n}\n\n///|\n///\npub async fn[T] TaskManagerClient::scoped_upsert(\n  f : async (TaskManagerClient) -> T,\n) -> T {\n  let client = TaskManagerClient::upsert()\n  defer client.drop()\n  f(client)\n}\n\n///|\n///\npub async fn[T] TaskManagerClient::scoped_get_by_id(\n  agent_id : @schema.AgentId,\n  f : async (TaskManagerClient) -> T,\n) -> T {\n  let client = TaskManagerClient::get_by_id(agent_id)\n  defer client.drop()\n  f(client)\n}\n\n///|\n///\npub async fn[T] TaskManagerClient::scoped_create(\n  keyed? : Bool = false,\n  env? : Array[(String, String)] = [],\n  f : async (TaskManagerClient) -> T,\n) -> T {\n  let client = TaskManagerClient::create(keyed~, env~)\n  defer client.drop()\n  f(client)\n}\n\n///|\n///\npub async fn TaskManagerClient::add_task(\n  self : TaskManagerClient,\n  task : TaskInfo,\n  scope_card? : @permissions.ScopeCard,\n) -> UInt64 {\n  self.client.check_scope_card(scope_card?)\n  let input = @agents.encode_wire_input_async([task])\n  let response = self.client.invoke_and_await(\"add_task\", input, scope_card?)\n  let result = response.value\n  match result {\n    Some(tree) => @agents.decode_wire_output(tree, \"TaskManager.add_task\")\n    None => {\n      raise  @common.AgentError::InvalidType(\n        \"TaskManager.add_task: expected a return value, got none\",\n      )\n    }\n  }\n}\n\n///|\n///\n/// Starts `add_task` without waiting for it to run, and returns the handle of\n/// the invocation once the target has accepted it.\npub async fn TaskManagerClient::trigger_add_task(\n  self : TaskManagerClient,\n  task : TaskInfo,\n) -> @rpc.Invocation[UInt64] {\n  let input = @agents.encode_wire_input([task])\n  self\n    .client\n    .trigger(\n      \"add_task\", input, output => {\n        match output {\n          Some(tree) => @agents.decode_wire_output(tree, \"TaskManager.add_task\")\n          None => {\n            raise  @common.AgentError::InvalidType(\n              \"TaskManager.add_task: expected a return value, got none\",\n            )\n          }\n        }\n      },\n    )\n}\n\n///|\n///\n/// Schedules `add_task` for `scheduled_at`, and returns the handle of the\n/// invocation, which can cancel it until it starts.\npub async fn TaskManagerClient::schedule_add_task(\n  self : TaskManagerClient,\n  scheduled_at : @systemClock.Instant,\n  task : TaskInfo,\n) -> @rpc.Invocation[UInt64] {\n  let input = @agents.encode_wire_input([task])\n  self\n    .client\n    .schedule(\n      scheduled_at, \"add_task\", input, output => {\n        match output {\n          Some(tree) => @agents.decode_wire_output(tree, \"TaskManager.add_task\")\n          None => {\n            raise  @common.AgentError::InvalidType(\n              \"TaskManager.add_task: expected a return value, got none\",\n            )\n          }\n        }\n      },\n    )\n}\n\n///|\n///\n/// Rebuilds the handle of an invocation of `add_task` from the ID that\n/// `trigger_add_task` or `schedule_add_task` returned. Makes no remote call.\npub fn TaskManagerClient::get_add_task_invocation(id : @rpc.InvocationId) -> @rpc.Invocation[\n  UInt64,\n] {\n  @rpc.Invocation::get(\n    id, output => {\n      match output {\n        Some(tree) => @agents.decode_wire_output(tree, \"TaskManager.add_task\")\n        None => {\n          raise  @common.AgentError::InvalidType(\n            \"TaskManager.add_task: expected a return value, got none\",\n          )\n        }\n      }\n    },\n  )\n}\n\n"
    ),
  )
}

///|
test "generate_client_code - all secret config generates no config override" {
  let agent : @lib.AgentInfo = {
    name: "SecretAgent",
    description: "Agent with all-secret config",
    mode: @lib.AgentMode::Durable,
    constructor_params: [
      ("name", @lib.ParamType::Simple("String")),
      (
        "config",
        @lib.ParamType::Parameterized("Config", [
          @lib.ParamType::Simple("AllSecretConfig"),
        ]),
      ),
    ],
    constructor_description: "Creates agent",
    constructor_prompt_hint: None,
    methods: [],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("AllSecretConfig"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "AllSecretConfig",
      package_path: "",
      fields: [
        (
          "api_key",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
        (
          "db_password",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
      ],
    },
  ]
  let impls = @lib.generate_client_code(
    [agent],
    config_types~,
    agent_package="",
  )
  let output = @formatter.impls_to_string(impls)
  assert_true(output.contains("SecretAgentClient::create"))
  assert_false(output.contains("config?"))
  assert_false(output.contains("Override"))
}

///|
test "generate_client_code - mixed config passes overrides through create" {
  let agent : @lib.AgentInfo = {
    name: "ConfiguredAgent",
    description: "Agent with mixed config",
    mode: @lib.AgentMode::Durable,
    constructor_params: [
      ("name", @lib.ParamType::Simple("String")),
      (
        "config",
        @lib.ParamType::Parameterized("Config", [
          @lib.ParamType::Simple("MixedConfig"),
        ]),
      ),
    ],
    constructor_description: "Creates agent",
    constructor_prompt_hint: None,
    methods: [
      {
        name: "get_value",
        description: "Gets value",
        prompt_hint: None,
        params: [],
        return_type: Some(@lib.ParamType::Simple("String")),
        http_endpoint: None,
        read_only: None,
      },
    ],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("MixedConfig"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "MixedConfig",
      package_path: "",
      fields: [
        (
          "api_key",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
        ("timeout", @lib.ParamType::Simple("UInt64")),
      ],
    },
  ]
  let impls = @lib.generate_client_code(
    [agent],
    config_types~,
    agent_package="",
  )
  let output = @formatter.impls_to_string(impls)
    .replace_all(old=" ", new="")
    .replace_all(old="\n", new="")
  assert_true(
    output.contains(
      "pubasyncfnConfiguredAgentClient::create(name:String,keyed?:Bool=false,config?:MixedConfigOverride,env?:Array[(String,String)]=[],)->ConfiguredAgentClient{",
    ),
  )
  assert_true(
    output.contains(
      "letagent_config=matchconfig{Some(config)=>collect_mixed_config_overrides(config,[])None=>[]}",
    ),
  )
  assert_true(
    output.contains(
      "@rpc.AgentClient::create(\"ConfiguredAgent\",constructor_input,keyed~,config=agent_config,env~,)",
    ),
  )
  assert_true(
    output.contains(
      "letclient=ConfiguredAgentClient::create(name,keyed~,config?,env~)",
    ),
  )
  // `upsert` takes no overrides.
  assert_true(
    output.contains(
      "pubfnConfiguredAgentClient::upsert(name:String)->ConfiguredAgentClientraise{",
    ),
  )
  assert_false(output.contains("with_config"))
}

///|
test "generate_client_code - ephemeral agent connects only through upsert" {
  let agent : @lib.AgentInfo = {
    name: "EphemeralWorker",
    description: "An ephemeral worker",
    mode: @lib.AgentMode::Ephemeral,
    constructor_params: [("name", @lib.ParamType::Simple("String"))],
    constructor_description: "Creates an ephemeral worker",
    constructor_prompt_hint: None,
    methods: [
      {
        name: "run",
        params: [],
        return_type: Some(@lib.ParamType::Simple("String")),
        description: "Runs work",
        prompt_hint: None,
        read_only: None,
        http_endpoint: None,
      },
    ],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("MixedConfig"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "MixedConfig",
      package_path: "",
      fields: [
        (
          "api_key",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
        ("timeout", @lib.ParamType::Simple("UInt64")),
      ],
    },
  ]
  let impls = @lib.generate_client_code(
    [agent],
    config_types~,
    agent_package="",
  )
  let output = @formatter.impls_to_string(impls)

  assert_true(output.contains("pub fn EphemeralWorkerClient::upsert("))
  assert_true(output.contains("@rpc.AgentClient::upsert(\"EphemeralWorker\""))
  assert_true(output.contains("EphemeralWorkerClient::scoped_upsert("))
  // The host creates, gets by ID and reports agent IDs only for durable
  // agents, and an ephemeral agent takes no config overrides.
  assert_false(output.contains("create"))
  assert_false(output.contains("get_by_id"))
  assert_false(output.contains("agent_id()"))
  assert_false(output.contains("bind_self"))
  assert_false(output.contains("Override"))
  assert_true(output.contains("InvocationResult"))
  assert_true(output.contains("response.metadata"))
  // Trigger and schedule return the same handle in every agent mode.
  assert_true(output.contains("@rpc.Invocation["))
  assert_false(output.contains("Receipt"))
  assert_false(output.contains("schedule_cancelable_"))
  assert_false(output.contains("_with_metadata"))
}

///|
test "generate_config_override_types - flat config" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "MyConfig",
      package_path: "",
      fields: [
        (
          "api_key",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
        ("timeout", @lib.ParamType::Simple("UInt64")),
        ("name", @lib.ParamType::Simple("String")),
      ],
    },
  ]
  let impls = @lib.generate_config_override_types(config_types)
  let output = @formatter.impls_to_string(impls)
  // api_key (Secret) should be omitted, timeout and name should be present as Option
  assert_true(output.contains("MyConfigOverride"))
  assert_true(output.contains("timeout : UInt64?"))
  assert_true(output.contains("name : String?"))
  assert_false(output.contains("api_key"))
  assert_true(output.contains("::empty()"))
}

///|
test "generate_config_override_types - nested config" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "DbConfig",
      package_path: "",
      fields: [
        ("host", @lib.ParamType::Simple("String")),
        ("port", @lib.ParamType::Simple("UInt")),
      ],
    },
    {
      name: "AppConfig",
      package_path: "",
      fields: [
        ("db", @lib.ParamType::Simple("DbConfig")),
        ("name", @lib.ParamType::Simple("String")),
      ],
    },
  ]
  let impls = @lib.generate_config_override_types(config_types)
  let output = @formatter.impls_to_string(impls)
  // DbConfig -> DbConfigOverride with host and port
  assert_true(output.contains("DbConfigOverride"))
  assert_true(output.contains("host : String?"))
  assert_true(output.contains("port : UInt?"))
  // AppConfig -> AppConfigOverride with db as DbConfigOverride? and name as String?
  assert_true(output.contains("AppConfigOverride"))
  assert_true(output.contains("db : DbConfigOverride?"))
  assert_true(output.contains("name : String?"))
}

///|
test "generate_config_override_walkers - flat config" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "MyConfig",
      package_path: "",
      fields: [
        (
          "api_key",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
        ("timeout", @lib.ParamType::Simple("UInt64")),
      ],
    },
  ]
  let impls = @lib.generate_config_override_walkers(config_types)
  let output = @formatter.impls_to_string(impls)
  assert_true(output.contains("collect_my_config_overrides"))
  assert_true(output.contains("config.timeout"))
  assert_true(output.contains("typed_config_value"))
  assert_true(
    output.contains("impl @rpc.AgentConfigOverrides for MyConfigOverride"),
  )
  assert_true(output.contains("with fn typed_values("))
  // Secret fields should not appear
  assert_false(output.contains("api_key"))
}

///|
test "generate_config_override_walkers - nested config" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "DbConfig",
      package_path: "",
      fields: [("host", @lib.ParamType::Simple("String"))],
    },
    {
      name: "AppConfig",
      package_path: "",
      fields: [
        ("db", @lib.ParamType::Simple("DbConfig")),
        ("name", @lib.ParamType::Simple("String")),
      ],
    },
  ]
  let impls = @lib.generate_config_override_walkers(config_types)
  let output = @formatter.impls_to_string(impls)
  // Nested db field should call collect_db_config_overrides recursively
  assert_true(output.contains("collect_db_config_overrides"))
  assert_true(output.contains("collect_app_config_overrides"))
  // Leaf name field should call typed_config_value
  assert_true(output.contains("typed_config_value"))
}

///|
test "generate_config_override_types - all secret yields empty" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "AllSecret",
      package_path: "",
      fields: [
        (
          "key1",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
        (
          "key2",
          @lib.ParamType::Parameterized("@config.Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
      ],
    },
  ]
  let impls = @lib.generate_config_override_types(config_types)
  let output = @formatter.impls_to_string(impls)
  // All fields are secret, no override struct should be generated
  assert_false(output.contains("AllSecretOverride"))
}

///|
test "pascal_to_snake conversion" {
  // Test indirectly via walker function names
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "DatabaseConfig",
      package_path: "",
      fields: [("host", @lib.ParamType::Simple("String"))],
    },
  ]
  let impls = @lib.generate_config_override_walkers(config_types)
  let output = @formatter.impls_to_string(impls)
  assert_true(output.contains("collect_database_config_overrides"))
}

///|
test "config_has_overridable_fields - recursion through nested all-secret" {
  // SecretsOnly has only a Secret field => no overridable leaves.
  let secrets_only : @lib.ConfigTypeInfo = {
    name: "SecretsOnly",
    package_path: "",
    fields: [
      (
        "key",
        @lib.ParamType::Parameterized("Secret", [
          @lib.ParamType::Simple("String"),
        ]),
      ),
    ],
  }
  // Parent whose only non-secret field is the all-secret nested config.
  let parent_all_secret : @lib.ConfigTypeInfo = {
    name: "ParentAllSecret",
    package_path: "",
    fields: [("nested", @lib.ParamType::Simple("SecretsOnly"))],
  }
  // Parent that also has a plain leaf => overridable.
  let parent_with_leaf : @lib.ConfigTypeInfo = {
    name: "ParentWithLeaf",
    package_path: "",
    fields: [
      ("nested", @lib.ParamType::Simple("SecretsOnly")),
      ("name", @lib.ParamType::Simple("String")),
    ],
  }
  let by_name : Map[String, @lib.ConfigTypeInfo] = Map([])
  by_name["SecretsOnly"] = secrets_only
  by_name["ParentAllSecret"] = parent_all_secret
  by_name["ParentWithLeaf"] = parent_with_leaf
  assert_false(@lib.config_has_overridable_fields(secrets_only, by_name))
  assert_false(@lib.config_has_overridable_fields(parent_all_secret, by_name))
  assert_true(@lib.config_has_overridable_fields(parent_with_leaf, by_name))
}

///|
test "generate_config_override - nested all-secret config does not dangle" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "SecretsOnly",
      package_path: "",
      fields: [
        (
          "key",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
      ],
    },
    {
      name: "AppConfig",
      package_path: "",
      fields: [
        ("db", @lib.ParamType::Simple("SecretsOnly")),
        ("name", @lib.ParamType::Simple("String")),
      ],
    },
  ]
  let types_out = @formatter.impls_to_string(
    @lib.generate_config_override_types(config_types),
  )
  // No override type for the all-secret nested struct.
  assert_false(types_out.contains("SecretsOnlyOverride"))
  // AppConfigOverride exists but only carries the `name` leaf, never `db`.
  assert_true(types_out.contains("AppConfigOverride"))
  assert_true(types_out.contains("name : String?"))
  assert_false(types_out.contains("db :"))
  let walkers_out = @formatter.impls_to_string(
    @lib.generate_config_override_walkers(config_types),
  )
  // No walker for the all-secret nested struct, and the parent must not call it.
  assert_false(walkers_out.contains("collect_secrets_only_overrides"))
  assert_true(walkers_out.contains("collect_app_config_overrides"))
  assert_true(walkers_out.contains("config.name"))
}

///|
test "validate_config_types - Secret inside a container is rejected" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "BadConfig",
      package_path: "",
      fields: [
        (
          "keys",
          @lib.ParamType::List(
            @lib.ParamType::Parameterized("Secret", [
              @lib.ParamType::Simple("String"),
            ]),
          ),
        ),
      ],
    },
  ]
  match
    capture_agent_parse_error(fn() raise @lib.AgentParseError {
      @lib.validate_config_types([], config_types)
    }) {
    Err(@lib.AgentParseError(msg)) => assert_true(msg.contains("Secret"))
    Ok(_) => abort("expected Secret-in-container to be rejected")
  }
}

///|
test "generate_client_code - no _with_config for all-secret config" {
  let agent : @lib.AgentInfo = {
    name: "SecretAgent",
    description: "An agent with secret-only config",
    mode: @lib.AgentMode::Durable,
    constructor_params: [],
    constructor_description: "Creates a SecretAgent",
    constructor_prompt_hint: None,
    methods: [],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("SecretsOnly"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "SecretsOnly",
      package_path: "",
      fields: [
        (
          "key",
          @lib.ParamType::Parameterized("Secret", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
      ],
    },
  ]
  let output = @formatter.impls_to_string(
    @lib.generate_client_code([agent], config_types~),
  )
  assert_true(output.contains("SecretAgentClient"))
  assert_false(output.contains("config?"))
}

///|
test "validate_config_types - cross-package nested config is rejected" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "DbConfig",
      package_path: "models",
      fields: [("host", @lib.ParamType::Simple("String"))],
    },
    {
      name: "AppConfig",
      package_path: "app",
      fields: [("db", @lib.ParamType::Simple("@models.DbConfig"))],
    },
  ]
  match
    capture_agent_parse_error(fn() raise @lib.AgentParseError {
      @lib.validate_config_types([], config_types)
    }) {
    Err(@lib.AgentParseError(msg)) =>
      assert_true(msg.contains("cross-package nested config"))
    Ok(_) => abort("expected cross-package nested config to be rejected")
  }
}

///|
test "validate_config_types - same-package qualified ref is allowed" {
  // A qualified reference to a config type in the *same* package must not be
  // rejected by the cross-package guard.
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "DbConfig",
      package_path: "app",
      fields: [("host", @lib.ParamType::Simple("String"))],
    },
    {
      name: "AppConfig",
      package_path: "app",
      fields: [("db", @lib.ParamType::Simple("@app.DbConfig"))],
    },
  ]
  match
    capture_agent_parse_error(fn() raise @lib.AgentParseError {
      @lib.validate_config_types([], config_types)
    }) {
    Ok(_) => ()
    Err(@lib.AgentParseError(msg)) =>
      abort("same-package qualified ref should be allowed, got: " + msg)
  }
}

///|
fn stream_client_test_method(
  name : String,
  params : Array[(String, @lib.ParamType)],
  return_type : @lib.ParamType?,
) -> @lib.MethodInfo {
  {
    name,
    description: "Stream detection test method",
    prompt_hint: None,
    params,
    return_type,
    http_endpoint: None,
    read_only: None,
  }
}

///|
fn stream_client_test_agent(
  constructor_params : Array[(String, @lib.ParamType)],
  methods : Array[@lib.MethodInfo],
) -> @lib.AgentInfo {
  {
    name: "StreamAgent",
    description: "Stream detection test agent",
    mode: @lib.AgentMode::Durable,
    constructor_params,
    constructor_description: "Creates a stream detection test agent",
    constructor_prompt_hint: None,
    methods,
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: None,
  }
}

///|
test "generate_client_code - recursively stream-bearing methods are awaited only" {
  let direct_stream = @lib.ParamType::Parameterized("AgentStream", [
    @lib.ParamType::Simple("String"),
  ])
  let qualified_stream = @lib.ParamType::Parameterized("@schema.AgentStream", [
    @lib.ParamType::Simple("UInt64"),
  ])
  let value_types : Array[@lib.ValueTypeInfo] = [
    {
      name: "NestedRecord",
      inline: false,
      kind: @lib.ValueTypeKind::Record([
        ("items", @lib.ParamType::Optional(@lib.ParamType::List(direct_stream))),
      ]),
    },
    {
      name: "NestedVariant",
      inline: false,
      kind: @lib.ValueTypeKind::VariantEnum([
        (
          @lib.ValueCase::{ source_name: "Empty", name: "Empty", },
          @lib.VariantPayload::None,
        ),
        (
          @lib.ValueCase::{ source_name: "One", name: "One", },
          @lib.VariantPayload::Single(
            @lib.ParamType::ResultType(
              @lib.ParamType::Simple("String"),
              qualified_stream,
            ),
          ),
        ),
        (
          @lib.ValueCase::{ source_name: "Many", name: "Many", },
          @lib.VariantPayload::Multi(
            [
              (
                "payload",
                @lib.ParamType::Tuple([
                  @lib.ParamType::Simple("Bool"),
                  direct_stream,
                ]),
              ),
            ],
            true,
          ),
        ),
      ]),
    },
    {
      name: "RecursiveStart",
      inline: false,
      kind: @lib.ValueTypeKind::Record([
        ("cycle", @lib.ParamType::Simple("RecursiveCycle")),
        ("streaming_sibling", @lib.ParamType::Simple("NestedRecord")),
      ]),
    },
    {
      name: "RecursiveCycle",
      inline: false,
      kind: @lib.ValueTypeKind::VariantEnum([
        (
          @lib.ValueCase::{ source_name: "Again", name: "Again", },
          @lib.VariantPayload::Single(
            @lib.ParamType::Optional(@lib.ParamType::Simple("RecursiveStart")),
          ),
        ),
      ]),
    },
    {
      name: "SafeRecursiveA",
      inline: false,
      kind: @lib.ValueTypeKind::Record([
        (
          "next",
          @lib.ParamType::Optional(@lib.ParamType::Simple("SafeRecursiveB")),
        ),
      ]),
    },
    {
      name: "SafeRecursiveB",
      inline: false,
      kind: @lib.ValueTypeKind::Record([
        (
          "next",
          @lib.ParamType::Optional(@lib.ParamType::Simple("SafeRecursiveA")),
        ),
      ]),
    },
  ]
  let methods = [
    stream_client_test_method("direct", [("input", direct_stream)], None),
    stream_client_test_method("qualified", [], Some(qualified_stream)),
    stream_client_test_method(
      "nested_record",
      [("input", @lib.ParamType::Simple("NestedRecord"))],
      None,
    ),
    stream_client_test_method(
      "nested_variant",
      [],
      Some(@lib.ParamType::Simple("NestedVariant")),
    ),
    stream_client_test_method(
      "tuple",
      [
        (
          "input",
          @lib.ParamType::Tuple([
            @lib.ParamType::Simple("String"),
            direct_stream,
          ]),
        ),
      ],
      None,
    ),
    stream_client_test_method(
      "generic",
      [("input", @lib.ParamType::Parameterized("Box", [direct_stream]))],
      None,
    ),
    stream_client_test_method(
      "multimodal",
      [("input", @lib.ParamType::MultimodalType(direct_stream))],
      None,
    ),
    stream_client_test_method(
      "recursive_sibling",
      [("input", @lib.ParamType::Simple("RecursiveStart"))],
      None,
    ),
    stream_client_test_method(
      "stream_free_recursive",
      [("input", @lib.ParamType::Simple("SafeRecursiveA"))],
      Some(@lib.ParamType::Simple("String")),
    ),
    stream_client_test_method(
      "scalar",
      [("input", @lib.ParamType::Simple("String"))],
      Some(@lib.ParamType::Simple("UInt64")),
    ),
  ]
  let output = @formatter.impls_to_string(
    @lib.generate_client_code(
      [stream_client_test_agent([], methods)],
      value_types~,
    ),
  )
  let awaited_only = [
    "direct", "qualified", "nested_record", "nested_variant", "tuple", "generic",
    "multimodal", "recursive_sibling",
  ]
  for name in awaited_only {
    assert_true(output.contains("pub async fn StreamAgentClient::\{name}"))
    assert_false(output.contains("StreamAgentClient::trigger_\{name}"))
    assert_false(output.contains("StreamAgentClient::schedule_\{name}"))
    assert_false(output.contains("StreamAgentClient::get_\{name}_invocation"))
  }
  for name in ["stream_free_recursive", "scalar"] {
    assert_true(output.contains("pub async fn StreamAgentClient::\{name}"))
    assert_true(output.contains("StreamAgentClient::trigger_\{name}"))
    assert_true(output.contains("StreamAgentClient::schedule_\{name}"))
    assert_true(output.contains("StreamAgentClient::get_\{name}_invocation"))
  }
}

///|
test "generate_client_code - stream-bearing constructor is rejected" {
  let value_types : Array[@lib.ValueTypeInfo] = [
    {
      name: "ConstructorInput",
      inline: false,
      kind: @lib.ValueTypeKind::Record([
        (
          "stream",
          @lib.ParamType::Parameterized("AgentStream", [
            @lib.ParamType::Simple("String"),
          ]),
        ),
      ]),
    },
  ]
  let agent = stream_client_test_agent(
    [("input", @lib.ParamType::Simple("ConstructorInput"))],
    [],
  )
  match
    capture_agent_parse_error(fn() raise @lib.AgentParseError {
      @lib.generate_client_code([agent], value_types~)
    }) {
    Err(@lib.AgentParseError(msg)) => {
      assert_true(msg.contains("agent 'StreamAgent'"))
      assert_true(msg.contains("constructor parameter 'input'"))
      assert_true(msg.contains("native client construction is synchronous"))
      assert_true(msg.contains("move the stream to an agent method"))
    }
    Ok(_) => abort("expected stream-bearing constructor to be rejected")
  }
}

///|
test "generate_client_code renames a positional scope_card parameter" {
  let agent : @lib.AgentInfo = {
    name: "Delegator",
    description: "",
    mode: @lib.AgentMode::Durable,
    constructor_params: [],
    constructor_description: "",
    constructor_prompt_hint: None,
    methods: [
      {
        name: "forward",
        description: "",
        prompt_hint: None,
        params: [("scope_card", @lib.ParamType::Simple("String"))],
        return_type: None,
        http_endpoint: None,
        read_only: None,
      },
    ],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: None,
  }
  let output = @formatter.impls_to_string(@lib.generate_client_code([agent]))
    .replace_all(old=" ", new="")
    .replace_all(old="\n", new="")
  assert_true(
    output.contains(
      "pubasyncfnDelegatorClient::forward(self:DelegatorClient,scope_card_:String,scope_card?:@permissions.ScopeCard,)->Unit{self.client.check_scope_card(scope_card?)letinput=@agents.encode_wire_input_async([scope_card_])letresponse=self.client.invoke_and_await(\"forward\",input,scope_card?)",
    ),
  )
  assert_true(
    output.contains(
      "pubasyncfnDelegatorClient::trigger_forward(self:DelegatorClient,scope_card_:String,)",
    ),
  )
}

///|
fn scope_card_test_agent(
  method_name : String,
  params : Array[(String, @lib.ParamType)],
) -> @lib.AgentInfo {
  {
    name: "Delegator",
    description: "",
    mode: @lib.AgentMode::Durable,
    constructor_params: [],
    constructor_description: "",
    constructor_prompt_hint: None,
    methods: [
      {
        name: method_name,
        description: "",
        prompt_hint: None,
        params,
        return_type: None,
        http_endpoint: None,
        read_only: None,
      },
    ],
    has_init: false,
    has_snapshottable: false,
    has_multipart_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: None,
  }
}

///|
test "generate_client_code renames scope_card away from every other parameter" {
  let agent = scope_card_test_agent("forward", [
    ("scope_card", @lib.ParamType::Simple("String")),
    ("scope_card_", @lib.ParamType::Simple("Int")),
    ("scope_card__", @lib.ParamType::Simple("Bool")),
  ])
  let output = @formatter.impls_to_string(@lib.generate_client_code([agent]))
    .replace_all(old=" ", new="")
    .replace_all(old="\n", new="")
  assert_true(
    output.contains(
      "pubasyncfnDelegatorClient::forward(self:DelegatorClient,scope_card___:String,scope_card_:Int,scope_card__:Bool,scope_card?:@permissions.ScopeCard,)",
    ),
  )
  assert_true(
    output.contains(
      "@agents.encode_wire_input_async([scope_card___,scope_card_,scope_card__])",
    ),
  )
}

///|
test "generate_client_code rejects a method named set_default_scope_card" {
  let agent = scope_card_test_agent("set_default_scope_card", [])
  match
    capture_agent_parse_error(fn() raise @lib.AgentParseError {
      @lib.generate_client_code([agent])
    }) {
    Err(@lib.AgentParseError(msg)) =>
      assert_true(
        msg.contains(
          "method name 'set_default_scope_card' is reserved for the generated client method",
        ),
      )
    Ok(_) => abort("expected the reserved method name to be rejected")
  }
}

///|
test "generate_client_code rejects a method named like another method's invocation helper" {
  let base = scope_card_test_agent("value", [])
  for helper in ["trigger_value", "schedule_value", "get_value_invocation"] {
    let agent : @lib.AgentInfo = {
      ..base,
      methods: [base.methods[0], { ..base.methods[0], name: helper, }],
    }
    match
      capture_agent_parse_error(fn() raise @lib.AgentParseError {
        @lib.generate_client_code([agent])
      }) {
      Err(@lib.AgentParseError(msg)) =>
        assert_true(
          msg.contains(
            "method name '\{helper}' is reserved for a generated invocation helper of method 'value'",
          ),
        )
      Ok(_) => abort("expected the helper name to be rejected")
    }
  }
}

///|
test "generate_client_code renames constructor parameters named like create options" {
  let agent : @lib.AgentInfo = {
    ..scope_card_test_agent("run", []),
    constructor_params: [
      ("keyed", @lib.ParamType::Simple("Bool")),
      ("env", @lib.ParamType::Simple("String")),
      ("keyed_", @lib.ParamType::Simple("Int")),
    ],
  }
  let output = @formatter.impls_to_string(@lib.generate_client_code([agent]))
    .replace_all(old=" ", new="")
    .replace_all(old="\n", new="")
  assert_true(
    output.contains(
      "pubasyncfnDelegatorClient::create(keyed__:Bool,env_:String,keyed_:Int,keyed?:Bool=false,env?:Array[(String,String)]=[],)->DelegatorClient{letconstructor_input=@agents.encode_wire_input([keyed__,env_,keyed_])",
    ),
  )
  // `upsert` has no labels, so its parameters keep their names.
  assert_true(
    output.contains(
      "pubfnDelegatorClient::upsert(keyed:Bool,env:String,keyed_:Int)->DelegatorClientraise{letconstructor_input=@agents.encode_wire_input([keyed,env,keyed_])",
    ),
  )
}

///|
test "generate_client_code - durable client maps to upsert, get_by_id, create and agent_id" {
  let agent = scope_card_test_agent("run", [])
  let output = @formatter.impls_to_string(@lib.generate_client_code([agent]))
    .replace_all(old=" ", new="")
    .replace_all(old="\n", new="")
  assert_true(
    output.contains(
      "letclient=@rpc.AgentClient::upsert(\"Delegator\",constructor_input)",
    ),
  )
  assert_true(
    output.contains(
      "pubfnDelegatorClient::get_by_id(agent_id:@schema.AgentId)->DelegatorClientraise@agentHost.RpcError{letclient=@rpc.AgentClient::get_by_id(\"Delegator\",agent_id)",
    ),
  )
  assert_true(
    output.contains(
      "@rpc.AgentClient::create(\"Delegator\",constructor_input,keyed~,env~)",
    ),
  )
  assert_true(
    output.contains(
      "pubasyncfnDelegatorClient::agent_id(self:DelegatorClient)->@schema.AgentId{self.client.agent_id()}",
    ),
  )
  assert_true(
    output.contains(
      "letclient=DelegatorClient::get_by_id(agent_id)deferclient.drop()",
    ),
  )
  assert_false(output.contains("phantom"))
}

///|
test "generate_client_code rejects a durable method named agent_id" {
  let agent = scope_card_test_agent("agent_id", [])
  match
    capture_agent_parse_error(fn() raise @lib.AgentParseError {
      @lib.generate_client_code([agent])
    }) {
    Err(@lib.AgentParseError(msg)) =>
      assert_true(
        msg.contains(
          "method name 'agent_id' is reserved for the generated method that returns the agent ID",
        ),
      )
    Ok(_) => abort("expected the reserved method name to be rejected")
  }
}
