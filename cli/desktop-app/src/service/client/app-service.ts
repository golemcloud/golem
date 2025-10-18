import { localDataDir } from "@tauri-apps/api/path";
import { CLIService } from "./cli-service";
import { invoke } from "@tauri-apps/api/core";

export class AppService {
  private cliService: CLIService;

  constructor(cliService: CLIService) {
    this.cliService = cliService;
  }

  /**
   * checkHealth: Check if the CLI connection is healthy
   * @returns {Promise<void>} - Resolves if the connection is healthy, rejects if not
   */
  public checkHealth = async (): Promise<void> => {
    const appCacheDirPath = await localDataDir();
    await invoke("call_golem_command", {
      command: "api",
      subcommands: ["deployment", "list"],
      folderPath: appCacheDirPath,
    });
  };

  public buildApp = async (appId: string, componentNames?: string[]) => {
    const subcommands = ["build"];
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(appId, "app", subcommands);
  };

  public updateAgents = async (
    appId: string,
    componentNames?: string[],
    updateMode: string = "auto",
  ) => {
    const subcommands = ["update-agents"];
    if (updateMode) {
      subcommands.push("--update-mode", updateMode);
    }
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(appId, "app", subcommands);
  };

  public deployAgents = async (
    appId: string,
    componentNames?: string[],
    updateAgents?: boolean,
  ) => {
    const subcommands = ["deploy"];
    if (updateAgents) {
      subcommands.push("--update-agents");
    }
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(appId, "app", subcommands);
  };

  public cleanApp = async (appId: string, componentNames?: string[]) => {
    const subcommands = ["clean"];
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(appId, "app", subcommands);
  };
}
