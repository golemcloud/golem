// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Per-subcommand `Examples:` blocks attached to clap definitions via
//! `#[command(after_help = ...)]`. These are surfaced in `--help` output and
//! are intended to give both humans and coding agents copy-pasteable starting
//! points for the most common usage of every CLI command. Example invocations
//! are drawn from the bundled skills under `golem-skills/skills/`.

// Top-level / app-scoped commands -----------------------------------------------------------------

pub const NEW: &str = "Examples:
  # List available templates first
  golem-cli templates

  # Scaffold a new Rust application in ./my-app
  golem-cli new --template rust --yes my-app

  # Scaffold a new TypeScript application in the current directory
  golem-cli new --template ts --yes .

  # Add a new component to an existing application (run from app root)
  golem-cli new --template rust --component-name myapp:billing-service --yes .

  # Use a sub-template (e.g. snapshotting variant)
  golem-cli new --template rust/snapshotting --component-name myapp:orders --yes .";

pub const TEMPLATES: &str = "Examples:
  # List all available templates
  golem-cli templates

  # Filter by language or template name
  golem-cli templates rust
  golem-cli templates snapshotting";

pub const BUILD: &str = "Examples:
  # Build all components in the current application
  golem-cli build --yes

  # Build only selected components
  golem-cli build my-component another-component --yes

  # Run only the source-check step (fast, no compile)
  golem-cli build --step check --yes

  # Force a rebuild ignoring up-to-date checks
  golem-cli build --force-build --yes";

pub const GENERATE_BRIDGE: &str = "Examples:
  # Generate bridge SDK(s) for every agent in the application,
  # using each agent's own language by default
  golem-cli generate-bridge

  # Generate a TypeScript bridge SDK for a specific agent type
  golem-cli generate-bridge --language ts --agent-type-name CounterAgent

  # Restrict to one component and write into a chosen output directory
  golem-cli generate-bridge --component-name my-component --output-dir ./bridges";

pub const REPL: &str = "Examples:
  # Start the REPL for the component selected by the current directory
  golem-cli repl

  # Pick a specific component and language
  golem-cli repl my-component --language ts

  # Run a script and exit (script is in the component's language)
  golem-cli repl --script-file test.ts --yes

  # Run a one-liner script
  golem-cli repl --script 'agent.greet(\"Alice\")' --yes

  # Always start from a clean state (delete agents and environment)
  golem-cli repl --reset";

pub const DEPLOY: &str = "Examples:
  # Build, upload and activate everything (always pass --yes for non-interactive use)
  golem-cli deploy --yes

  # Iterative development: wipe agents and environment, then redeploy
  golem-cli deploy --yes --reset

  # Update existing running agents to the new component version
  golem-cli deploy --yes --update-agents automatic

  # Delete and recreate all agents using the new version
  golem-cli deploy --yes --redeploy-agents

  # Show what would change without applying anything
  golem-cli deploy --plan

  # Roll back to a specific deployment revision or version tag
  golem-cli deploy --revision 3 --yes
  golem-cli deploy --version v1.2.0 --yes

  # Deploy to a specific environment
  golem-cli deploy --yes -E staging
  golem-cli deploy --yes --cloud";

pub const CLEAN: &str = "Examples:
  # Clean all components in the application
  golem-cli clean

  # Clean only selected components
  golem-cli clean my-component another-component";

pub const UPDATE_AGENTS: &str = "Examples:
  # Try to automatically update all running agents of the application
  golem-cli update-agents

  # Restrict to selected components, manual mode, and wait for completion
  golem-cli update-agents my-component --update-mode manual --await

  # Do not wake up suspended agents (update applies on next wake-up)
  golem-cli update-agents --disable-wakeup";

pub const REDEPLOY_AGENTS: &str = "Examples:
  # Delete and recreate all running agents of the application using the current version
  golem-cli redeploy-agents

  # Restrict to selected components
  golem-cli redeploy-agents my-component";

pub const EXEC: &str = "Examples:
  # Custom commands are defined under `commands:` in the application's golem.yaml.
  # Discover what is available by running --help inside an application directory:
  golem-cli exec --help

  # Then run one of the listed commands
  golem-cli exec test
  golem-cli exec lint";

