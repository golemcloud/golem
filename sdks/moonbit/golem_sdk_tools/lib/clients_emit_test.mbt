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
      #|"///|\npub(all) struct CounterClient {\n  client : @rpc.AgentClient\n}\n\n///|\npub fn CounterClient::get(name : String) -> CounterClient raise @common.AgentError {\n  let constructor_input = \n    @agents.encode_invocation_input([@schema.to_value_as(name)])\n  let client = @rpc.AgentClient::get(\"Counter\", constructor_input)\n  { client, }\n}\n\n///|\npub fn CounterClient::new_phantom(name : String) -> CounterClient raise @common.AgentError {\n  let constructor_input = \n    @agents.encode_invocation_input([@schema.to_value_as(name)])\n  let client = @rpc.AgentClient::new_phantom(\"Counter\", constructor_input)\n  { client, }\n}\n\n///|\npub fn CounterClient::get_phantom(name : String, phantom_id : @types.Uuid) -> CounterClient raise @common.AgentError {\n  let constructor_input = \n    @agents.encode_invocation_input([@schema.to_value_as(name)])\n  let client = \n    @rpc.AgentClient::get_phantom(\"Counter\", constructor_input, phantom_id)\n  { client, }\n}\n\n///|\npub fn CounterClient::get_agent_id(self : CounterClient) -> String raise @common.AgentError {\n  self.client.get_agent_id()\n}\n\n///|\npub fn CounterClient::phantom_id(self : CounterClient) -> @types.Uuid? {\n  self.client.phantom_id()\n}\n\n///|\npub fn CounterClient::drop(self : CounterClient) -> Unit {\n  self.client.drop()\n}\n\n///|\npub fn[T] CounterClient::scoped(\n  name : String,\n  f : (CounterClient) -> T raise @common.AgentError,\n) -> T raise @common.AgentError {\n  let client = CounterClient::get(name)\n  defer client.drop()\n  f(client)\n}\n\n///|\npub fn[T] CounterClient::scoped_new_phantom(\n  name : String,\n  f : (CounterClient) -> T raise @common.AgentError,\n) -> T raise @common.AgentError {\n  let client = CounterClient::new_phantom(name)\n  defer client.drop()\n  f(client)\n}\n\n///|\npub fn[T] CounterClient::scoped_get_phantom(\n  name : String,\n  phantom_id : @types.Uuid,\n  f : (CounterClient) -> T raise @common.AgentError,\n) -> T raise @common.AgentError {\n  let client = CounterClient::get_phantom(name, phantom_id)\n  defer client.drop()\n  f(client)\n}\n\n///|\npub fn CounterClient::increment(self : CounterClient) -> Unit raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  let _ = self.client.invoke_and_await(\"increment\", input)\n  \n}\n\n///|\npub fn CounterClient::trigger_increment(self : CounterClient) -> Unit raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  self.client.invoke(\"increment\", input)\n}\n\n///|\npub fn CounterClient::schedule_increment(\n  self : CounterClient,\n  scheduled_at : @wallClock.Datetime,\n) -> Unit raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  self.client.schedule_invocation(scheduled_at, \"increment\", input)\n}\n\n///|\npub fn CounterClient::schedule_cancelable_increment(\n  self : CounterClient,\n  scheduled_at : @wallClock.Datetime,\n) -> @agentHost.CancellationToken raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  self.client.schedule_cancelable_invocation(scheduled_at, \"increment\", input)\n}\n\n///|\npub fn CounterClient::get_value(self : CounterClient) -> UInt64 raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  let result = self.client.invoke_and_await(\"get_value\", input)\n  match result {\n    Some(tree) => @agents.decode_output(tree, \"Counter.get_value\")\n    None => {\n      raise  @common.AgentError::InvalidType(\n        \"Counter.get_value: expected a return value, got none\",\n      )\n    }\n  }\n}\n\n///|\npub fn CounterClient::trigger_get_value(self : CounterClient) -> Unit raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  self.client.invoke(\"get_value\", input)\n}\n\n///|\npub fn CounterClient::schedule_get_value(\n  self : CounterClient,\n  scheduled_at : @wallClock.Datetime,\n) -> Unit raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  self.client.schedule_invocation(scheduled_at, \"get_value\", input)\n}\n\n///|\npub fn CounterClient::schedule_cancelable_get_value(\n  self : CounterClient,\n  scheduled_at : @wallClock.Datetime,\n) -> @agentHost.CancellationToken raise @common.AgentError {\n  let input = @agents.encode_invocation_input([])\n  self.client.schedule_cancelable_invocation(scheduled_at, \"get_value\", input)\n}\n\n"
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
      #|"///|\npub(all) struct TaskManagerClient {\n  client : @rpc.AgentClient\n}\n\n///|\npub fn TaskManagerClient::get() -> TaskManagerClient raise @common.AgentError {\n  let constructor_input = @agents.encode_invocation_input([])\n  let client = @rpc.AgentClient::get(\"TaskManager\", constructor_input)\n  { client, }\n}\n\n///|\npub fn TaskManagerClient::new_phantom() -> TaskManagerClient raise @common.AgentError {\n  let constructor_input = @agents.encode_invocation_input([])\n  let client = @rpc.AgentClient::new_phantom(\"TaskManager\", constructor_input)\n  { client, }\n}\n\n///|\npub fn TaskManagerClient::get_phantom(phantom_id : @types.Uuid) -> TaskManagerClient raise @common.AgentError {\n  let constructor_input = @agents.encode_invocation_input([])\n  let client = \n    @rpc.AgentClient::get_phantom(\"TaskManager\", constructor_input, phantom_id)\n  { client, }\n}\n\n///|\npub fn TaskManagerClient::get_agent_id(self : TaskManagerClient) -> String raise @common.AgentError {\n  self.client.get_agent_id()\n}\n\n///|\npub fn TaskManagerClient::phantom_id(self : TaskManagerClient) -> @types.Uuid? {\n  self.client.phantom_id()\n}\n\n///|\npub fn TaskManagerClient::drop(self : TaskManagerClient) -> Unit {\n  self.client.drop()\n}\n\n///|\npub fn[T] TaskManagerClient::scoped(\n  f : (TaskManagerClient) -> T raise @common.AgentError,\n) -> T raise @common.AgentError {\n  let client = TaskManagerClient::get()\n  defer client.drop()\n  f(client)\n}\n\n///|\npub fn[T] TaskManagerClient::scoped_new_phantom(\n  f : (TaskManagerClient) -> T raise @common.AgentError,\n) -> T raise @common.AgentError {\n  let client = TaskManagerClient::new_phantom()\n  defer client.drop()\n  f(client)\n}\n\n///|\npub fn[T] TaskManagerClient::scoped_get_phantom(\n  phantom_id : @types.Uuid,\n  f : (TaskManagerClient) -> T raise @common.AgentError,\n) -> T raise @common.AgentError {\n  let client = TaskManagerClient::get_phantom(phantom_id)\n  defer client.drop()\n  f(client)\n}\n\n///|\npub fn TaskManagerClient::add_task(self : TaskManagerClient, task : TaskInfo) -> UInt64 raise @common.AgentError {\n  let input = @agents.encode_invocation_input([@schema.to_value_as(task)])\n  let result = self.client.invoke_and_await(\"add_task\", input)\n  match result {\n    Some(tree) => @agents.decode_output(tree, \"TaskManager.add_task\")\n    None => {\n      raise  @common.AgentError::InvalidType(\n        \"TaskManager.add_task: expected a return value, got none\",\n      )\n    }\n  }\n}\n\n///|\npub fn TaskManagerClient::trigger_add_task(\n  self : TaskManagerClient,\n  task : TaskInfo,\n) -> Unit raise @common.AgentError {\n  let input = @agents.encode_invocation_input([@schema.to_value_as(task)])\n  self.client.invoke(\"add_task\", input)\n}\n\n///|\npub fn TaskManagerClient::schedule_add_task(\n  self : TaskManagerClient,\n  scheduled_at : @wallClock.Datetime,\n  task : TaskInfo,\n) -> Unit raise @common.AgentError {\n  let input = @agents.encode_invocation_input([@schema.to_value_as(task)])\n  self.client.schedule_invocation(scheduled_at, \"add_task\", input)\n}\n\n///|\npub fn TaskManagerClient::schedule_cancelable_add_task(\n  self : TaskManagerClient,\n  scheduled_at : @wallClock.Datetime,\n  task : TaskInfo,\n) -> @agentHost.CancellationToken raise @common.AgentError {\n  let input = @agents.encode_invocation_input([@schema.to_value_as(task)])\n  self.client.schedule_cancelable_invocation(scheduled_at, \"add_task\", input)\n}\n\n"
    ),
  )
}

