import { Component, ComponentList } from "@/types/component.ts";
import { parseErrorResponse } from "@/service/error-handler.ts";
import { settingsService } from "@/lib/settings.ts";
import { readDir } from "@tauri-apps/plugin-fs";
import { join } from "@tauri-apps/api/path";
import { CLIService } from "./cli-service";

export class ComponentService {
  private cliService: CLIService;

  constructor(cliService: CLIService) {
    this.cliService = cliService;
  }

  /**
   * getComponents: Get the list of all components
   * Note: Sample Endpoint https://release.api.golem.cloud/v1/components
   * @returns {Promise<Component[]>}
   */
  public getComponents = async (appId: string): Promise<Component[]> => {
    // Check if app has any components before making CLI call
    const hasComponents = await this.hasComponents(appId);
    if (!hasComponents) {
      return [];
    }

    const r = await this.cliService.callCLI(appId, "component", ["list"]);
    return r as Component[];
  };

  /**
   * Check if app has any components by looking for non-empty components-* folders
   * @param appId - The ID of the application
   * @returns {Promise<boolean>} - True if app has components, false otherwise
   */
  private hasComponents = async (appId: string): Promise<boolean> => {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      return false;
    }

    try {
      // Get all entries in app folder
      const appEntries = await readDir(app.folderLocation);
      const appFolders = appEntries
        .filter(entry => entry.isDirectory)
        .map(entry => entry.name);

      // Find all folders starting with "components-"
      const componentsFolders = appFolders.filter(folder =>
        folder.startsWith("components-"),
      );

      // If no components-* folders exist, no components
      if (componentsFolders.length === 0) {
        return false;
      }

      // Check if any components-* folder has content
      for (const componentsFolder of componentsFolders) {
        const componentsFolderPath = await join(
          app.folderLocation,
          componentsFolder,
        );

        try {
          const subEntries = await readDir(componentsFolderPath);
          const subFolders = subEntries.filter(entry => entry.isDirectory);

          // If any components-* folder has subdirectories, we have components
          if (subFolders.length > 0) {
            return true;
          }
        } catch (error) {
          // Continue to next folder if this one fails
          console.warn(
            `Failed to read components folder ${componentsFolder}:`,
            error,
          );
        }
      }

      // All components-* folders are empty
      return false;
    } catch (error) {
      console.error("Error checking for components:", error);
      return false;
    }
  };

  public getComponentById = async (appId: string, componentId: string) => {
    const r = (await this.cliService.callCLI(appId, "component", [
      "list",
    ])) as Component[];
    const c = r.find(c => c.componentId === componentId);
    if (!c) {
      throw new Error("Could not find component");
    }
    return c;
  };

  public getComponentByIdAndVersion = async (
    appId: string,
    componentId: string,
    version: number,
  ) => {
    const r = (await this.cliService.callCLI(appId, "component", [
      "list",
    ])) as Component[];
    return r.find(
      c => c.componentId === componentId && c.componentVersion === version,
    );
  };

  public createComponent = async (
    appId: string,
    name: string,
    template: string,
  ) => {
    try {
      await this.cliService.callCLI(appId, "component", [
        "new",
        template,
        name,
      ]);
    } catch (error) {
      console.error("Error in createComponent:", error);
      parseErrorResponse(error);
    }
  };

  public getComponentByName = async (appId: string, name: string) => {
    const r = (await this.cliService.callCLI(appId, "component", [
      "get",
      name,
    ])) as Component[];
    return r as Component;
  };

  public deletePluginToComponentWithApp = async (
    appId: string,
    componentId: string,
    installationId: string,
  ) => {
    // Get the component details to find the component name
    const component = await this.getComponentById(appId, componentId);
    if (!component) {
      throw new Error(`Component with ID ${componentId} not found`);
    }

    try {
      // Use CLI to uninstall plugin from component using installation ID
      const componentName = component.componentName || componentId;
      const args = [
        "plugin",
        "uninstall",
        "--installation-id",
        installationId,
        componentName,
      ];

      return await this.cliService.callCLI(appId, "component", args);
    } catch (error) {
      console.error("Failed to uninstall plugin:", error);
      throw error;
    }
  };

  public getInstalledPlugins = async (appId: string, componentId: string) => {
    // Get the component details to find the component name
    const component = await this.getComponentById(appId, componentId);
    if (!component) {
      throw new Error(`Component with ID ${componentId} not found`);
    }

    try {
      // Use CLI to get installed plugins for component
      const componentName = component.componentName || componentId;
      const args = ["plugin", "get", componentName];

      const result = await this.cliService.callCLI(appId, "component", args);

      // The CLI returns an array of plugin objects with this structure:
      // [{"id":"...","pluginName":"...","pluginVersion":"...","pluginRegistered":true,"priority":1,"parameters":{}}]

      // Transform the CLI response to match our InstalledPlugin interface
      return (
        (result as Array<{
          id: string;
          pluginName: string;
          pluginVersion: string;
          priority: number;
          parameters?: Record<string, unknown>;
        }>) || []
      ).map(plugin => ({
        id: plugin.id,
        name: plugin.pluginName,
        version: plugin.pluginVersion,
        priority: plugin.priority,
        parameters: plugin.parameters || {},
      }));
    } catch (error) {
      console.error("Failed to get installed plugins:", error);
      throw error;
    }
  };

  public addPluginToComponentWithApp = async (
    appId: string,
    componentId: string,
    form: {
      name: string;
      version: string;
      priority: number;
      parameters?: Record<string, unknown>;
    },
  ) => {
    const { name, version, priority, parameters } = form;

    // Get the component details to find the component name
    const component = await this.getComponentById(appId, componentId);
    if (!component) {
      throw new Error(`Component with ID ${componentId} not found`);
    }

    try {
      // Use CLI to install plugin to component
      const componentName = component.componentName || componentId;
      const args = [
        "plugin",
        "install",
        "--plugin-name",
        name,
        "--plugin-version",
        version,
        "--priority",
        priority.toString(),
        componentName,
      ];

      // Add parameters if provided (note: this might need adjustment based on actual CLI spec)
      if (parameters && Object.keys(parameters).length > 0) {
        // Convert parameters object to CLI format
        for (const [key, value] of Object.entries(parameters)) {
          args.push("--parameter", `${key}=${value}`);
        }
      }

      return await this.cliService.callCLI(appId, "component", args);
    } catch (error) {
      console.error("Failed to install plugin:", error);
      throw error;
    }
  };

  public getComponentByIdAsKey = async (
    appId: string,
  ): Promise<Record<string, ComponentList>> => {
    // Assume getComponents returns a Promise<RawComponent[]>
    const components = await this.getComponents(appId);

    return components.reduce<Record<string, ComponentList>>(
      (acc, component) => {
        const { componentName, componentId, componentType, componentVersion } =
          component;

        // Use componentId as the key. If not available, you might want to skip or handle differently.
        const key = componentId || "";

        // Initialize the component entry if it doesn't exist
        if (!acc[key]) {
          acc[key] = {
            componentName: componentName || "",
            componentId: componentId || "",
            componentType: componentType || "",
            versions: [],
            versionList: [],
          };
        }
        if (acc[key].versionList) {
          acc[key].versionList.push(componentVersion!);
        }
        if (acc[key].versions) {
          acc[key].versions.push(component);
        }
        return acc;
      },
      {},
    );
  };
}