pub const OUTPUT_SCHEMA: &str = "Examples:
  # Print the full structured output JSON Schema
  golem-cli output-schema

  # List known structured output type names
  golem-cli output-schema --types

  # Print a pruned schema for one output type
  golem-cli output-schema --type agent.invoke

  # Print a pruned schema bundle for multiple output types
  golem-cli output-schema --type agent.oplog --type agent.stream

  # Use a different raw schema output format
  golem-cli --format yaml output-schema --type agent.invoke";

pub const COMPLETION: &str = "Examples:
  # Print bash completions to stdout
  golem-cli completion bash

  # Install bash completions for the current user
  golem-cli completion bash > ~/.local/share/bash-completion/completions/golem-cli

  # Other shells
  golem-cli completion zsh
  golem-cli completion fish
  golem-cli completion powershell";

// Agent commands -----------------------------------------------------------------------------------

pub const AGENT_NEW: &str = "Examples:
  # Create an instance of an agent with no parameters
  golem-cli agent new 'MyAgent()'

  # Create an instance with a string parameter (note the shell quoting)
  golem-cli agent new 'ChatRoom(\"general\")'

  # Provide environment variables visible to the agent
  golem-cli agent new 'MyAgent(\"u-123\")' --env API_URL=https://api.example.com --env LOG_LEVEL=debug

  # Provide configuration values declared by the agent
  golem-cli agent new 'MyAgent()' --config max_retries=5 --config timeout_seconds=30

  # Address an agent in a specific environment
  golem-cli agent new 'staging/MyAgent(\"u-123\")'";

pub const AGENT_INVOKE: &str = "Examples:
  # Invoke a function with no arguments
  golem-cli agent invoke 'MyAgent()' get_status

  # Pass arguments using the agent's language syntax (Rust shown here);
  # quote the whole AGENT_ID and string arguments to protect them from the shell
  golem-cli agent invoke 'MyAgent(\"u-123\")' process_order '\"order-456\"' 42

  # Pass a record literal
  golem-cli agent invoke 'MyAgent(\"u-123\")' update_profile 'MyProfile { display_name: \"Alice\", age: Some(30) }'

  # Fire-and-forget (do not wait for the result)
  golem-cli agent invoke -t 'MyAgent()' start_background_job '\"job-123\"'

  # Use an explicit idempotency key (use \"-\" to auto-generate one)
  golem-cli agent invoke -i my-unique-key 'MyAgent()' do_work
  golem-cli agent invoke -i - 'MyAgent()' do_work

  # Schedule for the future (RFC 3339 / ISO 8601, UTC)
  golem-cli agent invoke --schedule-at 2026-03-15T10:30:00Z 'Reporter()' send_report

  # Stream only log entries, no invocation markers
  golem-cli agent invoke --logs-only 'MyAgent()' run

  # Address an agent in a specific environment
  golem-cli agent invoke 'staging/MyAgent(\"u-123\")' get_status";

pub const AGENT_GET: &str = "Examples:
  # Get full metadata for an agent (status, version, env, config, etc.)
  golem-cli agent get 'CounterAgent(\"my-counter\")'

  # Address an agent in a non-default environment
  golem-cli agent get 'staging/CounterAgent(\"my-counter\")'

  # Machine-readable output
  golem-cli agent get 'CounterAgent(\"my-counter\")' --format json";

pub const AGENT_DELETE: &str = "Examples:
  # Delete an agent (this drops its state permanently)
  golem-cli agent delete 'CounterAgent(\"my-counter\")'

  # Address an agent in a non-default environment
  golem-cli agent delete 'staging/CounterAgent(\"my-counter\")'";

pub const AGENT_LIST: &str = "Examples:
  # List all (durable) agents in the current environment
  golem-cli agent list

  # List agents of a specific agent type
  golem-cli agent list CounterAgent

  # List agents belonging to a specific component
  golem-cli agent list --component-name my-component

  # Filter on metadata; multiple --filter flags are AND-combined
  golem-cli agent list --filter 'status == Running' --filter 'name like %counter%'
  golem-cli agent list --filter 'env.region == us-east'

  # Include ephemeral agents (or only ephemeral ones)
  golem-cli agent list --mode all
  golem-cli agent list --mode ephemeral

  # Pagination
  golem-cli agent list --max-count 10
  golem-cli agent list --max-count 10 --scan-cursor 0/10

  # Force fresh status for each agent
  golem-cli agent list --precise

  # Watch mode (text output only, default 400ms refresh)
  golem-cli agent list --refresh
  golem-cli agent list --refresh=1000";