///|
test "generate_client_code - all secret config generates no with_config methods" {
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
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("AllSecretConfig"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "AllSecretConfig",
      package: "",
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
  assert_false(output.contains("with_config"))
}

///|
test "generate_client_code - mixed config generates with_config methods" {
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
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("MixedConfig"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "MixedConfig",
      package: "",
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
  // Should contain the 6 with_config variants
  assert_true(output.contains("get_with_config"))
  assert_true(output.contains("new_phantom_with_config"))
  assert_true(output.contains("get_phantom_with_config"))
  assert_true(output.contains("scoped_with_config"))
  assert_true(output.contains("scoped_new_phantom_with_config"))
  assert_true(output.contains("scoped_get_phantom_with_config"))
  // Should reference the override type and walker
  assert_true(output.contains("MixedConfigOverride"))
  assert_true(output.contains("collect_mixed_config_overrides"))
  // Should call the _with_config runtime methods
  assert_true(output.contains("AgentClient::get_with_config"))
  assert_true(output.contains("AgentClient::new_phantom_with_config"))
  assert_true(output.contains("AgentClient::get_phantom_with_config"))
}

///|
test "generate_client_code - ephemeral agent skips non-phantom constructors" {
  let agent : @lib.AgentInfo = {
    name: "EphemeralWorker",
    description: "An ephemeral worker",
    mode: @lib.AgentMode::Ephemeral,
    constructor_params: [("name", @lib.ParamType::Simple("String"))],
    constructor_description: "Creates an ephemeral worker",
    constructor_prompt_hint: None,
    methods: [],
    has_init: false,
    has_snapshottable: false,
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("MixedConfig"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "MixedConfig",
      package: "",
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

  assert_false(output.contains("pub fn EphemeralWorkerClient::get("))
  assert_false(output.contains("get_with_config"))
  assert_false(output.contains("scoped_with_config"))
  assert_true(output.contains("new_phantom_with_config"))
  assert_true(output.contains("get_phantom_with_config"))
  assert_true(output.contains("scoped_new_phantom_with_config"))
  assert_true(output.contains("scoped_get_phantom_with_config"))
}

///|
test "generate_config_override_types - flat config" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "MyConfig",
      package: "",
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
      package: "",
      fields: [
        ("host", @lib.ParamType::Simple("String")),
        ("port", @lib.ParamType::Simple("UInt")),
      ],
    },
    {
      name: "AppConfig",
      package: "",
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
      package: "",
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
  // Secret fields should not appear
  assert_false(output.contains("api_key"))
}

///|
test "generate_config_override_walkers - nested config" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "DbConfig",
      package: "",
      fields: [("host", @lib.ParamType::Simple("String"))],
    },
    {
      name: "AppConfig",
      package: "",
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
      package: "",
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
      package: "",
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
    package: "",
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
    package: "",
    fields: [("nested", @lib.ParamType::Simple("SecretsOnly"))],
  }
  // Parent that also has a plain leaf => overridable.
  let parent_with_leaf : @lib.ConfigTypeInfo = {
    name: "ParentWithLeaf",
    package: "",
    fields: [
      ("nested", @lib.ParamType::Simple("SecretsOnly")),
      ("name", @lib.ParamType::Simple("String")),
    ],
  }
  let by_name : Map[String, @lib.ConfigTypeInfo] = {}
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
      package: "",
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
      package: "",
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
      package: "",
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
  match (try? @lib.validate_config_types([], config_types)) {
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
    has_json_derives: false,
    snapshotting: Disabled,
    fields: [],
    http_mount: None,
    config_type: Some("SecretsOnly"),
  }
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "SecretsOnly",
      package: "",
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
  assert_false(output.contains("with_config"))
}

///|
test "validate_config_types - cross-package nested config is rejected" {
  let config_types : Array[@lib.ConfigTypeInfo] = [
    {
      name: "DbConfig",
      package: "models",
      fields: [("host", @lib.ParamType::Simple("String"))],
    },
    {
      name: "AppConfig",
      package: "app",
      fields: [("db", @lib.ParamType::Simple("@models.DbConfig"))],
    },
  ]
  match (try? @lib.validate_config_types([], config_types)) {
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
      package: "app",
      fields: [("host", @lib.ParamType::Simple("String"))],
    },
    {
      name: "AppConfig",
      package: "app",
      fields: [("db", @lib.ParamType::Simple("@app.DbConfig"))],
    },
  ]
  match (try? @lib.validate_config_types([], config_types)) {
    Ok(_) => ()
    Err(@lib.AgentParseError(msg)) =>
      abort("same-package qualified ref should be allowed, got: " + msg)
  }
}
