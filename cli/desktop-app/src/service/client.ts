import { CLIService } from "./client/cli-service";
import { ComponentService } from "./client/component-service";
import { WorkerService } from "./client/worker-service";
import { APIService } from "./client/api-service";
import { PluginService } from "./client/plugin-service";
import { DeploymentService } from "./client/deployment-service";
import { AppService } from "./client/app-service";
import { ManifestService } from "./client/manifest-service";

export class Service {
  public cliService: CLIService;
  public componentService: ComponentService;
  public workerService: WorkerService;
  public apiService: APIService;
  public pluginService: PluginService;
  public deploymentService: DeploymentService;
  public appService: AppService;
  public manifestService: ManifestService;

  constructor() {
    // Initialize services in the correct order to handle dependencies
    this.cliService = new CLIService();
    this.componentService = new ComponentService(this.cliService);
    this.manifestService = new ManifestService(this.cliService);
    this.workerService = new WorkerService(
      this.cliService,
      this.componentService,
    );
    this.apiService = new APIService(
      this.cliService,
      this.componentService,
      this.manifestService,
    );
    this.pluginService = new PluginService(this.cliService);
    this.deploymentService = new DeploymentService(this.cliService);
    this.appService = new AppService(this.cliService);
  }
}
