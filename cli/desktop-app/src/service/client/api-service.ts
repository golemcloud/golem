import {
  HttpApiDefinition,
  serializeHttpApiDefinition,
} from "@/types/golemManifest.ts";
import { settingsService } from "@/lib/settings.ts";
import { readTextFile } from "@tauri-apps/plugin-fs";
import { join } from "@tauri-apps/api/path";
import { parseDocument, Document, YAMLMap } from "yaml";
import { CLIService } from "./cli-service";
import { ComponentService } from "./component-service";
import { ManifestService } from "./manifest-service";

export class APIService {
  private cliService: CLIService;
  private componentService: ComponentService;
  private manifestService: ManifestService;

  constructor(
    cliService: CLIService,
    componentService: ComponentService,
    manifestService: ManifestService,
  ) {
    this.cliService = cliService;
    this.componentService = componentService;
    this.manifestService = manifestService;
  }

  public getApiList = async (appId: string): Promise<HttpApiDefinition[]> => {
    let result: HttpApiDefinition[] = [];
    // we get it on a per-component basis
    let components = await this.componentService.getComponents(appId);
    for (const component of components) {
      try {
        let manifest = await this.manifestService.getComponentManifest(
          appId,
          component.componentId!,
        );
        let APIList = manifest.httpApi;
        if (APIList && APIList.definitions) {
          for (const apiListKey in APIList.definitions) {
            let data = APIList.definitions[apiListKey];
            if (data) {
              data.id = apiListKey;
              data.componentId = component.componentId;
              result.push(data);
            }
          }
        }
      } catch (e) {
        console.error(e, component.componentName);
      }
    }
    // find in app's golem.yaml
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }
    const manifest = await this.manifestService.getAppManifest(appId);
    let APIList = manifest.httpApi;
    if (APIList && APIList.definitions) {
      for (const apiListKey in APIList.definitions) {
        let data = APIList.definitions[apiListKey];
        if (data) {
          data.id = apiListKey;
          data.componentId = undefined; // This is not from a component
          result.push(data);
        }
      }
    }

