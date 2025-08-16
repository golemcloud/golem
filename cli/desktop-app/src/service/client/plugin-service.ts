import { Plugin, PluginList } from "@/types/plugin";
import { CreatePluginFormData } from "@/pages/plugin/create";
import { settingsService } from "@/lib/settings.ts";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { join } from "@tauri-apps/api/path";
import { stringify } from "yaml";
import { CLIService } from "./cli-service";

export class PluginService {
  private cliService: CLIService;

  constructor(cliService: CLIService) {
    this.cliService = cliService;
  }

  public getPlugins = async (appId: string): Promise<PluginList[]> => {
    const rawPlugins = await this.cliService.callCLI(appId, "plugin", ["list"]);

    // Group plugins by name
    const grouped = (rawPlugins as Plugin[]).reduce(
      (acc: Record<string, Plugin[]>, plugin: Plugin) => {
        if (!acc[plugin.name]) {
          acc[plugin.name] = [];
        }
        acc[plugin.name]!.push(plugin);
        return acc;
      },
      {},
    );

    // Convert to PluginList array
    return Object.entries(grouped).map(([name, versions]) => ({
      name,
      versions: (versions as Plugin[]).sort((a: Plugin, b: Plugin) =>
        b.version.localeCompare(a.version),
      ), // Sort versions descending
    }));
  };

  public getPluginByName = async (
    appId: string,
    name: string,
  ): Promise<Plugin[]> => {
    const allPlugins = await this.cliService.callCLI(appId, "plugin", ["list"]);
    return (allPlugins as Plugin[]).filter(
      (plugin: Plugin) => plugin.name === name,
    );
  };

  public createPlugin = async (
    appId: string,
    pluginData: CreatePluginFormData,
  ) => {
    const app = await settingsService.getAppById(appId);
    if (!app) {
      throw new Error("App not found");
    }

    // Create plugin.yaml content
    const pluginYaml = this.generatePluginYaml(pluginData);

    // Create plugin.yaml file using plugin name
    const pluginFilePath = await join(
      app.folderLocation,
      `${pluginData.name}.yaml`,
    );

    // Write the plugin.yaml file
    await writeTextFile(pluginFilePath, pluginYaml);

    // Register the plugin using CLI
    return await this.cliService.callCLI(appId, "plugin", [
      "register",
      pluginFilePath,
    ]);
  };

  private generatePluginYaml = (pluginData: CreatePluginFormData): string => {
    // Use yaml library to stringify - data structure already matches YAML format
    return stringify(pluginData);
  };

  public registerPlugin = async (
    appId: string,
    manifestFileLocation: string,
  ) => {
    return await this.cliService.callCLI(appId, "plugin", [
      "register",
      manifestFileLocation,
    ]);
  };

  public deletePlugin = async (
    appId: string,
    name: string,
    version: string,
  ) => {
    return await this.cliService.callCLI(appId, "plugin", [
      "unregister",
      name,
      version,
    ]);
  };
}
