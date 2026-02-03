// Environment type definitions matching Rust implementation
// Based on cli/golem-cli/src/model/environment.rs and golem-common/src/model/environment.rs

export interface EnvironmentName {
  name: string;
}

export interface DeploymentOptions {
  compatibilityCheck?: boolean;
  versionCheck?: boolean;
  securityOverrides?: boolean;
}

export interface CliOptions {
  format?: "text" | "json" | "yaml" | "pretty-json" | "pretty-yaml";
  autoConfirm?: boolean;
  redeployAgents?: boolean;
  reset?: boolean;
}

export type BuiltinServer = "local" | "cloud";

export interface CustomServer {
  url: string;
  workerUrl?: string;
  allowInsecure?: boolean;
  auth: CustomServerAuth;
}

export type CustomServerAuth = { oauth2: true } | { staticToken: string };

export type Server =
  | { type: "builtin"; value: BuiltinServer }
  | { type: "custom"; value: CustomServer };

// Manifest-level environment (what gets written to golem.yaml)
export interface ManifestEnvironment {
  default?: boolean;
  account?: string;
  server?: Server;
  componentPresets?: string | string[];
  cli?: CliOptions;
  deployment?: DeploymentOptions;
}

// Environments map in the manifest
export type ManifestEnvironments = Record<string, ManifestEnvironment>;

// CLI response types (from CLI commands)
export interface EnvironmentId {
  id: string;
}

export interface EnvironmentRevision {
  revision: number;
}

export interface EnvironmentCurrentDeploymentView {
  revision: {
    currentRevision: number;
  };
  deploymentRevision: {
    revision: number;
  };
  deploymentVersion: string;
  deploymentHash: string;
}

// Full environment from CLI (server-side representation)
export interface Environment {
  id: EnvironmentId;
  revision: EnvironmentRevision;
  applicationId: string;
  name: string;
  compatibilityCheck: boolean;
  versionCheck: boolean;
  securityOverrides: boolean;
  ownerAccountId: string;
  rolesFromActiveShares: string[];
  currentDeployment?: EnvironmentCurrentDeploymentView;
}

// Creation payload
export interface EnvironmentCreation {
  name: string;
  compatibilityCheck: boolean;
  versionCheck: boolean;
  securityOverrides: boolean;
}

// Update payload
export interface EnvironmentUpdate {
  currentRevision: number;
  name?: string;
  compatibilityCheck?: boolean;
  versionCheck?: boolean;
  securityOverrides?: boolean;
}

// UI-specific types for form handling
export interface EnvironmentFormData {
  name: string;
  isDefault: boolean;
  account?: string;
  serverType: "local" | "cloud" | "custom";
  customServerUrl?: string;
  customServerWorkerUrl?: string;
  customServerAllowInsecure?: boolean;
  customServerAuthType?: "oauth2" | "static";
  customServerStaticToken?: string;
  componentPresets: string[];
  cliFormat?: "text" | "json" | "yaml" | "pretty-json" | "pretty-yaml";
  cliAutoConfirm?: boolean;
  cliRedeployAgents?: boolean;
  cliReset?: boolean;
  deploymentCompatibilityCheck?: boolean;
  deploymentVersionCheck?: boolean;
  deploymentSecurityOverrides?: boolean;
}

// Helper function to convert form data to manifest environment
export function formDataToManifestEnvironment(
  data: EnvironmentFormData,
): ManifestEnvironment {
  const env: ManifestEnvironment = {};

  if (data.isDefault) {
    env.default = true;
  }

  if (data.account) {
    env.account = data.account;
  }

  // Server configuration
  if (data.serverType === "local") {
    env.server = { type: "builtin", value: "local" };
  } else if (data.serverType === "cloud") {
    env.server = { type: "builtin", value: "cloud" };
  } else if (data.serverType === "custom" && data.customServerUrl) {
    const customServer: CustomServer = {
      url: data.customServerUrl,
      workerUrl: data.customServerWorkerUrl,
      allowInsecure: data.customServerAllowInsecure,
      auth:
        data.customServerAuthType === "oauth2"
          ? { oauth2: true }
          : { staticToken: data.customServerStaticToken || "" },
    };
    env.server = { type: "custom", value: customServer };
  }

  // Component presets
  if (data.componentPresets.length > 0) {
    env.componentPresets =
      data.componentPresets.length === 1
        ? data.componentPresets[0]
        : data.componentPresets;
  }

  // CLI options
  const cliOptions: CliOptions = {};
  if (data.cliFormat) cliOptions.format = data.cliFormat;
  if (data.cliAutoConfirm) cliOptions.autoConfirm = true;
  if (data.cliRedeployAgents) cliOptions.redeployAgents = true;
  if (data.cliReset) cliOptions.reset = true;
  if (Object.keys(cliOptions).length > 0) {
    env.cli = cliOptions;
  }

  // Deployment options
  const deploymentOptions: DeploymentOptions = {};
  if (data.deploymentCompatibilityCheck !== undefined) {
    deploymentOptions.compatibilityCheck = data.deploymentCompatibilityCheck;
  }
  if (data.deploymentVersionCheck !== undefined) {
    deploymentOptions.versionCheck = data.deploymentVersionCheck;
  }
  if (data.deploymentSecurityOverrides !== undefined) {
    deploymentOptions.securityOverrides = data.deploymentSecurityOverrides;
  }
  if (Object.keys(deploymentOptions).length > 0) {
    env.deployment = deploymentOptions;
  }

  return env;
}

// Helper function to convert manifest environment to form data
export function manifestEnvironmentToFormData(
  name: string,
  env: ManifestEnvironment,
): EnvironmentFormData {
  const formData: EnvironmentFormData = {
    name,
    isDefault: env.default || false,
    account: env.account,
    serverType: "local", // default
    componentPresets: [],
  };

  // Parse server
  if (env.server) {
    if (env.server.type === "builtin") {
      formData.serverType = env.server.value;
    } else if (env.server.type === "custom") {
      formData.serverType = "custom";
      formData.customServerUrl = env.server.value.url;
      formData.customServerWorkerUrl = env.server.value.workerUrl;
      formData.customServerAllowInsecure = env.server.value.allowInsecure;
      if ("oauth2" in env.server.value.auth) {
        formData.customServerAuthType = "oauth2";
      } else if ("staticToken" in env.server.value.auth) {
        formData.customServerAuthType = "static";
        formData.customServerStaticToken = env.server.value.auth.staticToken;
      }
    }
  }

  // Parse component presets
  if (env.componentPresets) {
    formData.componentPresets =
      typeof env.componentPresets === "string"
        ? [env.componentPresets]
        : env.componentPresets;
  }

  // Parse CLI options
  if (env.cli) {
    formData.cliFormat = env.cli.format;
    formData.cliAutoConfirm = env.cli.autoConfirm;
    formData.cliRedeployAgents = env.cli.redeployAgents;
    formData.cliReset = env.cli.reset;
  }

  // Parse deployment options
  if (env.deployment) {
    formData.deploymentCompatibilityCheck = env.deployment.compatibilityCheck;
    formData.deploymentVersionCheck = env.deployment.versionCheck;
    formData.deploymentSecurityOverrides = env.deployment.securityOverrides;
  }

  return formData;
}