pub const AGENT_STREAM: &str = "Examples:
  # Stream stdout / stderr / log output from an agent
  golem-cli agent stream 'CounterAgent(\"my-counter\")'

  # Hide log levels and timestamps; show only payload entries
  golem-cli agent stream 'CounterAgent(\"my-counter\")' --stream-no-timestamp --stream-no-log-level --logs-only

  # In structured formats, each stream event is emitted as a separate document
  golem-cli --format json agent stream 'CounterAgent(\"my-counter\")'";

pub const AGENT_UPDATE: &str = "Examples:
  # Update one agent in automatic mode (default), to the current deployed revision
  golem-cli agent update 'Counter(\"my-counter\")'

  # Switch to manual update mode
  golem-cli agent update 'Counter(\"my-counter\")' manual

  # Pin to a specific target revision and wait for completion
  golem-cli agent update 'Counter(\"my-counter\")' automatic 3 --await

  # Do not wake suspended agents; update applies on next wake-up
  golem-cli agent update 'Counter(\"my-counter\")' --disable-wakeup";

pub const AGENT_INTERRUPT: &str = "Examples:
  # Interrupt a running agent
  golem-cli agent interrupt 'CounterAgent(\"my-counter\")'

  # Address an agent in a non-default environment
  golem-cli agent interrupt 'staging/CounterAgent(\"my-counter\")'";

pub const AGENT_RESUME: &str = "Examples:
  # Resume an interrupted agent
  golem-cli agent resume 'CounterAgent(\"my-counter\")'

  # Address an agent in a non-default environment
  golem-cli agent resume 'staging/CounterAgent(\"my-counter\")'";

pub const AGENT_SIMULATE_CRASH: &str = "Examples:
  # Simulate a crash on an agent (it recovers automatically)
  golem-cli agent simulate-crash 'CounterAgent(\"my-counter\")'

  # Useful test loop: invoke, crash, invoke again, verify state survived
  golem-cli agent invoke   'Counter(\"c1\")' increment
  golem-cli agent simulate-crash 'Counter(\"c1\")'
  golem-cli agent invoke   'Counter(\"c1\")' get_value";

pub const AGENT_OPLOG: &str = "Examples:
  # Stream the entire oplog of an agent
  golem-cli agent oplog 'CounterAgent(\"my-counter\")'

  # Continue from a specific oplog index
  golem-cli agent oplog 'CounterAgent(\"my-counter\")' --from 100

  # Filter using a Lucene query
  golem-cli agent oplog 'CounterAgent(\"my-counter\")' --query 'error'
  golem-cli agent oplog 'CounterAgent(\"my-counter\")' --query 'function_name:increment'

  # In structured formats, each oplog entry is emitted as a separate document
  golem-cli --format json agent oplog 'CounterAgent(\"my-counter\")'";

pub const AGENT_REVERT: &str = "Examples:
  # Undo the agent's last N invocations
  golem-cli agent revert 'CounterAgent(\"c1\")' --number-of-invocations 1

  # Or revert state to a specific oplog index (use `agent oplog` to find one)
  golem-cli agent revert 'CounterAgent(\"c1\")' --last-oplog-index 42

  # The two flags are mutually exclusive";

pub const AGENT_CANCEL_INVOCATION: &str = "Examples:
  # Cancel a still-enqueued invocation by its idempotency key
  golem-cli agent cancel-invocation 'CounterAgent(\"my-counter\")' my-key-123

  # Address an agent in a non-default environment
  golem-cli agent cancel-invocation 'staging/CounterAgent(\"my-counter\")' my-key-123";

pub const AGENT_FILES: &str = "Examples:
  # List the agent's root filesystem
  golem-cli agent files 'CounterAgent(\"c1\")'

  # List a specific directory
  golem-cli agent files 'CounterAgent(\"c1\")' /data

  # Machine-readable output
  golem-cli agent files 'CounterAgent(\"c1\")' --format json";

