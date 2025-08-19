import { Store, load } from "@tauri-apps/plugin-store";
import { exists } from "@tauri-apps/plugin-fs";

const SETTINGS_FILE = "settings.json";
const GOLEM_CLI_PATH_KEY = "golem_cli_path";
const APPS_KEY = "apps";

export interface App {
  id: string;
  name?: string;
  folderLocation: string;
  golemYamlLocation: string;
  lastOpened: string;
}

async function validFolderFilter(app: App): Promise<boolean> {
  let validFolder = false;
  try {
    validFolder = await exists(app.folderLocation);
  } catch (error) {
    console.error(
      `Error checking existence of folder ${app.folderLocation}:`,
      error,
    );
  }
  return validFolder;
}

export class SettingsService {
  private readonly storeName: string;

  constructor() {
    this.storeName = SETTINGS_FILE;
  }

  private async getStore(): Promise<Store> {
    return await load(this.storeName);
  }

  // Golem CLI Path
  async getGolemCliPath(): Promise<string | null> {
    try {
      const store = await this.getStore();
      const path = await store.get(GOLEM_CLI_PATH_KEY);
      return path as string | null;
    } catch (error) {
      console.error("Error getting golem-cli path:", error);
      return null;
    }
  }

  async setGolemCliPath(path: string): Promise<boolean> {
    try {
      const store = await this.getStore();
      await store.set(GOLEM_CLI_PATH_KEY, path);
      await store.save();
      return true;
    } catch (error) {
      console.error("Error saving golem-cli path:", error);
      return false;
    }
  }

  // Application Management
  async getApps(): Promise<App[]> {
    try {
      const store = await this.getStore();
      const apps = ((await store.get(APPS_KEY)) as App[] | null) || [];
      let validApps: App[] = [];
      // filter by what exists via folderLocation
      if (apps && Array.isArray(apps)) {
        const results = await Promise.all(apps.map(validFolderFilter));
        validApps = apps.filter((_, index) => results[index]);
        // if not equal, update
        if (validApps.length !== apps.length) {
          await store.set(APPS_KEY, validApps);
          await store.save();
        }
      }

      return (validApps as App[]) || [];
    } catch (error) {
      console.error("Error getting apps:", error);
      return [];
    }
  }

  async addApp(app: App): Promise<boolean> {
    try {
      const store = await this.getStore();
      const apps = await this.getApps();
      app.name = (app.name || app.folderLocation.split("/").pop() || "")
        .split(/[-_]/)
        .map(word => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase())
        .join(" ");

      // check for same folder location
      const sameFolderIndex = apps.findIndex(
        a => a.folderLocation === app.folderLocation,
      );

      if (sameFolderIndex >= 0) {
        // Update existing app
        apps[sameFolderIndex] = app;
      } else {
        // Add new app
        apps.push(app);
      }

      await store.set(APPS_KEY, apps);
      await store.save();
      return true;
    } catch (error) {
      console.error("Error saving app:", error);
      return false;
    }
  }

  // async removeApp(appId: string): Promise<boolean> {
  //   try {
  //     const store = await this.getStore();
  //     const apps = await this.getApps();
  //     const filteredApps = apps.filter(app => app.id !== appId);
  //
  //     await store.set(APPS_KEY, filteredApps);
  //     await store.save();
  //     return true;
  //   } catch (error) {
  //     console.error("Error removing app:", error);
  //     return false;
  //   }
  // }

  async updateAppLastOpened(appId: string): Promise<boolean> {
    try {
      const store = await this.getStore();
      const apps = await this.getApps();
      const appIndex = apps.findIndex(app => app.id === appId);

      if (appIndex >= 0) {
        if (apps[appIndex]) {
          apps[appIndex].lastOpened = new Date().toISOString();
          await store.set(APPS_KEY, apps);
          await store.save();
          return true;
        }
      }
      return false;
    } catch (error) {
      console.error("Error updating app last opened:", error);
      return false;
    }
  }

  // Get app by ID
  async getAppById(appId: string): Promise<App | undefined> {
    try {
      const apps = await this.getApps();
      return apps.find(app => app.id === appId);
    } catch (error) {
      console.error("Error getting app by ID:", error);
      return undefined;
    }
  }

  // Check if a folder contains a golem.yaml file
  async validateGolemApp(
    folderPath: string,
  ): Promise<{ isValid: boolean; yamlPath: string }> {
    try {
      const yamlPath = `${folderPath}/golem.yaml`;
      const fileExists = await exists(yamlPath);

      return {
        isValid: fileExists,
        yamlPath: fileExists ? yamlPath : "",
      };
    } catch (error) {
      console.error("Error validating golem app:", error);
      return { isValid: false, yamlPath: "" };
    }
  }
}

export const storeService = new SettingsService();
// For backward compatibility
export const settingsService = storeService;
