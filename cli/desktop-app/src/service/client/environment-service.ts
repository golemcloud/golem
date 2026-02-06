import { settingsService } from "@/lib/settings";
import { readTextFile } from "@tauri-apps/plugin-fs";
import { parseDocument, Document, YAMLMap } from "yaml";
import { ManifestService } from "./manifest-service";
import { ManifestEnvironment, ManifestEnvironments } from "@/types/environment";

export class EnvironmentService {
  private manifestService: ManifestService;

  constructor(manifestService: ManifestService) {
    this.manifestService = manifestService;
  }

  /**
   * Get all environments from the manifest
   */
  public async getEnvironments(appId: string): Promise<ManifestEnvironments> {
    const manifest = await this.manifestService.getAppManifest(appId);
    return manifest.environments || {};
  }

  /**
   * Get a single environment by name
   */
  public async getEnvironment(
    appId: string,
    envName: string,
  ): Promise<ManifestEnvironment | undefined> {
    const environments = await this.getEnvironments(appId);
    return environments[envName];
  }

  /**
   * Create a new environment in the manifest
   */
  public async createEnvironment(
    appId: string,
    envName: string,
    environment: ManifestEnvironment,
  ): Promise<void> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    const yamlPath = await this.manifestService.getAppYamlPath(appId);
    if (!yamlPath) {
      throw new Error("App manifest file not found");
    }

    // Validate environment name (lowercase-kebab-case)
    if (!/^[a-z][a-z0-9-]*$/.test(envName)) {
      throw new Error(
        "Environment name must be lowercase-kebab-case (e.g., 'my-environment')",
      );
    }

    // Load the YAML into memory
    const rawYaml = await readTextFile(yamlPath);
    const manifest: Document = parseDocument(rawYaml);

    // Get or create environments section
    let environments = manifest.get("environments") as YAMLMap | undefined;
    if (!environments) {
      manifest.set("environments", new YAMLMap());
      environments = manifest.get("environments") as YAMLMap;
    }

    // Check if environment already exists
    if (environments.has(envName)) {
      throw new Error(`Environment '${envName}' already exists`);
    }

    // Add the new environment
    const envMap = this.environmentToYAMLMap(environment);
    environments.set(envName, envMap);

