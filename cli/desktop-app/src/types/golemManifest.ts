export interface GolemApplicationManifest {
  includes?: string[];
  tempDir?: string;
  witDeps?: string[];
  templates?: Record<string, ComponentTemplate>;
  components?: Record<string, Component>;
  dependencies?: Record<string, ComponentDependency[]>;
  clean?: string[];
  customCommands?: Record<string, ExternalCommand[]>;
  httpApi?: HttpApi;
  profiles?: Record<string, Profile>;
}

export interface ComponentTemplate
  extends ComponentProperties,
    ComponentProfiles {}

export interface Component extends ComponentProperties, ComponentProfiles {
  template?: string;
}

export interface ComponentProperties {
  sourceWit?: string;
  generatedWit?: string;
  componentWasm?: string;
  linkedWasm?: string;
  build?: ExternalCommand[];
  customCommands?: Record<string, ExternalCommand[]>;
  clean?: string[];
  componentType?: "durable" | "ephemeral" | "library";
  files?: InitialComponentFile[];
  plugins?: PluginInstallation[];
  env?: Record<string, string>;
}

export interface ComponentProfiles {
  profiles?: Record<string, ComponentProperties>;
  defaultProfile?: string;
}

export interface ExternalCommand {
  command: string;
  dir?: string;
  rmdirs?: string[];
  mkdirs?: string[];
  sources?: string[];
  targets?: string[];
}

export type ComponentDependency =
  | WasmRpcDependency
  | WasmDependencyPath
  | WasmDependencyUrl;

export interface WasmRpcDependency {
  type: "wasm-rpc" | "wasm" | "wasm-rpc-static";
  target: string;
}

export interface WasmDependencyPath {
  type: "wasm";
  path: string;
}

export interface WasmDependencyUrl {
  type: "wasm";
  url: string;
}

export interface InitialComponentFile {
  sourcePath: string;
  targetPath: string;
  permissions?: "read-only" | "read-write";
}

export interface PluginInstallation {
  name: string;
  version: string;
  parameters?: Record<string, string>;
}

export interface HttpApi {
  definitions?: Record<string, HttpApiDefinition>;
  deployments?: Record<string, HttpApiDeployment[]>;
}

export interface HttpApiDefinition {
  version: string;
  project?: string;
  routes?: HttpApiDefinitionRoute[];

  // id and componentId not part of YAML
  id?: string;
  componentId?: string;
}

export function serializeHttpApiDefinition(
  definition: HttpApiDefinition,
): HttpApiDefinition {
  //   remove added attributes
  return {
    version: definition.version,
    project: definition.project,
    routes: definition.routes,
  };
}

export interface HttpApiDefinitionRoute {
  method:
    | "GET"
    | "CONNECT"
    | "POST"
    | "DELETE"
    | "PUT"
    | "PATCH"
    | "OPTIONS"
    | "TRACE"
    | "HEAD";
  path: string;
  security?: string;
  binding: HttpApiRouteBinding;
}

export interface HttpApiRouteBinding {
  type?: "default" | "cors-preflight" | "file-server" | "http-handler";
  componentName?: string;
  componentVersion?: number;
  idempotencyKey?: string;
  invocationContext?: string;
  response?: string;
}

export interface HttpApiDeployment {
  host: string;
  subdomain?: string;
  definition?: string[];
}

export interface Profile {
  default?: boolean;
  cloud?: boolean;
  project?: string;
  url?: string;
  workerUrl?: string;
  format?: "text" | "json" | "yaml";
  buildProfile?: string;
  autoConfirm?: boolean;
  redeployWorkers?: boolean;
  redeployHttpApi?: boolean;
  redeployAll?: boolean;
}
