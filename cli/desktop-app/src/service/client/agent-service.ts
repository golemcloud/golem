import { Agent } from "../../types/agent";
import { InvokeResponse } from "../../hooks/useInvoke";
import { convertValuesToWaveArgs, convertPayloadToWaveArgs } from "@/lib/wave";
import { Typ } from "@/types/component";
import { CLIService } from "./cli-service";
import { ComponentService } from "./component-service";

export class AgentService {
  private cliService: CLIService;
  private componentService: ComponentService;

  constructor(cliService: CLIService, componentService: ComponentService) {
    this.cliService = cliService;
    this.componentService = componentService;
  }

  public upgradeAgent = async (
    appId: string,
    componentName: string,
    agentName: string,
    version: number,
    upgradeType: string,
  ) => {
    return await this.cliService.callCLI(appId, "agent", [
      "update",
      `${componentName}/${agentName}`,
      upgradeType,
      `${version}`,
    ]);
  };

  public findAgent = async (
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
    return (await this.cliService.callCLI(appId, "agent", params)) as Promise<{
      agents: Agent[];
    }>;
  };

  public deleteAgent = async (
    appId: string,
    componentId: string,
    agentName: string,
  ) => {
    let component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    return await this.cliService.callCLI(appId, "agent", [
      "delete",
      `${component?.componentName}/${agentName}`,
    ]);
  };

  public createAgent = async (
    appId: string,
    componentID: string,
    name: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentID,
    );
    return await this.cliService.callCLI(appId, "agent", [
      "new",
      `${component?.componentName!}/${name}`,
      // JSON.stringify(params),
    ]);
  };

  public getParticularAgent = async (
    appId: string,
    componentId: string,
    agentName: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    return await this.cliService.callCLI(appId, "agent", [
      "get",
      `${component?.componentName}/${agentName}`,
    ]);
  };

  public interruptAgent = async (
    appId: string,
    componentId: string,
    agentName: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullAgentName = `${component?.componentName}/${agentName}`;
    return await this.cliService.callCLI(appId, "agent", [
      "interrupt",
      fullAgentName,
    ]);
  };

  public resumeAgent = async (
    appId: string,
    componentId: string,
    agentName: string,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullAgentName = `${component?.componentName}/${agentName}`;
    return await this.cliService.callCLI(appId, "agent", [
      "resume",
      fullAgentName,
    ]);
  };

  public invokeAgentAwait = async (
    appId: string,
    componentId: string,
    agentName: string,
    functionName: string,
    payload: { params?: unknown[] } | unknown[] | undefined,
  ): Promise<InvokeResponse> => {
    // Get component name for proper agent identification
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullAgentName = `${component?.componentName}/${agentName}`;

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

    return (await this.cliService.callCLI(appId, "agent", [
      "invoke",
      fullAgentName,
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
    // Get component name for ephemeral agent identification
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const ephemeralAgentName = `${component?.componentName}/-`;

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

    return (await this.cliService.callCLI(appId, "agent", [
      "invoke",
      ephemeralAgentName,
      functionName,
      ...waveArgs,
    ])) as InvokeResponse;
  };

  public getOplog = async (
    appId: string,
    componentId: string,
    agentName: string,
    searchQuery: string,
  ) => {
    // Get component name for proper agent identification
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );
    const fullAgentName = `${component?.componentName}/${agentName}`;

    const r = await this.cliService.callCLI(appId, "agent", [
      "oplog",
      fullAgentName,
      `--query=${searchQuery}`,
    ]);
    return r;
  };
}