    // Save back to file
    await this.manifestService.saveAppManifest(appId, manifest.toString());
  }

  /**
   * Update an existing environment
   */
  public async updateEnvironment(
    appId: string,
    envName: string,
    environment: ManifestEnvironment,
  ): Promise<void> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    const yamlPath = await this.manifestService.getAppYamlPath(appId);
    if (!yamlPath) {
      throw new Error("App manifest file not found");
    }

    // Load the YAML into memory
    const rawYaml = await readTextFile(yamlPath);
    const manifest: Document = parseDocument(rawYaml);

    // Get environments section
    const environments = manifest.get("environments") as YAMLMap | undefined;
    if (!environments) {
      throw new Error("No environments section found in manifest");
    }

    // Check if environment exists
    if (!environments.has(envName)) {
      throw new Error(`Environment '${envName}' not found`);
    }

    // Update the environment
    const envMap = this.environmentToYAMLMap(environment);
    environments.set(envName, envMap);

    // Save back to file
    await this.manifestService.saveAppManifest(appId, manifest.toString());
  }

  /**
   * Delete an environment
   */
  public async deleteEnvironment(
    appId: string,
    envName: string,
  ): Promise<void> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    const yamlPath = await this.manifestService.getAppYamlPath(appId);
    if (!yamlPath) {
      throw new Error("App manifest file not found");
    }

    // Load the YAML into memory
    const rawYaml = await readTextFile(yamlPath);
    const manifest: Document = parseDocument(rawYaml);

    // Get environments section
    const environments = manifest.get("environments") as YAMLMap | undefined;
    if (!environments) {
      throw new Error("No environments section found in manifest");
    }

    // Check if environment exists
    if (!environments.has(envName)) {
      throw new Error(`Environment '${envName}' not found`);
    }

    // Check if it's the default environment
    const env = environments.get(envName) as YAMLMap;
    if (env && env.get("default") === true) {
      throw new Error("Cannot delete the default environment");
    }

    // Delete the environment
    environments.delete(envName);

    // Save back to file
    await this.manifestService.saveAppManifest(appId, manifest.toString());
  }

  /**
   * Set an environment as the default
   */
  public async setDefaultEnvironment(
    appId: string,
    envName: string,
  ): Promise<void> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    const yamlPath = await this.manifestService.getAppYamlPath(appId);
    if (!yamlPath) {
      throw new Error("App manifest file not found");
    }

    // Load the YAML into memory
    const rawYaml = await readTextFile(yamlPath);
    const manifest: Document = parseDocument(rawYaml);

    // Get environments section
    const environments = manifest.get("environments") as YAMLMap | undefined;
    if (!environments) {
      throw new Error("No environments section found in manifest");
    }

    // Check if environment exists
    if (!environments.has(envName)) {
      throw new Error(`Environment '${envName}' not found`);
    }

    // Remove default flag from all environments
    for (const pair of environments.items) {
      if (pair.value instanceof YAMLMap) {
        pair.value.delete("default");
      }
    }

    // Set new default
    const env = environments.get(envName) as YAMLMap;
    if (env) {
      env.set("default", true);
    }

    // Save back to file
    await this.manifestService.saveAppManifest(appId, manifest.toString());
  }

  /**
   * Get the default environment
   */
  public async getDefaultEnvironment(
    appId: string,
  ): Promise<{ name: string; environment: ManifestEnvironment } | undefined> {
    const environments = await this.getEnvironments(appId);
    for (const [name, env] of Object.entries(environments)) {
      if (env.default) {
        return { name, environment: env };
      }
    }
    return undefined;
  }

  /**
   * Migrate old profiles to new environments format
   */
  public async migrateProfilesToEnvironments(appId: string): Promise<void> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    const yamlPath = await this.manifestService.getAppYamlPath(appId);
    if (!yamlPath) {
      throw new Error("App manifest file not found");
    }

    // Load the YAML into memory
    const rawYaml = await readTextFile(yamlPath);
    const manifest: Document = parseDocument(rawYaml);

    // Get profiles section
    const profiles = manifest.get("profiles") as YAMLMap | undefined;
    if (!profiles || profiles.items.length === 0) {
      throw new Error("No profiles found to migrate");
    }

    // Check if environments already exist
    let environments = manifest.get("environments") as YAMLMap | undefined;
    if (environments && environments.items.length > 0) {
      throw new Error("Environments already exist. Manual migration required.");
    }

    // Create environments section
    if (!environments) {
      manifest.set("environments", new YAMLMap());
      environments = manifest.get("environments") as YAMLMap;
    }

    // Migrate each profile to environment
    for (const pair of profiles.items) {
      if (pair.value instanceof YAMLMap) {
        const migratedEnv = this.migrateProfileToEnvironment(pair.value);
        const envMap = this.environmentToYAMLMap(migratedEnv);
        environments.set(String(pair.key), envMap);
      }
    }

    // Optionally remove profiles section (commented out for safety)
    // manifest.delete("profiles");

    // Save back to file
    await this.manifestService.saveAppManifest(appId, manifest.toString());
  }

  /**
   * Convert a profile YAMLMap to environment structure
   */
  private migrateProfileToEnvironment(
    profileMap: YAMLMap,
  ): ManifestEnvironment {
    const env: ManifestEnvironment = {};

    // Map profile fields to environment fields
    if (profileMap.get("default")) {
      env.default = true;
    }

    if (profileMap.get("project")) {
      env.account = String(profileMap.get("project"));
    }

    // Map server configuration
    const cloud = profileMap.get("cloud");
    const url = profileMap.get("url");
    if (cloud) {
      env.server = { type: "builtin", value: "cloud" };
    } else if (url) {
      env.server = {
        type: "custom",
        value: {
          url: String(url),
          workerUrl: profileMap.get("agentUrl")
            ? String(profileMap.get("agentUrl"))
            : undefined,
          auth: { oauth2: true }, // Default to OAuth2 for custom servers
        },
      };
    } else {
      env.server = { type: "builtin", value: "local" };
    }

    // Map build profile to component presets
    if (profileMap.get("buildProfile")) {
      env.componentPresets = String(profileMap.get("buildProfile"));
    }

    // Map CLI options
    const cliOptions: Record<string, unknown> = {};
    if (profileMap.get("format")) {
      cliOptions.format = String(profileMap.get("format"));
    }
    if (profileMap.get("autoConfirm")) {
      cliOptions.autoConfirm = true;
    }
    if (profileMap.get("redeployAgents")) {
      cliOptions.redeployAgents = true;
    }
    if (Object.keys(cliOptions).length > 0) {
      env.cli = cliOptions;
    }

    // Map deployment options
    const deploymentOptions: Record<string, boolean> = {};
    if (profileMap.get("redeployAll")) {
      // This was a profile-specific option, map to reset
      deploymentOptions.securityOverrides = true;
    }
    if (Object.keys(deploymentOptions).length > 0) {
      env.deployment = deploymentOptions;
    }

    return env;
  }

  /**
   * Convert ManifestEnvironment to YAMLMap
   */
  private environmentToYAMLMap(env: ManifestEnvironment): YAMLMap {
    const map = new YAMLMap();

    if (env.default) {
      map.set("default", true);
    }

    if (env.account) {
      map.set("account", env.account);
    }

    if (env.server) {
      if (env.server.type === "builtin") {
        map.set("server", env.server.value);
      } else if (env.server.type === "custom") {
        const serverMap = new YAMLMap();
        serverMap.set("url", env.server.value.url);
        if (env.server.value.workerUrl) {
          serverMap.set("workerUrl", env.server.value.workerUrl);
        }
        if (env.server.value.allowInsecure) {
          serverMap.set("allowInsecure", true);
        }
        if ("oauth2" in env.server.value.auth) {
          const authMap = new YAMLMap();
          authMap.set("oauth2", true);
          serverMap.set("auth", authMap);
        } else if ("staticToken" in env.server.value.auth) {
          const authMap = new YAMLMap();
          authMap.set("staticToken", env.server.value.auth.staticToken);
          serverMap.set("auth", authMap);
        }
        map.set("server", serverMap);
      }
    }

    if (env.componentPresets) {
      map.set("componentPresets", env.componentPresets);
    }

    if (env.cli) {
      const cliMap = new YAMLMap();
      if (env.cli.format) {
        cliMap.set("format", env.cli.format);
      }
      if (env.cli.autoConfirm) {
        cliMap.set("autoConfirm", true);
      }
      if (env.cli.redeployAgents) {
        cliMap.set("redeployAgents", true);
      }
      if (env.cli.reset) {
        cliMap.set("reset", true);
      }
      if (cliMap.items.length > 0) {
        map.set("cli", cliMap);
      }
    }

    if (env.deployment) {
      const deploymentMap = new YAMLMap();
      if (env.deployment.compatibilityCheck !== undefined) {
        deploymentMap.set(
          "compatibilityCheck",
          env.deployment.compatibilityCheck,
        );
      }
      if (env.deployment.versionCheck !== undefined) {
        deploymentMap.set("versionCheck", env.deployment.versionCheck);
      }
      if (env.deployment.securityOverrides !== undefined) {
        deploymentMap.set(
          "securityOverrides",
          env.deployment.securityOverrides,
        );
      }
      if (deploymentMap.items.length > 0) {
        map.set("deployment", deploymentMap);
      }
    }

    return map;
  }
}