    return result;
  };

  public getUploadedDefinitions = async (
    appId: string,
  ): Promise<Array<{ id: string; version: string }>> => {
    return (await this.cliService.callCLI(appId, "api", [
      "definition",
      "list",
    ])) as Promise<Array<{ id: string; version: string }>>;
  };

  public deployDefinition = async (appId: string, _definitionId: string) => {
    await this.manifestService.migrateDeploymentSchema(appId);
    // CLI v1.4.2: api definition deploy removed, use root deploy command instead
    // This deploys the entire application including all API definitions
    return await this.cliService.callCLI(appId, "deploy", []);
  };

  public getApi = async (
    appId: string,
    name: string,
  ): Promise<HttpApiDefinition[]> => {
    const ApiList = await this.getApiList(appId);
    const Api = ApiList.filter(a => a.id == name);
    if (!Api) {
      throw new Error("Api not found");
    }
    return Api;
  };

  public createApi = async (appId: string, payload: HttpApiDefinition) => {
    // should use the app's YAML file
    const path = await this.manifestService.getAppYamlPath(appId);
    if (!path) {
      throw new Error("App manifest file not found");
    }
    // Load the YAML into memory, update and save
    const rawYaml = await readTextFile(path);
    // Parse as Document to preserve comments and formatting
    const manifest: Document = parseDocument(rawYaml);
    let httpApi = manifest.get("httpApi") as YAMLMap | undefined;
    if (!httpApi) {
      // Create new httpApi section if it doesn't exist
      manifest.set("httpApi", new YAMLMap());
      httpApi = manifest.get("httpApi") as YAMLMap;
    }
    // set the definition with the key
    let definitions = httpApi.get("definitions") as YAMLMap | undefined;
    if (!definitions) {
      // Create new definitions section if it doesn't exist
      httpApi.set("definitions", new YAMLMap());
      definitions = httpApi.get("definitions") as YAMLMap;
    }
    // Add or update the API definition
    payload.version = payload.version || "0.1.0"; // Ensure version is set
    definitions.set(payload.id, serializeHttpApiDefinition(payload));
    // Save config back
    await this.manifestService.saveAppManifest(appId, manifest.toString());
  };

  public deleteApi = async (appId: string, id: string, version: string) => {
    // Step 1: Find the API definition to determine where it's stored (app or component)
    const apiList = await this.getApiList(appId);
    const apiToDelete = apiList.find(
      api => api.id === id && api.version === version,
    );

    if (!apiToDelete) {
      throw new Error(`API definition ${id}@${version} not found`);
    }

    // CLI v1.4.2: api definition delete command removed
    // Only delete from YAML file (both definition and deployments)
    // Server-side deletion is no longer supported via CLI
    await this.deleteApiFromYaml(appId, id, version, apiToDelete.componentId);
  };

  private deleteApiFromYaml = async (
    appId: string,
    apiId: string,
    version: string,
    componentId?: string,
  ) => {
    // Step 1: Delete the definition from its source (component or app YAML)
    let definitionYamlPath = "";

    if (componentId) {
      const component = await this.componentService.getComponentById(
        appId,
        componentId,
      );
      definitionYamlPath = await this.manifestService.getComponentYamlPath(
        appId,
        component.componentName!,
      );
    } else {
      definitionYamlPath = (await this.manifestService.getAppYamlPath(
        appId,
      )) as string;
      if (!definitionYamlPath) {
        throw new Error("App manifest file not found");
      }
    }

    // Delete the definition from source YAML
    await this.deleteDefinitionFromYaml(
      definitionYamlPath,
      apiId,
      appId,
      componentId,
    );

    // Step 2: Remove from deployments in app-level YAML (if it's referenced)
    const appYamlPath = await this.manifestService.getAppYamlPath(appId);
    if (appYamlPath) {
      await this.removeApiFromDeployments(appYamlPath, apiId, version, appId);
    }
  };

  private deleteDefinitionFromYaml = async (
    yamlPath: string,
    apiId: string,
    appId: string,
    componentId?: string,
  ) => {
    const rawYaml = await readTextFile(yamlPath);
    const manifest: Document = parseDocument(rawYaml);

    const httpApi = manifest.get("httpApi") as YAMLMap | undefined;
    if (!httpApi) {
      return;
    }

    const definitions = httpApi.get("definitions") as YAMLMap | undefined;
    if (!definitions) {
      return;
    }

    // Delete the definition by ID
    definitions.delete(apiId);

    // Save back to file
    if (componentId) {
      await this.manifestService.saveComponentManifest(
        appId,
        componentId,
        manifest.toString(),
      );
    } else {
      await this.manifestService.saveAppManifest(appId, manifest.toString());
    }
  };

  private removeApiFromDeployments = async (
    yamlPath: string,
    apiId: string,
    version: string,
    appId: string,
  ) => {
    const rawYaml = await readTextFile(yamlPath);
    const manifest: Document = parseDocument(rawYaml);

    const httpApi = manifest.get("httpApi") as YAMLMap | undefined;
    if (!httpApi) {
      return;
    }

    const deployments = httpApi.get("deployments") as YAMLMap | undefined;
    if (!deployments || (deployments.items?.length || 0) === 0) {
      return;
    }

    let modified = false;
    const targetDefinition = `${apiId}@${version}`;

    // Iterate through all profiles (e.g., "local", "production", etc.)
    for (const pair of deployments.items) {
      const profileDeployments = pair.value as YAMLMap;

      if (!profileDeployments || !profileDeployments.items) continue;

      // For each deployment in the profile
      for (let i = profileDeployments.items.length - 1; i >= 0; i--) {
        const deployment = profileDeployments.items[i] as unknown as YAMLMap;
        const definitionsSeq = deployment.get("definitions") as
          | YAMLMap
          | undefined;

        if (definitionsSeq && definitionsSeq.items) {
          const originalLength = definitionsSeq.items.length;

          // Remove the specific API definition (apiId@version)
          definitionsSeq.items = definitionsSeq.items.filter(item => {
            const defString = String(item).trim();
            return defString !== targetDefinition;
          });

          if (definitionsSeq.items.length !== originalLength) {
            modified = true;
          }

          // If deployment has no more definitions, remove the entire deployment
          if (definitionsSeq.items.length === 0) {
            profileDeployments.items.splice(i, 1);
            modified = true;
          }
        }
      }
    }

    // Save if we made any changes
    if (modified) {
      await this.manifestService.saveAppManifest(appId, manifest.toString());
    }
  };

  public putApi = async (
    id: string,
    version: string,
    payload: HttpApiDefinition,
  ) => {
    const componentId = payload.componentId;
    let yamlPath = "";
    if (componentId) {
      const component = await this.componentService.getComponentById(
        id,
        componentId,
      );
      const componentYamlPath = await this.manifestService.getComponentYamlPath(
        id,
        component.componentName!,
      );
      yamlPath = componentYamlPath;
    } else {
      const app = await settingsService.getAppById(id);
      if (!app) {
        throw new Error("App not found");
      }
      yamlPath = await join(app.folderLocation, "golem.yaml");
    }

    // Load the YAML into memory, update and save
    const rawYaml = await readTextFile(yamlPath);
    // Parse as Document to preserve comments and formatting
    const manifest: Document = parseDocument(rawYaml);
    // Get or create httpApi section
    let httpApi = manifest.get("httpApi") as YAMLMap | undefined;
    if (!httpApi) {
      // Create new httpApi section if it doesn't exist
      manifest.set("httpApi", new YAMLMap());
      httpApi = manifest.get("httpApi") as YAMLMap;
    }
    // set the definition with the key
    let definitions = httpApi.get("definitions") as YAMLMap | undefined;
    if (!definitions) {
      // Create new definitions section if it doesn't exist
      httpApi.set("definitions", new YAMLMap());
      definitions = httpApi.get("definitions") as YAMLMap;
    }
    // Add or update the API definition
    payload.version = version;
    definitions.set(payload.id, serializeHttpApiDefinition(payload));
    // Save config back
    if (componentId) {
      await this.manifestService.saveComponentManifest(
        id,
        componentId,
        manifest.toString(),
      );
    } else {
      await this.manifestService.saveAppManifest(id, manifest.toString());
    }
  };

  public async createApiVersion(appId: string, payload: HttpApiDefinition) {
    // We need to know if the definition came from a component and store it there
    const app = await settingsService.getAppById(appId);
    let yamlToUpdate = app!.golemYamlLocation;

    if (payload.componentId) {
      const component = await this.componentService.getComponentById(
        appId,
        payload.componentId,
      );
      yamlToUpdate = await this.manifestService.getComponentYamlPath(
        appId,
        component.componentName!,
      );
    }

    // Now load the YAML into memory, update and save
    const rawYaml = await readTextFile(yamlToUpdate);

    // Parse as Document to preserve comments and formatting
    const manifest: Document = parseDocument(rawYaml);

    // Type-safe access to the parsed content
    // const manifestData = manifest.toJS() as GolemApplicationManifest;

    // Get or create httpApi section
    let httpApi = manifest.get("httpApi") as YAMLMap | undefined;
    if (!httpApi) {
      // Create new httpApi section if it doesn't exist
      manifest.set("httpApi", new YAMLMap());
      httpApi = manifest.get("httpApi") as YAMLMap;
    }

    // Get or create definitions section
    let definitions = httpApi.get("definitions") as YAMLMap | undefined;
    if (!definitions) {
      // Create new definitions section if it doesn't exist
      httpApi.set("definitions", new YAMLMap());
      definitions = httpApi.get("definitions") as YAMLMap;
    }

    // Add or update the API definition
    definitions.set(payload.id!, serializeHttpApiDefinition(payload));

    // Save config back
    if (payload.componentId) {
      await this.manifestService.saveComponentManifest(
        appId,
        payload.componentId,
        manifest.toString(),
      );
    } else {
      await this.manifestService.saveAppManifest(appId, manifest.toString());
    }
  }
}
