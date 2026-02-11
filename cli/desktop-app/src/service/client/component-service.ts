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
      c => c.componentId === componentId && c.componentRevision === version,
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
    _appId: string,
    _componentId: string,
    _installationId: string,
  ) => {
    // CLI v1.4.2: component plugin commands have been removed
    // This feature is temporarily unavailable until alternative implementation
    throw new Error(
      "Component plugin uninstall is temporarily unavailable in CLI v1.4.2",
    );
  };

  public getInstalledPlugins = async (_appId: string, _componentId: string) => {
    // CLI v1.4.2: component plugin commands have been removed
    // Return empty array until alternative implementation is available
    console.warn(
      "Component plugin get is temporarily unavailable in CLI v1.4.2",
    );
    return [];
  };

  public addPluginToComponentWithApp = async (
    _appId: string,
    _componentId: string,
    _form: {
      name: string;
      version: string;
      priority: number;
      parameters?: Record<string, unknown>;
    },
  ) => {
    // CLI v1.4.2: component plugin commands have been removed
    // This feature is temporarily unavailable until alternative implementation
    throw new Error(
      "Component plugin install is temporarily unavailable in CLI v1.4.2",
    );
  };

  /**
   * Get the list of unbuilt components (folders that exist but are not yet built)
   * @param appId - The ID of the application
   * @returns {Promise<{name: string}[]>} - Array of unbuilt component names
   */
  public getUnbuiltComponents = async (
    appId: string,
  ): Promise<{ name: string }[]> => {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      return [];
    }

    try {
      // Get all built components
      let builtComponents: Component[] = [];
      try {
        builtComponents = await this.getComponents(appId);
      } catch (error) {
        console.error("Failed to get built components:", error);
      }

      // Extract built component names and convert to filesystem format (colons to hyphens)
      const builtComponentNames = new Set(
        builtComponents.map(c => {
          return c.componentName!.replace(/:/g, "-").toLowerCase();
        }),
      );

      // Get all component folders
      const appEntries = await readDir(app.folderLocation);
      const componentsFolders = appEntries
        .filter(
          entry => entry.isDirectory && entry.name.startsWith("components-"),
        )
        .map(entry => entry.name);

      const unbuiltComponents: { name: string }[] = [];

      // Check each components-* folder
      for (const componentsFolder of componentsFolders) {
        const componentsFolderPath = await join(
          app.folderLocation,
          componentsFolder,
        );

        try {
          const subEntries = await readDir(componentsFolderPath);
          const subFolders = subEntries.filter(entry => entry.isDirectory);

          // Check each subfolder
          for (const folder of subFolders) {
            const folderNameLower = folder.name.toLowerCase();

            // If this folder name is not in built components, it's unbuilt
            if (!builtComponentNames.has(folderNameLower)) {
              // Convert filesystem folder name back to component name format (last hyphen to colon)
              const componentName = folderNameLower.replace(/-([^-]*)$/, ":$1");
              unbuiltComponents.push({ name: componentName });
            }
          }
        } catch (error) {
          console.warn(
            `Failed to read components folder ${componentsFolder}:`,
            error,
          );
        }
      }

      return unbuiltComponents;
    } catch (error) {
      console.error("Error getting unbuilt components:", error);
      return [];
    }
  };

  public getComponentByIdAsKey = async (
    appId: string,
  ): Promise<Record<string, ComponentList>> => {
    // Assume getComponents returns a Promise<RawComponent[]>
    const components = await this.getComponents(appId);

    return components.reduce<Record<string, ComponentList>>(
      (acc, component) => {
        const { componentName, componentId, componentType, componentRevision } =
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
          acc[key].versionList.push(componentRevision!);
        }
        if (acc[key].versions) {
          acc[key].versions.push(component);
        }
        return acc;
      },
      {},
    );
  };

  public getComponentTemplates = async (): Promise<
    { language: string; template: string; description: string }[]
  > => {
    const { invoke } = await import("@tauri-apps/api/core");
    const result = await invoke<string>("get_component_templates");
    return JSON.parse(result);
  };
}
