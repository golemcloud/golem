import { Worker } from "./../../types/worker";
import { InvokeResponse } from "./../../hooks/useInvoke";
import { convertValuesToWaveArgs, convertPayloadToWaveArgs } from "@/lib/wave";
import { Typ } from "@/types/component";
import { CLIService } from "./cli-service";
import { ComponentService } from "./component-service";

export class WorkerService {
  private cliService: CLIService;
  private componentService: ComponentService;

  constructor(cliService: CLIService, componentService: ComponentService) {
    this.cliService = cliService;
    this.componentService = componentService;
  }

  public upgradeWorker = async (
    appId: string,
    componentName: string,
    workerName: string,
    version: number,
    upgradeType: string,
  ) => {
    return await this.cliService.callCLI(appId, "worker", [
      "update",
      `${componentName}/${workerName}`,
      upgradeType,
      `${version}`,
    ]);
  };

  public findWorker = async (
    appId: string,
    componentId: string,
    param = { count: 100, precise: true },
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const params = [
      "list",
      component.componentName!,
      `--max-count=${param.count}`,
    ];
    if (param.precise) {
      params.push(`--precise`);
    }
    return (await this.cliService.callCLI(appId, "worker", params)) as Promise<{
      workers: Worker[];
    }>;
  };

  public deleteWorker = async (
    appId: string,
    componentId: string,
    workerName: string,
  ) => {
    let component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    return await this.cliService.callCLI(appId, "worker", [
      "delete",
      `${component?.componentName}/${workerName}`,
    ]);
  };

  public createWorker = async (
    appId: string,
    componentID: string,
    name: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentID,
    );
    return await this.cliService.callCLI(appId, "worker", [
      "new",
      `${component?.componentName!}/${name}`,
      // JSON.stringify(params),
    ]);
  };

  public getParticularWorker = async (
    appId: string,
    componentId: string,
    workerName: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    return await this.cliService.callCLI(appId, "worker", [
      "get",
      `${component?.componentName}/${workerName}`,
    ]);
  };

  public interruptWorker = async (
    appId: string,
    componentId: string,
    workerName: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullWorkerName = `${component?.componentName}/${workerName}`;
    return await this.cliService.callCLI(appId, "worker", [
      "interrupt",
      fullWorkerName,
    ]);
  };

  public resumeWorker = async (
    appId: string,
    componentId: string,
    workerName: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullWorkerName = `${component?.componentName}/${workerName}`;
    return await this.cliService.callCLI(appId, "worker", [
      "resume",
      fullWorkerName,
    ]);
  };

  public invokeWorkerAwait = async (
    appId: string,
    componentId: string,
    workerName: string,
    functionName: string,
    payload: { params?: unknown[] } | unknown[] | undefined,
  ): Promise<InvokeResponse> => {
    // Get component name for proper worker identification
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullWorkerName = `${component?.componentName}/${workerName}`;

    // Convert payload to individual WAVE-formatted arguments using enhanced converter
    let waveArgs: string[];
    if (
      payload &&
      typeof payload === "object" &&
      !Array.isArray(payload) &&
      "params" in payload &&
      Array.isArray(payload.params)
    ) {
      // Use the enhanced payload converter that handles all WIT types
      waveArgs = convertPayloadToWaveArgs(
        payload as { params: Array<{ value: unknown; typ?: Typ }> },
      );
    } else if (Array.isArray(payload)) {
      // Legacy format - array of raw values
      waveArgs = convertValuesToWaveArgs(payload);
    } else {
      // Empty or invalid payload
      waveArgs = [];
    }

    return (await this.cliService.callCLI(appId, "worker", [
      "invoke",
      fullWorkerName,
      functionName,
      ...waveArgs,
    ])) as InvokeResponse;
  };

  public invokeEphemeralAwait = async (
    appId: string,
    componentId: string,
    functionName: string,
    payload: { params?: unknown[] } | unknown[] | undefined,
  ): Promise<InvokeResponse> => {
    // Get component name for ephemeral worker identification
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const ephemeralWorkerName = `${component?.componentName}/-`;

    // Convert payload to individual WAVE-formatted arguments using enhanced converter
    let waveArgs: string[];
    if (
      payload &&
      typeof payload === "object" &&
      !Array.isArray(payload) &&
      "params" in payload &&
      Array.isArray(payload.params)
    ) {
      // Use the enhanced payload converter that handles all WIT types
      waveArgs = convertPayloadToWaveArgs(
        payload as { params: Array<{ value: unknown; typ?: Typ }> },
      );
    } else if (Array.isArray(payload)) {
      // Legacy format - array of raw values
      waveArgs = convertValuesToWaveArgs(payload);
    } else {
      // Empty or invalid payload
      waveArgs = [];
    }

    return (await this.cliService.callCLI(appId, "worker", [
      "invoke",
      ephemeralWorkerName,
      functionName,
      ...waveArgs,
    ])) as InvokeResponse;
  };

  public getOplog = async (
    appId: string,
    componentId: string,
    workerName: string,
    searchQuery: string,
  ) => {
    // Get component name for proper worker identification
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullWorkerName = `${component?.componentName}/${workerName}`;

    const r = await this.cliService.callCLI(appId, "worker", [
      "oplog",
      fullWorkerName,
      `--query=${searchQuery}`,
    ]);
    return r;
  };
}
