import { toast } from "@/hooks/use-toast";
import { settingsService } from "@/lib/settings.ts";
import {
  readDir,
  readTextFile,
  writeTextFile,
  exists,
} from "@tauri-apps/plugin-fs";
import { join } from "@tauri-apps/api/path";
import { GolemApplicationManifest } from "@/types/golemManifest.ts";
import { parse } from "yaml";
import { CLIService } from "./cli-service";
import { Component } from "@/types/component.ts";
import { AppYamlFiles } from "@/types/yaml-files.ts";

export class ManifestService {
  private cliService: CLIService;

  constructor(cliService: CLIService) {
    this.cliService = cliService;
  }

  /**
   * getComponentYamlPath: Get the path to the YAML file of a component
   * @param appId - The ID of the application
   * @param componentName - The name of the component
   * @returns {Promise<string | null>} - The path to the YAML file or null if not found
   */
  public async getComponentYamlPath(
    appId: string,
    componentName: string,
  ): Promise<string> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    // Convert colons to hyphens for filesystem compatibility
    const folderName = componentName.replace(/:/g, "-").toLowerCase();

    try {
      // Get all folders in app.folderLocation
      const appEntries = await readDir(app.folderLocation);
      const appFolders = appEntries
        .filter(entry => entry.isDirectory)
        .map(entry => entry.name);

      // Find all folders starting with "components-"
      const componentsFolders = appFolders.filter(folder =>
        folder.startsWith("components-"),
      );

      // Search through each component-* folder for the component
      for (const componentsFolder of componentsFolders) {
        const componentsFolderPath = await join(
          app.folderLocation,
          componentsFolder,
        );

        try {
          const subEntries = await readDir(componentsFolderPath);
          const subFolders = subEntries
            .filter(entry => entry.isDirectory)
            .map(entry => entry.name.toLowerCase());

          // Check if our target folder exists
          if (subFolders.includes(folderName)) {
            const componentPath = await join(componentsFolderPath, folderName);

            // Check if the component path exists
            if (await exists(componentPath)) {
              // Look for the golem YAML file in the component folder
              const files = await readDir(componentPath);
              const yamlFile = files
                .filter(entry => !entry.isDirectory)
                .map(entry => entry.name)
                .find(file => file === "golem.yaml" || file === "golem.yml");

              if (yamlFile) {
                return await join(componentPath, yamlFile);
              }
            }
          }
        } catch (error) {
          // Continue to the next components folder if this one fails
          console.warn(
            `Failed to read components folder ${componentsFolder}:`,
            error,
          );
        }
      }

      // Component folder isn't found in any components-* directory
      toast({
        title: "Error finding Component Manifest",
        description:
          "Could not find component golem.yaml for matched component in this app",
        variant: "destructive",
        duration: 5000,
      });
    } catch (error) {
      throw new Error(`Failed to scan app folder: ${error}`);
    }