pub const AGENT_FILE_CONTENTS: &str = "Examples:
  # Save a file using its guest basename in the current directory
  golem-cli agent file-contents 'CounterAgent(\"c1\")' /data/log.txt

  # Save the contents to a local file
  golem-cli agent file-contents 'CounterAgent(\"c1\")' /data/log.txt --output ./log.txt

  # Machine-readable metadata about the saved file
  golem-cli --format json agent file-contents 'CounterAgent(\"c1\")' /data/log.txt --output ./log.txt";

pub const AGENT_ACTIVATE_PLUGIN: &str = "Examples:
  # First list installed plugins for the agent's component (and their priorities):
  golem-cli component get my-component

  # Activate by plugin name
  golem-cli agent activate-plugin --plugin-name my-plugin 'CounterAgent(\"c1\")'

  # If multiple installations of the same plugin exist, disambiguate by priority
  golem-cli agent activate-plugin --plugin-name my-plugin --plugin-priority 10 'CounterAgent(\"c1\")'";

pub const AGENT_DEACTIVATE_PLUGIN: &str = "Examples:
  # First list installed plugins for the agent's component (and their priorities):
  golem-cli component get my-component

  # Deactivate by plugin name
  golem-cli agent deactivate-plugin --plugin-name my-plugin 'CounterAgent(\"c1\")'

  # If multiple installations of the same plugin exist, disambiguate by priority
  golem-cli agent deactivate-plugin --plugin-name my-plugin --plugin-priority 10 'CounterAgent(\"c1\")'";

// Agent type commands ------------------------------------------------------------------------------

pub const AGENT_TYPE_LIST: &str = "Examples:
  # List all deployed agent types in the current environment
  golem-cli agent-type list

  # JSON output for scripting
  golem-cli agent-type list --format json";

pub const AGENT_TYPE_GET: &str = "Examples:
  # Show metadata of a deployed agent type (functions, parameters, mode, ...)
  golem-cli agent-type get CounterAgent

  # JSON output
  golem-cli agent-type get CounterAgent --format json";

// Component commands -------------------------------------------------------------------------------

pub const COMPONENT_LIST: &str = "Examples:
  # List all deployed components in the current environment
  golem-cli component list

  # JSON output for scripting
  golem-cli component list --format json";

pub const COMPONENT_GET: &str = "Examples:
  # Get current revision metadata for the component selected by cwd
  golem-cli component get

  # Pick a component by name
  golem-cli component get my-component

  # Pick a specific revision
  golem-cli component get my-component 3

  # The output also lists installed plugins per agent-type, with priorities
  golem-cli component get my-component --format json";

pub const COMPONENT_UPDATE_AGENTS: &str = "Examples:
  # Update all running agents of the selected component to the current version
  golem-cli component update-agents my-component

  # Manual update mode, wait for completion
  golem-cli component update-agents my-component --update-mode manual --await

  # Do not wake suspended agents
  golem-cli component update-agents my-component --disable-wakeup";

pub const COMPONENT_REDEPLOY_AGENTS: &str = "Examples:
  # Delete and recreate all running agents of the selected component
  golem-cli component redeploy-agents my-component";

pub const COMPONENT_MANIFEST_TRACE: &str = "Examples:
  # Show component manifest properties together with which manifest file/line
  # each value originates from (useful for debugging multi-file manifests)
  golem-cli component manifest-trace
  golem-cli component manifest-trace my-component";

// Environment commands -----------------------------------------------------------------------------

pub const ENVIRONMENT_LIST: &str = "Examples:
  # List application environments known to the current server
  golem-cli environment list

  # Restrict to a specific server profile
  golem-cli environment list --profile cloud";

pub const ENVIRONMENT_SYNC_DEPLOYMENT_OPTIONS: &str = "Examples:
  # Compare the application manifest's deployment options with what is on the
  # selected environment, and apply differences after confirmation
  golem-cli environment sync-deployment-options

  # Non-interactive use
  golem-cli environment sync-deployment-options --yes";

// API commands -------------------------------------------------------------------------------------

