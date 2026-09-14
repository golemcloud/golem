///|
fn role_error(f : () -> Unit raise @lib.ComponentRoleError) -> String {
  try f() catch {
    @lib.ComponentRoleError(message) => message
  } noraise {
    _ => "NO ERROR"
  }
}

///|
test "component roles map to exact generated packages" {
  let ordinary = @lib.parse_component_role("ordinary")
  let middleware = @lib.parse_component_role("tool-middleware")
  let combined = @lib.parse_component_role("combined")
  inspect(ordinary.gen_package(), content="gen")
  inspect(middleware.gen_package(), content="gen-tool-middleware")
  inspect(combined.gen_package(), content="gen-agent-tool-middleware")
  inspect(ordinary.allows_agent_registration(), content="true")
  inspect(middleware.allows_agent_registration(), content="false")
  inspect(middleware.allows_tool_registration(), content="false")
  inspect(combined.allows_tool_registration(), content="true")
  inspect(ordinary.allows_tool_middleware_registration(), content="false")
  inspect(middleware.allows_tool_middleware_registration(), content="true")
  inspect(combined.allows_tool_middleware_registration(), content="true")
}

///|
test "command parsers require valid explicit roles" {
  inspect(
    role_error(() => ignore(@lib.parse_component_role("pure"))),
    content="Invalid component role 'pure'; expected ordinary, tool-middleware, or combined",
  )
  inspect(
    role_error(() => {
      ignore(
        @lib.parse_agents_command_args([
          "project", "--component-dir", "component",
        ]),
      )
    }),
    content="Missing required --role option",
  )
  inspect(
    role_error(() => {
      ignore(@lib.parse_reexports_command_args(["sdk", "component", "--role"]))
    }),
    content="Missing value for --role",
  )
}

///|
test "command parsers produce one shared role model" {
  let agents = @lib.parse_agents_command_args([
    "project", "--role", "combined", "--component-dir", "components/main",
  ])
  inspect(agents.project_root, content="project")
  inspect(agents.component_dir, content="components/main")
  inspect(agents.role.name(), content="combined")
  let reexports = @lib.parse_reexports_command_args([
    "sdk", "component", "--role", "combined",
  ])
  inspect(reexports.sdk_path, content="sdk")
  inspect(reexports.target_dir, content="component")
  inspect(reexports.role.name(), content="combined")
}

///|
test "middleware annotations are detected from parsed attributes" {
  inspect(
    @lib.has_middleware_annotations([
      (
        "middleware.mbt",
        (
          #|#derive.tool_middleware("adapter", presented="Presented", expected="Expected", alias="one", alias="two")
          #|struct Adapter {}
          #|
        ),
      ),
    ]),
    content="true",
  )
  inspect(
    @lib.has_middleware_annotations([
      (
        "ordinary.mbt",
        (
          #|// #derive.tool_middleware("commented-out", presented="Tool")
          #|#derive.tool
          #|struct Tool {}
          #|
        ),
      ),
    ]),
    content="false",
  )
}

///|
test "role and source mismatches are rejected" {
  inspect(
    role_error(() => {
      @lib.validate_component_role(@lib.ComponentRole::Ordinary, 0, true)
    }),
    content="ordinary component role does not export tool middleware",
  )
  inspect(
    role_error(() => {
      @lib.validate_component_role(@lib.ComponentRole::ToolMiddleware, 1, false)
    }),
    content="tool-middleware component role does not export agents",
  )
  @lib.validate_component_role(@lib.ComponentRole::Combined, 1, true)
}

///|
test "component target and role marker are deterministic" {
  inspect(
    @lib.component_target_dir("project", "components/main"),
    content="project/components/main",
  )
  inspect(@lib.component_target_dir("project/", "."), content="project")
  let marker = @lib.component_role_marker(@lib.ComponentRole::ToolMiddleware)
  inspect(
    @lib.parse_component_role_marker(marker).name(),
    content="tool-middleware",
  )
}