    throw new Error(`Error finding Component Manifest`);
  }

  public async getAppYamlPath(appId: string): Promise<string | null> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }
    let appYamlPath = await join(app.folderLocation, "golem.yaml");
    if (!(await exists(appYamlPath))) {
      appYamlPath = await join(app.folderLocation, "golem.yml");
    }
    return appYamlPath;
  }

  public async getComponentManifest(
    appId: string,
    componentId: string,
  ): Promise<GolemApplicationManifest> {
    const component = await this.getComponentById(appId, componentId);
    let componentYamlPath = await this.getComponentYamlPath(
      appId,
      component.componentName!,
    );
    let rawYaml = await readTextFile(componentYamlPath);

    return parse(rawYaml) as GolemApplicationManifest;
  }

  public async getAppManifest(
    appId: string,
  ): Promise<GolemApplicationManifest> {
    let appYamlPath = await this.getAppYamlPath(appId);
    if (!appYamlPath) {
      throw new Error("App manifest file not found");
    }
    let rawYaml = await readTextFile(appYamlPath);

    return parse(rawYaml) as GolemApplicationManifest;
  }

  public async saveComponentManifest(
    appId: string,
    componentId: string,
    manifest: string,
  ): Promise<boolean> {
    const component = await this.getComponentById(appId, componentId);
    let componentYamlPath = await this.getComponentYamlPath(
      appId,
      component.componentName!,
    );
    // Write the YAML string to the file
    await writeTextFile(componentYamlPath, manifest);

    return true;
  }

  public async saveAppManifest(
    appId: string,
    manifest: string,
  ): Promise<boolean> {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }
    let appManifestPath = await join(app.folderLocation, "golem.yaml");
    await writeTextFile(appManifestPath, manifest);

    return true;
  }

  public getAppYamlContent = async (appId: string): Promise<string> => {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }
    const appManifestPath = await join(app.folderLocation, "golem.yaml");
    if (await exists(appManifestPath)) {
      return await readTextFile(appManifestPath);
    }
    const appManifestPathYml = await join(app.folderLocation, "golem.yml");
    if (await exists(appManifestPathYml)) {
      return await readTextFile(appManifestPathYml);
    }
    throw new Error("App manifest file not found");
  };

  public getComponentYamlContent = async (
    appId: string,
    componentName: string,
  ): Promise<string> => {
    const componentYamlPath = await this.getComponentYamlPath(
      appId,
      componentName,
    );
    return await readTextFile(componentYamlPath);
  };

  public getAllAppYamlFiles = async (appId: string): Promise<AppYamlFiles> => {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    const result: AppYamlFiles = {
      root: undefined,
      common: [],
      components: [],
    };

    // 1. Get root golem.yaml
    try {
      const rootYamlPath = await this.getAppYamlPath(appId);
      if (rootYamlPath) {
        const content = await readTextFile(rootYamlPath);
        result.root = {
          name: "golem.yaml",
          path: rootYamlPath,
          content,
          type: "root",
          editable: true,
        };
      }
    } catch (error) {
      console.warn("Failed to load root golem.yaml:", error);
    }

    // 2. Scan for common-*/golem.yaml files
    try {
      const appEntries = await readDir(app.folderLocation);
      const commonFolders = appEntries
        .filter(entry => entry.isDirectory && entry.name.startsWith("common-"))
        .map(entry => entry.name);

      for (const commonFolder of commonFolders) {
        try {
          const commonFolderPath = await join(app.folderLocation, commonFolder);
          const commonYamlPath = await join(commonFolderPath, "golem.yaml");

          if (await exists(commonYamlPath)) {
            const content = await readTextFile(commonYamlPath);
            result.common.push({
              name: `${commonFolder}/golem.yaml`,
              path: commonYamlPath,
              content,
              type: "common",
              editable: true,
            });
          } else {
            // Try .yml extension
            const commonYmlPath = await join(commonFolderPath, "golem.yml");
            if (await exists(commonYmlPath)) {
              const content = await readTextFile(commonYmlPath);
              result.common.push({
                name: `${commonFolder}/golem.yml`,
                path: commonYmlPath,
                content,
                type: "common",
                editable: true,
              });
            }
          }
        } catch (error) {
          console.warn(`Failed to read common folder ${commonFolder}:`, error);
        }
      }
    } catch (error) {
      console.warn("Failed to scan for common folders:", error);
    }

    // 3. Scan for components-*/*/golem.yaml files
    try {
      const appEntries = await readDir(app.folderLocation);
      const componentsFolders = appEntries
        .filter(
          entry => entry.isDirectory && entry.name.startsWith("components-"),
        )
        .map(entry => entry.name);

      for (const componentsFolder of componentsFolders) {
        try {
          const componentsFolderPath = await join(
            app.folderLocation,
            componentsFolder,
          );
          const subEntries = await readDir(componentsFolderPath);
          const subFolders = subEntries
            .filter(entry => entry.isDirectory)
            .map(entry => entry.name);

          for (const subFolder of subFolders) {
            try {
              const componentPath = await join(componentsFolderPath, subFolder);
              const componentYamlPath = await join(componentPath, "golem.yaml");

              if (await exists(componentYamlPath)) {
                const content = await readTextFile(componentYamlPath);
                result.components.push({
                  name: `${componentsFolder}/${subFolder}/golem.yaml`,
                  path: componentYamlPath,
                  content,
                  type: "component",
                  editable: true,
                });
              } else {
                // Try .yml extension
                const componentYmlPath = await join(componentPath, "golem.yml");
                if (await exists(componentYmlPath)) {
                  const content = await readTextFile(componentYmlPath);
                  result.components.push({
                    name: `${componentsFolder}/${subFolder}/golem.yml`,
                    path: componentYmlPath,
                    content,
                    type: "component",
                    editable: true,
                  });
                }
              }
            } catch (error) {
              console.warn(
                `Failed to read component folder ${subFolder}:`,
                error,
              );
            }
          }
        } catch (error) {
          console.warn(
            `Failed to read components folder ${componentsFolder}:`,
            error,
          );
        }
      }
    } catch (error) {
      console.warn("Failed to scan for component folders:", error);
    }

    return result;
  };

  public saveYamlFile = async (
    filePath: string,
    content: string,
  ): Promise<void> => {
    await writeTextFile(filePath, content);
  };

  // Helper method to get component by ID (needed for manifest operations)
  private async getComponentById(
    appId: string,
    componentId: string,
  ): Promise<Component> {
    const r = (await this.cliService.callCLI(appId, "component", [
      "list",
    ])) as Component[];
    const c = r.find(c => c.componentId === componentId);
    if (!c) {
      throw new Error("Could not find component");
    }
    return c;
  }
}
