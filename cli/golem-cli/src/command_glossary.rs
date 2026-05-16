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

//! Single source of truth for help text describing cross-cutting concepts that
//! appear on many CLI commands (agent IDs, function arguments, ...). The short
//! variants are surfaced via clap's `help = ...` (used by `-h`) and the long
//! variants via `long_help = ...` (used by `--help`).

// ── <AGENT_ID> ────────────────────────────────────────────────────────────────

pub const AGENT_ID_SHORT: &str = "Agent ID, accepted formats:
  <AGENT_TYPE>(<AGENT_PARAMETERS>)
  <ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
  <APPLICATION>/<ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
  <ACCOUNT>/<APPLICATION>/<ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)";

pub const AGENT_ID_LONG: &str = "Agent ID — fully-qualified address of an agent instance.

Forms (most → least qualified). Missing prefix segments default to the value
selected by `-A`/`-E`/`--profile` or by the current working directory:

  <AGENT_TYPE>(<AGENT_PARAMETERS>)
  <ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
  <APPLICATION>/<ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)
  <ACCOUNT>/<APPLICATION>/<ENVIRONMENT>/<AGENT_TYPE>(<AGENT_PARAMETERS>)

Components:
  <AGENT_TYPE>        Name of a deployed agent type, in the agent's source-language
                      casing (e.g. `CounterAgent`). Discover with:
                        golem-cli agent-type list
                        golem-cli agent-type get <AGENT_TYPE>
  <AGENT_PARAMETERS>  Constructor arguments for the agent. The parentheses are
                      MANDATORY even when the agent takes no parameters
                      (e.g. `MyAgent()`). See \"Parameter syntax\" below.
  <ENVIRONMENT>       Manifest environment name. Defaults to the active
                      environment selected by `-E`/`-L`/`-C`. List with:
                        golem-cli environment list
  <APPLICATION>       Application name from `golem.yaml`. Defaults to the
                      application selected by cwd / `-A`.
  <ACCOUNT>           Account ID. Only valid in multi-account / cloud setups;
                      defaults to the active profile's account.

