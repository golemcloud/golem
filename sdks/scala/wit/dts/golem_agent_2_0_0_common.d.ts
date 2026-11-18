declare module 'golem:agent/common@2.0.0' {
  import * as golemCore200Http from 'golem:core/http@2.0.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as wasiClocks030Types from 'wasi:clocks/types@0.3.0';
  export type SchemaGraph = golemCore200Types.SchemaGraph;
  export type TypeNodeIndex = golemCore200Types.TypeNodeIndex;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type MetadataEnvelope = golemCore200Types.MetadataEnvelope;
  export type AgentId = golemCore200Types.AgentId;
  export type AccountId = golemCore200Types.AccountId;
  export type ComponentId = golemCore200Types.ComponentId;
  export type HttpRouteMode = golemCore200Http.HttpRouteMode;
  export type DurableStreamRouteOptions = golemCore200Http.DurableStreamRouteOptions;
  export type ConcreteHttpMethod = golemCore200Http.HttpMethod;
  export type NeutralPathSegment = golemCore200Http.PathSegment;
  export type HeaderVariable = golemCore200Http.HeaderVariable;
  export type QueryVariable = golemCore200Http.QueryVariable;
  export type FileMapping = golemCore200Http.FileMapping;
  export type FileResponseHeader = golemCore200Http.FileResponseHeader;
  export type Duration = wasiClocks030Types.Duration;
  export type AgentMode = "durable" | "ephemeral";
  export type AgentTypeKind = "regular" | "http-router";
  export type AutoInjectedKind = "principal";
  export type FieldSource =
  {
    tag: 'user-supplied'
  } |
  {
    tag: 'auto-injected'
    val: AutoInjectedKind
  };
  export type NamedField = {
    name: string;
    source: FieldSource;
    /** Index into the owning agent-type / dependency `schema` graph. */
    schema: TypeNodeIndex;
    metadata: MetadataEnvelope;
  };
  /**
   * Ordered, named input parameters with per-field source annotation.
   */
  export type InputSchema =
  {
    tag: 'parameters'
    val: NamedField[]
  };
  /**
   * Output is either unit (no value) or a single value of the given type.
   */
  export type OutputSchema =
  {
    tag: 'unit'
  } |
  /** Index into the owning agent-type / dependency `schema` graph. */
  {
    tag: 'single'
    val: TypeNodeIndex
  };
  export type CachePolicy =
  {
    tag: 'no-cache'
  } |
  {
    tag: 'until-write'
  } |
  {
    tag: 'ttl'
    val: Duration
  };
  export type ReadOnlyConfig = {
    cachePolicy: CachePolicy;
    usesPrincipal: boolean;
  };
  export type PhantomIdBindingDetails = {
    name: string;
    optional: boolean;
  };
  export type PhantomIdBinding =
  {
    tag: 'path'
    val: PhantomIdBindingDetails
  } |
  {
    tag: 'query'
    val: PhantomIdBindingDetails
  };
  export type CorsOptions = {
    allowedPatterns: string[];
  };
  export type HttpMethod =
  {
    tag: 'method'
    val: ConcreteHttpMethod
  } |
  {
    tag: 'any'
  };
  export type SystemVariable = "agent-type" | "agent-version";
  export type PathSegment =
  {
    tag: 'segment'
    val: NeutralPathSegment
  } |
  {
    tag: 'system-variable'
    val: SystemVariable
  };
  export type AuthDetails = {
    required: boolean;
  };
  export type HttpMountDetails = {
    pathPrefix: PathSegment[];
    authDetails?: AuthDetails;
    phantomAgent: boolean;
    phantomIdBinding?: PhantomIdBinding;
    corsOptions: CorsOptions;
    webhookSuffix: PathSegment[];
    staticBindings: FileMapping[];
    filesystemBindings: FileMapping[];
    /** Additional response headers for files served by static-bindings or filesystem-bindings. */
    fileResponseHeaders: FileResponseHeader[];
    /** The name of a parameterless router method that returns the OpenAPI document as a string. */
    openapiProviderMethod?: string;
  };
  export type HttpEndpointDetails = {
    httpMethod: HttpMethod;
    pathSuffix: PathSegment[];
    headerVars: HeaderVariable[];
    queryVars: QueryVariable[];
    authDetails?: AuthDetails;
    corsOptions: CorsOptions;
    mode?: HttpRouteMode;
    durableStreams?: DurableStreamRouteOptions;
  };
  export type AgentMethod = {
    name: string;
    description: string;
    httpEndpoint: HttpEndpointDetails[];
    promptHint?: string;
    inputSchema: InputSchema;
    outputSchema: OutputSchema;
    readOnly?: ReadOnlyConfig;
  };
  export type OidcPrincipal = {
    sub: string;
    issuer: string;
    email?: string;
    name?: string;
    emailVerified?: boolean;
    givenName?: string;
    familyName?: string;
    picture?: string;
    preferredUsername?: string;
    claims: string;
  };
  export type AgentPrincipal = {
    agentId: AgentId;
  };
  export type GolemUserPrincipal = {
    accountId: AccountId;
  };
  export type Principal =
  {
    tag: 'oidc'
    val: OidcPrincipal
  } |
  {
    tag: 'agent'
    val: AgentPrincipal
  } |
  {
    tag: 'golem-user'
    val: GolemUserPrincipal
  } |
  {
    tag: 'anonymous'
  };
  /**
   * The principals that caused an invocation.
   */
  export type Caller = {
    /** The principal at the root of the call tree. */
    original: Principal;
    /** The principal that made this call. */
    immediate: Principal;
  };
  export type AgentConstructor = {
    name?: string;
    description: string;
    promptHint?: string;
    inputSchema: InputSchema;
  };
  /**
   * Dependent agent type. All schema roots inside it resolve against its own
   * `schema`, not against the parent agent-type's graph.
   */
  export type AgentDependency = {
    typeName: string;
    description?: string;
    schema: SchemaGraph;
    constructor: AgentConstructor;
    methods: AgentMethod[];
  };
  export type SnapshottingConfig =
  {
    tag: 'default'
  } |
  /** current default in the server */
  {
    tag: 'periodic'
    val: Duration
  } |
  {
    tag: 'every-n-invocation'
    val: number
  };
  export type Snapshotting =
  {
    tag: 'disabled'
  } |
  {
    tag: 'enabled'
    val: SnapshottingConfig
  };
  export type AgentConfigSource = "local" | "secret";
  export type AgentConfigDeclaration = {
    source: AgentConfigSource;
    path: string[];
    /** Index into the owning agent-type `schema` graph. */
    valueType: TypeNodeIndex;
    /** Canonical schema JSON. None differs from the explicit default "null". */
    defaultValue?: string;
  };
  /**
   * Full agent type declaration.
   * `schema` is the per-agent type-node pool. Constructor / method / config
   * schema roots below are `type-node-index` values into `schema`. The
   * `schema.root` field is a structurally-required placeholder and is not the
   * semantic root of the agent type.
   */
  export type AgentType = {
    typeName: string;
    kind: AgentTypeKind;
    description: string;
    sourceLanguage: string;
    schema: SchemaGraph;
    constructor: AgentConstructor;
    methods: AgentMethod[];
    dependencies: AgentDependency[];
    mode: AgentMode;
    httpMount?: HttpMountDetails;
    snapshotting: Snapshotting;
    config: AgentConfigDeclaration[];
  };
  /**
   * Associates an agent type with a component that implements it
   */
  export type RegisteredAgentType = {
    agentType: AgentType;
    implementedBy: ComponentId;
  };
  export type TypedAgentConfigValue = {
    path: string[];
    value: TypedSchemaValue;
  };
  /**
   * Agent-level failures
   */
  export type AgentError =
  {
    tag: 'invalid-input'
    val: string
  } |
  {
    tag: 'invalid-method'
    val: string
  } |
  {
    tag: 'invalid-type'
    val: string
  } |
  {
    tag: 'invalid-agent-id'
    val: string
  } |
  {
    tag: 'custom-error'
    val: TypedSchemaValue
  };
}