pub const API_DEPLOYMENT_GET: &str = "Examples:
  # Show the API deployment serving the given domain
  golem-cli api deployment get api.example.com";

pub const API_DEPLOYMENT_LIST: &str = "Examples:
  # List API deployments in the current environment
  golem-cli api deployment list";

pub const API_SECURITY_SCHEME_CREATE: &str = "Examples:
  # Create a Google OIDC security scheme
  golem-cli api security-scheme create my-oidc \\
    --provider-type google \\
    --client-id my-client-id \\
    --client-secret my-client-secret \\
    --scope openid --scope email \\
    --redirect-url https://api.example.com/auth/callback

  # Create a custom OIDC provider (issuer URL and display name required)
  golem-cli api security-scheme create my-custom \\
    --provider-type custom \\
    --custom-provider-name MyOIDC \\
    --custom-issuer-url https://issuer.example.com \\
    --client-id my-client-id \\
    --client-secret my-client-secret \\
    --redirect-url https://api.example.com/auth/callback";

pub const API_SECURITY_SCHEME_GET: &str = "Examples:
  # Show details of a security scheme
  golem-cli api security-scheme get my-oidc";

pub const API_SECURITY_SCHEME_UPDATE: &str = "Examples:
  # Replace the scope list of an existing security scheme
  golem-cli api security-scheme update my-oidc --scope openid --scope profile

  # Rotate the client secret
  golem-cli api security-scheme update my-oidc --client-secret new-secret";

pub const API_SECURITY_SCHEME_DELETE: &str = "Examples:
  # Delete a security scheme
  golem-cli api security-scheme delete my-oidc";

pub const API_SECURITY_SCHEME_LIST: &str = "Examples:
  # List all security schemes in the current environment
  golem-cli api security-scheme list";

pub const API_DOMAIN_LIST: &str = "Examples:
  # List registered API domains
  golem-cli api domain list";

pub const API_DOMAIN_REGISTER: &str = "Examples:
  # Register a new domain (DNS for the domain must be prepared separately)
  golem-cli api domain register api.example.com";

pub const API_DOMAIN_DELETE: &str = "Examples:
  # Unregister a domain
  golem-cli api domain delete api.example.com";

// Plugin commands ----------------------------------------------------------------------------------

pub const PLUGIN_LIST: &str = "Examples:
  # List all plugins registered for the current account
  golem-cli plugin list";

pub const PLUGIN_GET: &str = "Examples:
  # Show details of a registered plugin
  golem-cli plugin get 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890";

pub const PLUGIN_REGISTER: &str = "Examples:
  # Register a plugin from a manifest file on disk
  golem-cli plugin register ./my-plugin.json

  # Read the manifest from stdin (e.g. piped from a generator)
  cat my-plugin.json | golem-cli plugin register -";

pub const PLUGIN_UNREGISTER: &str = "Examples:
  # Unregister a plugin by ID (use `plugin list` / `plugin get` to find IDs)
  golem-cli plugin unregister 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890";

// Profile commands ---------------------------------------------------------------------------------

pub const PROFILE_NEW: &str = "Examples:
  # Interactive setup
  golem-cli profile new

  # Non-interactive: define a profile pointing at a self-hosted server with a static token
  golem-cli profile new my-staging \\
    --url https://staging.example.com \\
    --static-token sometoken \\
    --default-format json \\
    --set-active";

pub const PROFILE_LIST: &str = "Examples:
  # List all global CLI profiles
  golem-cli profile list";

pub const PROFILE_SWITCH: &str = "Examples:
  # Make a different profile the active default
  golem-cli profile switch my-staging";

pub const PROFILE_GET: &str = "Examples:
  # Show details of the active profile
  golem-cli profile get

  # Show details of a specific profile
  golem-cli profile get my-staging";

pub const PROFILE_DELETE: &str = "Examples:
  # Remove a profile from the global config
  golem-cli profile delete my-staging";

pub const PROFILE_CONFIG_SET_FORMAT: &str = "Examples:
  # Set the default output format for a profile
  golem-cli profile config my-staging set-format json
  golem-cli profile config my-staging set-format toon
  golem-cli profile config my-staging set-format text";

// Server commands ----------------------------------------------------------------------------------

