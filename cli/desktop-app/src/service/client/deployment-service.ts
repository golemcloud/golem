import { CLIService } from "./cli-service";
import { Deployment } from "@/types/deployments";

export class DeploymentService {
  private cliService: CLIService;

  constructor(cliService: CLIService) {
    this.cliService = cliService;
  }

  public getDeploymentApi = async (appId: string): Promise<Deployment[]> => {
    return (await this.cliService.callCLI(appId, "api", [
      "deployment",
      "list",
    ])) as Promise<Deployment[]>;
  };

  public deleteDeployment = async (appId: string, subdomain: string) => {
    return await this.cliService.callCLI(appId, "api", [
      "deployment",
      "delete",
      subdomain,
    ]);
  };

  public createDeployment = async (appId: string, subdomain?: string) => {
    const params = ["deployment", "deploy"];
    if (subdomain) {
      params.push(subdomain);
    }
    return await this.cliService.callCLI(appId, "api", params);
  };
}
