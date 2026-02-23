import { CLIService } from "./cli-service";
import { Deployment } from "@/types/deployments";
import { ManifestService } from "./manifest-service";
import { settingsService } from "@/lib/settings.ts";
import { readTextFile } from "@tauri-apps/plugin-fs";
import { parseDocument, Document, YAMLMap, YAMLSeq } from "yaml";
import { Profile } from "@/types/profile";

export class DeploymentService {
  private cliService: CLIService;
  private manifestService: ManifestService;

  constructor(cliService: CLIService, manifestService: ManifestService) {
    this.cliService = cliService;
    this.manifestService = manifestService;
  }

  private async getActiveProfileName(appId: string): Promise<string> {
    try {
      const profile = (await this.cliService.callCLI(appId, "profile", [
        "get",
      ])) as Profile;
      return profile.name;
    } catch (error) {
      console.error(
        "Failed to get active profile, defaulting to 'local':",
        error,
      );
      return "local"; // Fallback to "local" if profile fetch fails
    }
  }

  public getDeploymentApi = async (appId: string): Promise<Deployment[]> => {
    await this.manifestService.migrateDeploymentSchema(appId);
    return (await this.cliService.callCLI(appId, "api", [
      "deployment",
      "list",
    ])) as Promise<Deployment[]>;
  };

  public deleteDeployment = async (appId: string, host: string) => {
    await this.manifestService.migrateDeploymentSchema(appId);
    // CLI v1.4.2: api deployment delete command removed
    // Only delete from YAML file
    // Server-side deletion is no longer supported via CLI
    await this.deleteDeploymentFromYaml(appId, host);
  };

  public createDeployment = async (
    appId: string,
    host: string,
    subdomain: string | null,
    definitions: { id: string; version: string }[],
  ) => {
    await this.manifestService.migrateDeploymentSchema(appId);

    // Step 1: Write to YAML first
    await this.writeDeploymentToYaml(appId, host, subdomain, definitions);

    // Step 2: Call CLI to deploy (use root "deploy" command to deploy both definitions and deployments)
    return await this.cliService.callCLI(appId, "deploy", []);
  };

  private writeDeploymentToYaml = async (
    appId: string,
    host: string,
    subdomain: string | null,
    definitions: { id: string; version: string }[],
  ) => {
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

    // Get or create httpApi section
    let httpApi = manifest.get("httpApi") as YAMLMap | undefined;
    if (!httpApi) {
      manifest.set("httpApi", new YAMLMap());
      httpApi = manifest.get("httpApi") as YAMLMap;
    }

    // Get or create deployments section
    let deployments = httpApi.get("deployments") as YAMLMap | undefined;
    if (!deployments) {
      httpApi.set("deployments", new YAMLMap());
      deployments = httpApi.get("deployments") as YAMLMap;
    }

    // Get current active profile from CLI
    const profileName = await this.getActiveProfileName(appId);

    // Get or create profile's deployment array
    let profileDeployments = deployments.get(profileName) as
      | YAMLSeq
      | undefined;
    if (!profileDeployments) {
      deployments.set(profileName, new YAMLSeq());
      profileDeployments = deployments.get(profileName) as YAMLSeq;
    }

    // Combine subdomain into domain (schema only allows `domain` and `definitions`)
    const fullDomain = subdomain ? `${subdomain}.${host}` : host;

    // Check if deployment with this domain already exists
    let existingDeploymentIndex = -1;
    let existingDefinitions: Set<string> = new Set();

    profileDeployments.items.forEach((item, index) => {
      const deploymentMap = item as YAMLMap;
      const existingDomain = deploymentMap.get("domain");

      if (existingDomain === fullDomain) {
        existingDeploymentIndex = index;
        // Get existing definitions
        const existingDefsSeq = deploymentMap.get("definitions") as YAMLSeq;
        if (existingDefsSeq && existingDefsSeq.items) {
          existingDefsSeq.items.forEach(item => {
            existingDefinitions.add(String(item));
          });
        }
      }
    });

    // Format definitions as "id@version"
    const formattedDefinitions = definitions.map(
      def => `${def.id}@${def.version}`,
    );

    // Merge with existing definitions (avoid duplicates)
    const mergedDefinitions = new Set([
      ...existingDefinitions,
      ...formattedDefinitions,
    ]);

    // Create new deployment object (only `domain` and `definitions` are valid)
    const newDeployment = new YAMLMap();
    newDeployment.set("domain", fullDomain);
    const definitionsSeq = new YAMLSeq();
    mergedDefinitions.forEach(def => definitionsSeq.add(def));
    newDeployment.set("definitions", definitionsSeq);

    if (existingDeploymentIndex >= 0) {
      // Update existing deployment with merged definitions
      profileDeployments.set(existingDeploymentIndex, newDeployment);
    } else {
      // Add new deployment
      profileDeployments.add(newDeployment);
    }

    // Save back to file
    await this.manifestService.saveAppManifest(appId, manifest.toString());
  };

  private deleteDeploymentFromYaml = async (appId: string, host: string) => {
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

    // Get httpApi section
    const httpApi = manifest.get("httpApi") as YAMLMap | undefined;
    if (!httpApi) {
      return; // Nothing to delete
    }

    // Get deployments section
    const deployments = httpApi.get("deployments") as YAMLMap | undefined;
    if (!deployments) {
      return; // Nothing to delete
    }

    // Get current active profile from CLI
    const profileName = await this.getActiveProfileName(appId);

    // Get profile's deployment array
    const profileDeployments = deployments.get(profileName) as
      | YAMLSeq
      | undefined;
    if (!profileDeployments) {
      return; // Nothing to delete
    }

    // Find and remove deployment matching the host
    let modified = false;
    for (let i = profileDeployments.items.length - 1; i >= 0; i--) {
      const deploymentMap = profileDeployments.items[i] as YAMLMap;
      const deploymentHost = deploymentMap.get("domain");

      if (deploymentHost === host) {
        profileDeployments.items.splice(i, 1);
        modified = true;
        break; // Only one deployment per host
      }
    }

    // Save if we made any changes
    if (modified) {
      await this.manifestService.saveAppManifest(appId, manifest.toString());
    }
  };
}