pub const SERVER_RUN: &str = "Examples:
  # Run a local Golem server with default ports
  golem-cli server run

  # Bind the main API to a different port
  golem-cli server run --router-port 8080

  # Start from a clean data directory
  golem-cli server run --clean

  # Persist data and agent filesystems in a chosen location
  golem-cli server run --data-dir ./my-data --agent-filesystem-root ./agent-fs

  # Write actual port assignments to a JSON file (handy for scripts)
  golem-cli server run --ports-file ./ports.json";

pub const SERVER_CLEAN: &str = "Examples:
  # Wipe the local server's persistent data directory
  golem-cli server clean";

// Account commands ---------------------------------------------------------------------------------

pub const ACCOUNT_GET: &str = "Examples:
  # Show details of the currently authenticated account
  golem-cli account get

  # Show details of a specific account by ID
  golem-cli account get --account-id acc-12345";

pub const ACCOUNT_UPDATE: &str = "Examples:
   # Update the current account's name
   golem-cli account update 'Alice Smith'

   # Update a specific account by ID
   golem-cli account update 'Alice Smith' --account-id acc-12345";

pub const ACCOUNT_NEW: &str = "Examples:
  # Add a new account
  golem-cli account new 'Alice Smith' alice@example.com";

pub const ACCOUNT_DELETE: &str = "Examples:
  # Delete the current account
  golem-cli account delete

  # Delete a specific account by ID
  golem-cli account delete --account-id acc-12345";

pub const ACCOUNT_PERMISSION_SHARE_LIST: &str = "Examples:
  # List permission shares owned by the current account
  golem-cli account permission-share list

  # List permission shares received by the current account
  golem-cli account permission-share list --received";

pub const ACCOUNT_PERMISSION_SHARE_GET: &str = "Examples:
  # Get a permission share by ID
  golem-cli account permission-share get 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890";

pub const ACCOUNT_PERMISSION_SHARE_GET_BY_NAME: &str = "Examples:
  # Get a permission share by name from the current account
  golem-cli account permission-share get-by-name staging-access";

pub const ACCOUNT_PERMISSION_SHARE_NEW: &str = "Examples:
  # Share permissions with another account
  golem-cli account permission-share new target@example.com staging-access \
    --lower-positive 'environment(my-account/my-app) @ target@example.com : view : staging' \
    --lower-positive 'component(my-account/my-app/staging) @ target@example.com : view : *'

  # Add a lower negative grant by repeating the flag
  golem-cli account permission-share new target@example.com staging-access \
    --lower-positive 'environment(my-account/my-app) @ target@example.com : view : staging' \
    --lower-negative 'component(my-account/my-app/staging) @ target@example.com : delete : *'";

pub const ACCOUNT_PERMISSION_SHARE_UPDATE: &str = "Examples:
  # Replace lower permission grants on an existing share
  golem-cli account permission-share update 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890 \
    --lower-positive 'environment(my-account/my-app) @ target@example.com : view : staging'

  # Rename while replacing grants
  golem-cli account permission-share update 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890 \
    --name staging-access-v2 \
    --lower-positive 'environment(my-account/my-app) @ target@example.com : view : staging'";

pub const ACCOUNT_PERMISSION_SHARE_DELETE: &str = "Examples:
  # Delete a permission share by ID
  golem-cli account permission-share delete 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890";

// API token commands -------------------------------------------------------------------------------

pub const API_TOKEN_LIST: &str = "Examples:
  # List API tokens for the current account
  golem-cli api-token list";

pub const API_TOKEN_NEW: &str = "Examples:
  # Create a new long-lived token (default expires year 2100)
  golem-cli api-token new

  # Create a token that expires on a specific RFC 3339 timestamp
  golem-cli api-token new --expires-at 2027-01-01T00:00:00Z";

pub const API_TOKEN_DELETE: &str = "Examples:
  # Delete a token by ID (use `api-token list` to find IDs)
  golem-cli api-token delete 8fd5e4a2-9cab-4f8e-9d3a-1c2e4f567890";

// Secret commands ----------------------------------------------------------------------------------

