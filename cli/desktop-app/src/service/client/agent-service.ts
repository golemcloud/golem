import { Agent } from "../../types/agent";
import { InvokeResponse } from "../../hooks/useInvoke";
import {
  convertValuesToWaveArgs,
  convertPayloadToWaveArgs,
  convertToWaveFormat,
} from "@/lib/wave";
import { Typ } from "@/types/component";
import { CLIService } from "./cli-service";
import { ComponentService } from "./component-service";
import { AgentTypeSchema } from "@/types/agent-types";

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
      "--component-name",
      component.componentName!,
      `--max-count=${param.count}`,
    ];
    if (param.precise) {
      params.push("--precise");
    }
    return (await this.cliService.callCLI(appId, "agent", params)) as Promise<{
      workers: Agent[];
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
    constructorParamsArray: {
      name: string;
      schema: {
        type: "ComponentModel";
        elementType: Typ;
      };
    }[],
    constructorParamTypes: Typ[] | undefined,
    args?: string[],
    env?: Record<string, string>,
  ) => {
    const component = await this.componentService.getComponentById(
      appId,
      componentID,
    );

    // Convert constructor parameters to WAVE format and build agent name
    let agentName = name;
    if (constructorParamsArray && constructorParamsArray.length > 0) {
      // Convert parameters to WAVE format using types if available
      const waveParams = constructorParamsArray.map((param, index) => {
        const typ = constructorParamTypes?.[index];
        // Remove spaces from the WAVE format to avoid "Worker name must not contain whitespaces" error
        const waveFormatted = convertToWaveFormat(param, typ);
        return waveFormatted.replace(/\s+/g, "");
      });

      // Construct the agent name with parameters (no spaces allowed)
      const paramsString = waveParams.join(",");
      agentName = `${name}(${paramsString})`;
    } else {
      // No constructor parameters - just add empty parentheses
      agentName = `${name}()`;
    }

    const commandArgs = ["new", `${component?.componentName!}/${agentName}`];

    // Add environment variables as -e flags
    if (env) {
      Object.entries(env).forEach(([key, value]) => {
        commandArgs.push("-e", `${key}=${value}`);
      });
    }

    // Add positional arguments
    if (args && args.length > 0) {
      commandArgs.push(...args);
    }

    return await this.cliService.callCLI(appId, "agent", commandArgs);
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
    return (await this.cliService.callCLI(appId, "agent", [
      "get",
      `${component?.componentName}/${agentName}`,
    ])) as Promise<{ metadata: Agent }>;
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
    // Get component name for proper identification
    const component = await this.componentService.getComponentById(
      appId,
      componentId,
    );

    // Extract agent type from function name (e.g. "pack:ts/human-agent.{fn}" -> "human-agent")
    const interfacePart = functionName.split(".{")[0] || "";
    const agentType = interfacePart.split("/").pop() || "";
    const ephemeralId = `desktop-app-${crypto.randomUUID()}`;
    const ephemeralAgentName = `${component?.componentName}/${agentType}("${ephemeralId}")`;

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

  public async getAgentTypesForComponent(
    appId: string,
    componentId: string,
  ): Promise<AgentTypeSchema[]> {
    // golem-cli list-agent-types --format=json (v1.4.2: moved to root level)
    const result = (await this.cliService.callCLI(
      appId,
      "list-agent-types",
      [],
    )) as AgentTypeSchema[];
    return result.filter(spec => spec.implementedBy.componentId == componentId);
  }
}
