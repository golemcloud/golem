import { CLIService } from "./cli-service";

export class AppService {
  private cliService: CLIService;

  constructor(cliService: CLIService) {
    this.cliService = cliService;
  }

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

  public deployAgents = async (appId: string, updateAgents?: boolean) => {
    const subcommands: string[] = [];
    if (updateAgents) {
      subcommands.push("--update-agents");
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