pub const SECRET_CREATE: &str = "Examples:
  # Create a string secret in the current environment
  golem-cli secret create apiKey --secret-type String --secret-value 'sk-abc123'

  # Nested path (paths are dot-separated; casing is normalized)
  golem-cli secret create db.password --secret-type String --secret-value 's3cret'

  # Type and value use the project's language syntax (or JSON):
  #   --secret-type String   for Rust
  #   --secret-type string   for TypeScript";

pub const SECRET_GET: &str = "Examples:
  # Get a secret by path
  golem-cli secret get apiKey

  # Get a secret by ID
  golem-cli secret get --id sec-12345";

pub const SECRET_UPDATE_VALUE: &str = "Examples:
  # Update a secret's value (path or --id)
  golem-cli secret update-value apiKey --secret-value 'new-value'
  golem-cli secret update-value --id sec-12345 --secret-value 'new-value'";

pub const SECRET_DELETE: &str = "Examples:
  # Delete a secret by path or by ID
  golem-cli secret delete apiKey
  golem-cli secret delete --id sec-12345";

pub const SECRET_LIST: &str = "Examples:
  # List secrets in the current environment
  golem-cli secret list

  # Include environment ID and secret ID columns in text output
  golem-cli secret list --ids";

// Retry policy commands ----------------------------------------------------------------------------

pub const RETRY_POLICY_CREATE: &str = "Examples:
  # Create a retry policy for transient HTTP errors
  golem-cli retry-policy create http-transient \\
    --priority 10 \\
    --predicate '{ \"propIn\": { \"property\": \"status-code\", \"values\": [502, 503, 504] } }' \\
    --policy '{ \"countBox\": { \"maxRetries\": 5, \"inner\": { \"exponential\": { \"baseDelay\": \"200ms\", \"factor\": 2.0 } } } }'";

pub const RETRY_POLICY_LIST: &str = "Examples:
  # List retry policies in the current environment
  golem-cli retry-policy list";

pub const RETRY_POLICY_GET: &str = "Examples:
  # Get a retry policy by name
  golem-cli retry-policy get http-transient

  # Or by ID
  golem-cli retry-policy get --id rp-12345";

pub const RETRY_POLICY_UPDATE: &str = "Examples:
  # Change priority of an existing policy
  golem-cli retry-policy update http-transient --priority 15

  # Replace the predicate or policy JSON
  golem-cli retry-policy update http-transient --predicate '{ \"propIn\": { \"property\": \"status-code\", \"values\": [503, 504] } }'";

pub const RETRY_POLICY_DELETE: &str = "Examples:
  # Delete a retry policy by name or by ID
  golem-cli retry-policy delete http-transient
  golem-cli retry-policy delete --id rp-12345";

// Resource (quota) commands ------------------------------------------------------------------------

pub const RESOURCE_CREATE: &str = "Examples:
  # A rate-based quota: 100 calls per minute, capped at 1000
  golem-cli resource create api-calls \\
    --limit '{\"type\":\"rate\",\"value\":100,\"period\":\"minute\",\"max\":1000}'

  # A capacity-based quota with a custom unit label
  golem-cli resource create tokens \\
    --limit '{\"type\":\"capacity\",\"value\":500000}' --unit token --units tokens

  # A concurrency cap that rejects extra requests instead of throttling them
  golem-cli resource create concurrent-jobs \\
    --limit '{\"type\":\"concurrency\",\"value\":4}' --enforcement-action reject";

pub const RESOURCE_UPDATE: &str = "Examples:
  # Raise the limit on an existing rate quota
  golem-cli resource update api-calls --limit '{\"type\":\"rate\",\"value\":200,\"period\":\"minute\",\"max\":2000}'

  # Switch the enforcement action
  golem-cli resource update api-calls --enforcement-action terminate";

pub const RESOURCE_DELETE: &str = "Examples:
  # Delete a quota resource by name or by ID
  golem-cli resource delete api-calls
  golem-cli resource delete --id res-12345";

pub const RESOURCE_GET: &str = "Examples:
  # Get a quota resource by name
  golem-cli resource get api-calls

  # Or by ID
  golem-cli resource get --id res-12345";

pub const RESOURCE_LIST: &str = "Examples:
  # List quota resources in the current environment
  golem-cli resource list";