Parameter syntax:
  Use the agent component's source-language literal syntax. Discover the
  language with:
    golem-cli agent-type get <AGENT_TYPE>     (or `golem-cli component get`)

  Per-language examples for the same agent type `MyAgent(name: string, age: u32)`:
    Rust:        MyAgent(\"alice\", 42)
    TypeScript:  MyAgent(\"alice\", 42)
    Scala:       MyAgent(\"alice\", 42)
    MoonBit:     MyAgent(\"alice\", 42)

  When in doubt, use the canonical structural form — it is positional (no
  field names) and accepted regardless of the component's source language:
    Structural:  MyAgent(\"alice\",42)

  Structural quick reference:
    String       \"hello\"                    Char        c\"a\"
    Tuple        (a,b,c)                    List        [a,b,c]
    Record       (val1,val2,val3)           Option      s(x) | n
    Variant      v1 | v1(payload)           Result      ok(x) | err(x)
    Float        1.5  (decimal point required)

Shell quoting:
  Wrap the whole AGENT_ID in single quotes so the parens and any inner
  double quotes survive shell processing:
    golem-cli agent get 'MyAgent(\"alice\")'

Examples:
  golem-cli agent get 'CounterAgent(\"c1\")'
  golem-cli agent get 'staging/CounterAgent(\"c1\")'
  golem-cli agent get 'my-app/staging/CounterAgent(\"c1\")'
  golem-cli agent get 'acc-123/my-app/staging/CounterAgent(\"c1\")'";

// ── [ARGUMENTS]... on `agent invoke` ─────────────────────────────────────────

pub const INVOKE_ARGS_SHORT: &str = "Agent function arguments specified using the agent component's language syntax \
     (or the canonical structural form). One shell positional per function argument.";

pub const INVOKE_ARGS_LONG: &str =
    "Agent function arguments. Pass ONE shell positional per function
argument — do NOT join them with commas.

Each argument uses the agent component's source-language literal syntax (the
same syntax used for <AGENT_PARAMETERS>). Discover the component's language
with `golem-cli agent-type get <AGENT_TYPE>` (or `golem-cli component get`).

Per-language examples (agent function `update_profile(name: string, age: u32, cfg: MyConfig)`):
  Rust:        '\"alice\"' 42 'MyConfig { region: \"us-east\" }'
  TypeScript:  '\"alice\"' 42 '{ region: \"us-east\" }'
  Scala:       '\"alice\"' 42 'MyConfig(region = \"us-east\")'
  MoonBit:     '\"alice\"' 42 'MyConfig::{ region: \"us-east\" }'

When in doubt, use the canonical structural form — it is positional (no field
names) and accepted regardless of the component's source language:
  Structural:  '\"alice\"' 42 '(\"us-east\")'

Structural quick reference (same syntax as for <AGENT_PARAMETERS>):
  String   \"hello\"           Char     c\"a\"
  Tuple    (a,b,c)            List     [a,b,c]
  Record   (val1,val2,val3)   Option   s(x) | n
  Variant  v1 | v1(payload)   Result   ok(x) | err(x)
  Float    1.5  (decimal point required)

Shell quoting:
  Wrap each string-typed argument in single quotes so inner double quotes
  survive shell processing:
    golem-cli agent invoke 'MyAgent()' set_name '\"alice\"'";

// ── Group-level summary for `golem-cli agent --help` ─────────────────────────

pub const AGENT_GROUP_AFTER: &str =
    "For the AGENT_ID grammar (forms, parameter syntax, shell quoting,
per-language examples, and the universal structural fallback), run any leaf
command with --help, e.g.:
  golem-cli agent get --help

For the function argument grammar on `agent invoke`, see:
  golem-cli agent invoke --help";

// ── Retry policy: --predicate ────────────────────────────────────────────────

pub const RETRY_PREDICATE_SHORT: &str = "Retry predicate as JSON or YAML (single-key object per node, camelCase keys). \
     See --help for the full grammar.";

pub const RETRY_PREDICATE_LONG: &str = "Retry predicate as JSON or YAML.

A predicate is a boolean expression evaluated against the retry context to
decide whether the policy applies to a given error. It is encoded as a
single-key object whose key is the node kind. Keys are camelCase.

Comparison nodes (compare a context property to a typed value):
  { \"propEq\":         { \"property\": \"error.kind\",    \"value\": \"Timeout\" } }
  { \"propNeq\":        { \"property\": \"attempt\",       \"value\": 0 } }
  { \"propGt\"  | \"propGte\" | \"propLt\" | \"propLte\":
                      { \"property\": \"elapsedMs\",     \"value\": 1000 } }
  { \"propIn\":         { \"property\": \"error.kind\",    \"values\": [\"Timeout\",\"Throttled\"] } }
  { \"propMatches\":    { \"property\": \"error.message\", \"pattern\": \"rate.*limit*\" } }
  { \"propStartsWith\": { \"property\": \"error.kind\",    \"prefix\":  \"Net\" } }
  { \"propContains\":   { \"property\": \"error.message\", \"substring\": \"deadlock\" } }
  { \"propExists\":     \"trace.tag\" }

Combinators:
  { \"and\": [<predicate>, <predicate>] }
  { \"or\":  [<predicate>, <predicate>] }
  { \"not\":  <predicate> }

Constants:
  true   false

Property values are typed: strings, signed 64-bit integers, or booleans.
The set of available context properties is provided by the worker executor
and depends on the failing operation; commonly available are:
  error.kind        string
  error.message     string
  attempt           integer
  elapsedMs         integer

Examples:

  YAML, retry only on Timeout for the first 5 attempts:
    --predicate '
      and:
        - propEq:  { property: error.kind, value: Timeout }
        - propLte: { property: attempt,    value: 5 }
    '

  JSON, retry on any Net* error except NetUnreachable:
    --predicate '{
      \"and\": [
        { \"propStartsWith\": { \"property\": \"error.kind\", \"prefix\": \"Net\" } },
        { \"not\": { \"propEq\": { \"property\": \"error.kind\", \"value\": \"NetUnreachable\" } } }
      ]
    }'

  Always retry:
    --predicate true";

// ── Retry policy: --policy ───────────────────────────────────────────────────

pub const RETRY_POLICY_SHORT: &str = "Retry policy as JSON or YAML (single-key object per node, camelCase keys). \
     See --help for the full grammar.";

pub const RETRY_POLICY_LONG: &str = "Retry policy as JSON or YAML.

A policy is a composable strategy describing how (and how long) to retry. It
is encoded as a single-key object whose key is the policy kind. Keys are
camelCase. Durations accept either a humantime string (preferred):
  \"200ms\", \"5s\", \"1m\", \"500us\"
or the Rust std::time::Duration struct form:
  { \"secs\": <u64>, \"nanos\": <u32> }

Leaf strategies:
  \"immediate\"                              # retry with zero delay
  \"never\"                                  # give up on first failure
  { \"periodic\":    \"1s\" }
  { \"exponential\": { \"baseDelay\": \"1s\", \"factor\": 2.0 } }
  { \"fibonacci\":   { \"first\":     \"1s\",
                     \"second\":    \"1s\" } }

Bounding wrappers (limit how long retries continue):
  { \"countBox\": { \"maxRetries\": 5, \"inner\": <policy> } }
  { \"timeBox\":  { \"limit\": \"30s\", \"inner\": <policy> } }

Delay-shaping wrappers:
  { \"clamp\":    { \"minDelay\": <duration>, \"maxDelay\": <duration>, \"inner\": <policy> } }
  { \"addDelay\": { \"delay\":    <duration>,                          \"inner\": <policy> } }
  { \"jitter\":   { \"factor\":   0.1,                                 \"inner\": <policy> } }

Conditional wrapper (see --predicate for the predicate grammar):
  { \"filteredOn\": { \"predicate\": <predicate>, \"inner\": <policy> } }

Combinators:
  { \"andThen\":   [<policy>, <policy>] }   # run first until it gives up, then second
  { \"union\":     [<policy>, <policy>] }   # retry while EITHER would still retry
  { \"intersect\": [<policy>, <policy>] }   # retry while BOTH would still retry

Examples:

  YAML, exponential backoff capped at 30s, max 10 attempts:
    --policy '
      countBox:
        maxRetries: 10
        inner:
          clamp:
            minDelay: 0s
            maxDelay: 30s
            inner:
              exponential:
                baseDelay: 1s
                factor: 2.0
    '

  JSON, give up after 1 minute or 5 attempts, whichever comes first:
    --policy '{
      \"intersect\": [
        { \"timeBox\":  { \"limit\":      \"60s\", \"inner\": \"immediate\" } },
        { \"countBox\": { \"maxRetries\": 5,      \"inner\": \"immediate\" } }
      ]
    }'

  Periodic 1s retry only for Timeout errors:
    --policy '{
      \"filteredOn\": {
        \"predicate\": { \"propEq\": { \"property\": \"error.kind\", \"value\": \"Timeout\" } },
        \"inner\": { \"periodic\": \"1s\" }
      }
    }'";

// ── Resource quota: --limit ──────────────────────────────────────────────────

pub const RESOURCE_LIMIT_SHORT: &str = "Resource limit as JSON. Internally tagged by `type` (Rate | Capacity | Concurrency). \
     See --help for the full grammar.";

pub const RESOURCE_LIMIT_LONG: &str = "Resource limit as JSON.

The value is a JSON object internally tagged by `type` with one of three
shapes:

  Rate (a budget refilled every period, optional burst capacity):
    { \"type\": \"Rate\",
      \"value\":  <u64>,            // tokens granted per period
      \"period\": <time-period>,    // refill window (see below)
      \"max\":    <u64> }           // maximum burst capacity

  Capacity (a fixed total, decremented as it is consumed):
    { \"type\": \"Capacity\",
      \"value\": <u64> }

  Concurrency (max in-flight uses at any given moment):
    { \"type\": \"Concurrency\",
      \"value\": <u64> }

`period` (kebab-case enum value):
  second | minute | hour | day | month | year

Examples:

  100 tokens per minute, allowing bursts up to 500:
    --limit '{ \"type\": \"Rate\", \"value\": 100, \"period\": \"minute\", \"max\": 500 }'

  Total cap of 10,000,000 units (consumed over the quota's lifetime):
    --limit '{ \"type\": \"Capacity\", \"value\": 10000000 }'

  At most 8 concurrent uses:
    --limit '{ \"type\": \"Concurrency\", \"value\": 8 }'";

// ── Plugin manifest (`golem-cli plugin register <MANIFEST>`) ─────────────────

pub const PLUGIN_MANIFEST_SHORT: &str = "Path to the plugin manifest JSON or YAML, or '-' for STDIN. \
     See --help for the manifest grammar.";

pub const PLUGIN_MANIFEST_LONG: &str = "Path to the plugin manifest, or '-' to read from STDIN.

The manifest is JSON or YAML and uses camelCase keys. The `specs` field is
internally tagged by `type` and currently supports `OplogProcessor`. The
`icon` field is a path to an image file resolved relative to the manifest's
location and is uploaded as part of registration.

Top-level shape:
  name:        string             # plugin name
  version:     string             # plugin version
  description: string             # human-readable description
  icon:        path               # path to the plugin icon image file
  homepage:    string             # plugin homepage URL
  specs:       <plugin-spec>      # see below

`specs` shapes:
  OplogProcessor:
    { \"type\":             \"OplogProcessor\",
      \"componentId\":      \"<uuid>\",
      \"componentRevision\": <integer> }

Example (YAML):
    name: my-oplog-processor
    version: 0.1.0
    description: Processes oplog entries for auditing
    icon: ./icon.png
    homepage: https://example.com/my-plugin
    specs:
      type: OplogProcessor
      componentId: 5a3a1f6c-3a3f-4b1e-9ad0-7c8c9a2c1234
      componentRevision: 1

Register from STDIN:
  cat my-plugin.yaml | golem-cli plugin register -";

// ── server run / start: --ports-file ─────────────────────────────────────────

pub const PORTS_FILE_SHORT: &str =
    "Write discovered startup ports to this JSON file (overwritten atomically).";

pub const PORTS_FILE_LONG: &str = "Write discovered startup ports to this JSON file.

The file is written atomically once the local server has finished binding
all of its ports. The schema is a single object with camelCase fields:

  {
    \"routerPort\":        <u16>,    // main API port (default 9881)
    \"customRequestPort\": <u16>,    // custom HTTP API port (default 9006)
    \"mcpPort\":           <u16>     // MCP server port (default 9007)
  }

Useful when launching the local server with `--router-port 0` (or any
explicit 0) to let the OS pick a free port: read the JSON to discover which
port was actually bound.";

// ── api security-scheme --scope ──────────────────────────────────────────────

pub const SECURITY_SCHEME_SCOPE_SHORT: &str =
    "OAuth2/OIDC scope (provider-specific). Pass --scope multiple times for multiple scopes.";

pub const SECURITY_SCHEME_SCOPE_LONG: &str =
    "OAuth2 / OIDC scope requested from the identity provider.

Scopes are free-form strings whose meaning is defined by the chosen
provider, not by Golem. Pass `--scope` once per scope. The exact set of
valid values comes from the provider's documentation.

Common per-provider examples:
  Google:    --scope openid --scope email --scope profile
  Microsoft: --scope openid --scope profile --scope User.Read
  Gitlab:    --scope openid --scope email --scope read_user
  Facebook:  --scope email --scope public_profile
  Custom:    any scope advertised by the configured OIDC issuer

For full lists see each provider's docs, e.g.:
  https://developers.google.com/identity/protocols/oauth2/scopes
  https://learn.microsoft.com/azure/active-directory/develop/scopes-oidc
  https://docs.gitlab.com/ee/integration/oauth_provider.html#authorized-applications
  https://developers.facebook.com/docs/permissions";

// ── agent oplog --query ──────────────────────────────────────────────────────

pub const OPLOG_QUERY_SHORT: &str = "Lucene-style query against oplog entries (case-insensitive terms, AND/OR/NOT, phrases, regex). \
     Mutually exclusive with --from. See --help for matchable terms.";

pub const OPLOG_QUERY_LONG: &str = "Lucene-style query against the agent's oplog.

Supported syntax:
  term            case-insensitive substring match (e.g. error)
  \"phrase\"        case-sensitive contains match (e.g. \"connection refused\")
  /pattern/       regex match (Rust regex syntax)
  a AND b         conjunction (also: a b)
  a OR b          disjunction
  NOT a           negation
  field:term      restrict the term to a specific field path

What is matched:
  Each oplog entry exposes a small set of strings to the matcher: the entry
  kind name (and aliases) plus a few inner strings. Field-qualified queries
  only match when the entry path matches the qualifier; for most ad-hoc
  searches a bare term is what you want.

Common terms (case-insensitive, match the entry kind):
  create, host-call, imported-function, agent-invocation-started,
  agent-invocation-finished, pending-agent-invocation, agent-initialization,
  agent-method-invocation, save-snapshot, load-snapshot, manual-update,
  process-oplog-entries, suspend, error, noop, jump, interrupted, exited,
  begin-atomic-region, end-atomic-region, begin-remote-write, end-remote-write,
  invoke (alias matching all invocation entries)

Inner strings additionally matched (depending on entry kind):
  function/method names, idempotency keys, error messages, host-call request
  and response values.

Examples:
  --query 'error'
  --query 'invoke AND set_name'
  --query 'error AND NOT \"connection refused\"'
  --query '/timeout|throttle/'";

pub const CONCEPTS: &str = "\
Concepts:

  Profile vs Environment
    A \"profile\" (`--profile`, `golem-cli profile`) is a CLI-side identity:
    server URL(s), credentials and default output format. Profiles are
    stored under the config directory (see `--config-dir`, default
    $HOME/.golem) and are independent of any application.

    An \"environment\" (`--environment`/`-E`) is a deployment target defined
    inside the application manifest (`golem.yaml`). It selects the
    namespace into which agents, components, secrets, retry policies,
    resource definitions and APIs are deployed.

    A typical setup uses one profile per Golem cluster (e.g. `local`,
    `cloud`) and one environment per stage (e.g. `dev`, `staging`,
    `prod`). The two are orthogonal.

  -L / -C shortcuts
    `-L`/`--local` selects the `local` environment from the manifest if
    one exists, otherwise falls back to the `local` profile.
    `-C`/`--cloud` does the same for `cloud`. They are mutually exclusive
    with each other and convenient defaults for the two most common
    setups. Use `--profile` and `--environment` explicitly when you need
    something else.

  Application manifest discovery
    Most commands need an application manifest (`golem.yaml`). Unless
    `-X`/`--disable-app-manifest-discovery` is set, golem-cli walks from
    the current working directory upward through every parent directory
    and uses the OUTERMOST `golem.yaml` it finds as the application root.
    Sub-manifests referenced from that root are then loaded relative to
    it.

    Override the discovered path with `-A <PATH>`/
    `--app-manifest-path <PATH>` (the path must point at the root
    `golem.yaml`).

    Commands that do not require an application (e.g. `profile`,
    `account`, `server`, `completion`) ignore manifest discovery.

  Non-interactive use (for agents and CI)
    Pass `-Y`/`--yes` to auto-confirm destructive prompts and
    `-F json`/`--format json` (or `pretty-json`, `yaml`, `pretty-yaml`)
    to get machine-readable output. Both are global flags.";
