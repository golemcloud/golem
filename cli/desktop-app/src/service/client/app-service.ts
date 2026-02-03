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
    // Use 'plugin list' as it works from any directory and actually connects to the server
    // This is more reliable than 'api deployment list' which requires app context
    await invoke("call_golem_command", {
      command: "plugin",
      subcommands: ["list"],
      folderPath: appCacheDirPath,
    });
  };

  public buildApp = async (appId: string, componentNames?: string[]) => {
    const subcommands: string[] = [];
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(appId, "build", subcommands);
  };

  public updateAgents = async (
    appId: string,
    componentNames?: string[],
    updateMode: string = "auto",
  ) => {
    const subcommands: string[] = [];
    if (updateMode) {
      subcommands.push("--update-mode", updateMode);
    }
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(
      appId,
      "update-agents",
      subcommands,
    );
  };

  public deployAgents = async (
    appId: string,
    componentNames?: string[],
    updateAgents?: boolean,
  ) => {
    const subcommands: string[] = [];
    if (updateAgents) {
      subcommands.push("--update-agents");
    }
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(appId, "deploy", subcommands);
  };

  public cleanApp = async (appId: string, componentNames?: string[]) => {
    const subcommands: string[] = [];
    if (componentNames && componentNames.length > 0) {
      subcommands.push(...componentNames);
    }
    return await this.cliService.callCLIWithLogs(appId, "clean", subcommands);
  };
}
