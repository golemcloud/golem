declare module 'golem:agent/common@1.5.0' {
  import * as golemCore150Types from 'golem:core/types@1.5.0';
  import * as wasiClocks023MonotonicClock from 'wasi:clocks/monotonic-clock@0.2.3';
  export type ValueAndType = golemCore150Types.ValueAndType;
  export type WitType = golemCore150Types.WitType;
  export type WitValue = golemCore150Types.WitValue;
  export type AgentId = golemCore150Types.AgentId;
  export type AccountId = golemCore150Types.AccountId;
  export type ComponentId = golemCore150Types.ComponentId;
  export type TextType = golemCore150Types.TextType;
  export type TextReference = golemCore150Types.TextReference;
  export type BinaryType = golemCore150Types.BinaryType;
  export type BinaryReference = golemCore150Types.BinaryReference;
  export type Duration = wasiClocks023MonotonicClock.Duration;
  export type AgentMode = "durable" | "ephemeral";
  export type CorsOptions = {
    allowedPatterns: string[];
  };
  export type HttpMethod = 
  {
    tag: 'get'
  } |
  {
    tag: 'head'
  } |
  {
    tag: 'post'
  } |
  {
    tag: 'put'
  } |
  {
    tag: 'delete'
  } |
  {
    tag: 'connect'
  } |
  {
    tag: 'options'
  } |
  {
    tag: 'trace'
  } |
  {
    tag: 'patch'
  } |
  {
    tag: 'custom'
    val: string
  };
  export type SystemVariable = "agent-type" | "agent-version";
  export type PathVariable = {
    variableName: string;
  };
  export type PathSegment = 
  {
    tag: 'literal'
    val: string
  } |
  {
    tag: 'system-variable'
    val: SystemVariable
  } |
  {
    tag: 'path-variable'
    val: PathVariable
  } |
  /** only allowed as the last segment */
  {
    tag: 'remaining-path-variable'
    val: PathVariable
  };
  export type HeaderVariable = {
    headerName: string;
    variableName: string;
  };
  export type QueryVariable = {
    queryParamName: string;
    variableName: string;
  };
  export type AuthDetails = {
    required: boolean;
  };
  export type HttpMountDetails = {
    pathPrefix: PathSegment[];
    authDetails?: AuthDetails;
    phantomAgent: boolean;
    corsOptions: CorsOptions;
    webhookSuffix: PathSegment[];
  };
  export type HttpEndpointDetails = {
    httpMethod: HttpMethod;
    pathSuffix: PathSegment[];
    headerVars: HeaderVariable[];
    queryVars: QueryVariable[];
    authDetails?: AuthDetails;
    corsOptions: CorsOptions;
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
  export type ElementValue = 
  {
    tag: 'component-model'
    val: WitValue
  } |
  {
    tag: 'unstructured-text'
    val: TextReference
  } |
  {
    tag: 'unstructured-binary'
    val: BinaryReference
  };
  export type DataValue = 
  /** List of element values, each corresponding to an element of the tuple `data-schema` */
  {
    tag: 'tuple'
    val: ElementValue[]
  } |
  /**
   * List of element values and their schema names; each name points to one named element of the corresponding
   * multimodal `data-schema`.
   */
  {
    tag: 'multimodal'
    val: [string, ElementValue][]
  };
  export type TextDescriptor = {
    restrictions?: TextType[];
  };
  export type BinaryDescriptor = {
    restrictions?: BinaryType[];
  };
  export type ElementSchema = 
  {
    tag: 'component-model'
    val: WitType
  } |
  {
    tag: 'unstructured-text'
    val: TextDescriptor
  } |
  {
    tag: 'unstructured-binary'
    val: BinaryDescriptor
  };
  export type DataSchema = 
  /** List of named elements */
  {
    tag: 'tuple'
    val: [string, ElementSchema][]
  } |
  /** List of named variants that can be used 0 or more times in a multimodal `data-value` */
  {
    tag: 'multimodal'
    val: [string, ElementSchema][]
  };
  export type AgentMethod = {
    name: string;
    description: string;
    httpEndpoint: HttpEndpointDetails[];
    promptHint?: string;
    inputSchema: DataSchema;
    outputSchema: DataSchema;
  };
  export type AgentConstructor = {
    name?: string;
    description: string;
    promptHint?: string;
    inputSchema: DataSchema;
  };
  export type AgentDependency = {
    typeName: string;
    description?: string;
    constructor: AgentConstructor;
    methods: AgentMethod[];
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
    val: ValueAndType
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
  export type AgentType = {
    typeName: string;
    description: string;
    constructor: AgentConstructor;
    methods: AgentMethod[];
    dependencies: AgentDependency[];
    mode: AgentMode;
    httpMount?: HttpMountDetails;
    snapshotting: Snapshotting;
  };
  /**
   * Associates an agent type with a component that implements it
   */
  export type RegisteredAgentType = {
    agentType: AgentType;
    implementedBy: ComponentId;
  };
}
