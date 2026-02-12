import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  type MockedFunction,
} from "vitest";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/lib/settings", () => ({
  settingsService: {
    getAppById: vi.fn().mockResolvedValue({
      id: "app-1",
      name: "test",
      folderLocation: "/test/app",
      golemYamlLocation: "/test/app/golem.yaml",
    }),
  },
}));
vi.mock("@/hooks/use-toast", () => ({ toast: vi.fn() }));
vi.mock("@tauri-apps/plugin-fs", () => ({
  readDir: vi.fn().mockResolvedValue([]),
}));
vi.mock("@tauri-apps/api/path", () => ({
  join: vi.fn((...args: string[]) => Promise.resolve(args.join("/"))),
}));

import { CLIService } from "../client/cli-service";
import { ComponentService } from "../client/component-service";
import { AgentService } from "../client/agent-service";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;

describe("AgentService CLI commands", () => {
  let service: AgentService;

  beforeEach(() => {
    vi.clearAllMocks();
    // Default: invoke returns an empty array (valid JSON)
    mockedInvoke.mockResolvedValue("[]");

    const cliService = new CLIService();
    const componentService = new ComponentService(cliService);
    service = new AgentService(cliService, componentService);
  });

  // Helper: make getComponentById resolve a component, then the actual call
  function mockComponentThenResult(
    componentName: string,
    result: string = "[]",
  ) {
    mockedInvoke
      .mockResolvedValueOnce(
        JSON.stringify([
          { componentId: "comp-id", componentName: componentName },
        ]),
      )
      .mockResolvedValueOnce(result);
  }

  describe("upgradeAgent", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValue("true");
      await service.upgradeAgent("app-1", "comp", "agent", 1, "auto");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["update", "comp/agent", "auto", "1"],
        folderPath: "/test/app",
      });
    });
  });

  describe("findAgent", () => {
    it("sends correct command with defaults", async () => {
      mockComponentThenResult("comp");
      await service.findAgent("app-1", "comp-id");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: [
          "list",
          "--component-name",
          "comp",
          "--max-count=100",
          "--precise",
        ],
        folderPath: "/test/app",
      });
    });

    it("sends command without --precise when precise=false", async () => {
      mockComponentThenResult("comp");
      await service.findAgent("app-1", "comp-id", {
        count: 50,
        precise: false,
      });
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["list", "--component-name", "comp", "--max-count=50"],
        folderPath: "/test/app",
      });
    });

    it("sends command with custom count", async () => {
      mockComponentThenResult("comp");
      await service.findAgent("app-1", "comp-id", {
        count: 25,
        precise: true,
      });
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: [
          "list",
          "--component-name",
          "comp",
          "--max-count=25",
          "--precise",
        ],
        folderPath: "/test/app",
      });
    });
  });

  describe("deleteAgent", () => {
    it("sends correct command", async () => {
      mockComponentThenResult("comp");
      await service.deleteAgent("app-1", "comp-id", "agent");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["delete", "comp/agent"],
        folderPath: "/test/app",
      });
    });
  });

  describe("createAgent", () => {
    it("sends correct command with no constructor params", async () => {
      mockComponentThenResult("comp");
      await service.createAgent("app-1", "comp-id", "agent", [], undefined);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["new", "comp/agent()"],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with constructor params", async () => {
      mockComponentThenResult("comp");
      await service.createAgent(
        "app-1",
        "comp-id",
        "agent",
        [
          {
            name: "param1",
            schema: {
              type: "ComponentModel" as const,
              elementType: { type: "Str" },
            },
          },
        ],
        [{ type: "Str" }],
      );
      // convertToWaveFormat for a string param returns quoted string
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: expect.arrayContaining(["new"]),
        folderPath: "/test/app",
      });
      // Verify the agent name includes parentheses with params
      const call = mockedInvoke.mock.calls.find(
        c =>
          (c[1] as { command: string }).command === "agent" &&
          (c[1] as { subcommands: string[] }).subcommands[0] === "new",
      );
      expect(call).toBeDefined();
      const subcommands = (call![1] as { subcommands: string[] }).subcommands;
      expect(subcommands[1]).toMatch(/^comp\/agent\(.+\)$/);
    });

    it("sends correct command with env vars", async () => {
      mockComponentThenResult("comp");
      await service.createAgent(
        "app-1",
        "comp-id",
        "agent",
        [],
        undefined,
        undefined,
        {
          KEY: "VAL",
        },
      );
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["new", "comp/agent()", "-e", "KEY=VAL"],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with positional args", async () => {
      mockComponentThenResult("comp");
      await service.createAgent("app-1", "comp-id", "agent", [], undefined, [
        "arg1",
        "arg2",
      ]);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["new", "comp/agent()", "arg1", "arg2"],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with env + args combined", async () => {
      mockComponentThenResult("comp");
      await service.createAgent(
        "app-1",
        "comp-id",
        "agent",
        [],
        undefined,
        ["arg1"],
        { MY_VAR: "123" },
      );
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["new", "comp/agent()", "-e", "MY_VAR=123", "arg1"],
        folderPath: "/test/app",
      });
    });
  });

  describe("getParticularAgent", () => {
    it("sends correct command", async () => {
      mockComponentThenResult("comp");
      await service.getParticularAgent("app-1", "comp-id", "agent");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["get", "comp/agent"],
        folderPath: "/test/app",
      });
    });
  });

  describe("interruptAgent", () => {
    it("sends correct command", async () => {
      mockComponentThenResult("comp");
      await service.interruptAgent("app-1", "comp-id", "agent");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["interrupt", "comp/agent"],
        folderPath: "/test/app",
      });
    });
  });

  describe("resumeAgent", () => {
    it("sends correct command", async () => {
      mockComponentThenResult("comp");
      await service.resumeAgent("app-1", "comp-id", "agent");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["resume", "comp/agent"],
        folderPath: "/test/app",
      });
    });
  });

  describe("invokeAgentAwait", () => {
    it("sends correct command with params payload", async () => {
      mockComponentThenResult("comp", '{"result":"ok"}');
      await service.invokeAgentAwait(
        "app-1",
        "comp-id",
        "agent",
        "pkg:iface/ns.{fn}",
        { params: [{ value: "hello", typ: { type: "Str" } }] },
      );
      // Should call agent invoke with the function name and wave args
      const agentCall = mockedInvoke.mock.calls.find(
        c =>
          (c[1] as { command: string }).command === "agent" &&
          (c[1] as { subcommands: string[] }).subcommands[0] === "invoke",
      );
      expect(agentCall).toBeDefined();
      const subs = (agentCall![1] as { subcommands: string[] }).subcommands;
      expect(subs[0]).toBe("invoke");
      expect(subs[1]).toBe("comp/agent");
      expect(subs[2]).toBe("pkg:iface/ns.{fn}");
      // wave args follow
      expect(subs.length).toBeGreaterThan(3);
    });

    it("sends correct command with array payload", async () => {
      mockComponentThenResult("comp", '{"result":"ok"}');
      await service.invokeAgentAwait(
        "app-1",
        "comp-id",
        "agent",
        "pkg:iface/ns.{fn}",
        ["hello", 42],
      );
      const agentCall = mockedInvoke.mock.calls.find(
        c =>
          (c[1] as { command: string }).command === "agent" &&
          (c[1] as { subcommands: string[] }).subcommands[0] === "invoke",
      );
      expect(agentCall).toBeDefined();
      const subs = (agentCall![1] as { subcommands: string[] }).subcommands;
      expect(subs[0]).toBe("invoke");
      expect(subs[1]).toBe("comp/agent");
      expect(subs[2]).toBe("pkg:iface/ns.{fn}");
      expect(subs.length).toBeGreaterThan(3);
    });

    it("sends correct command with empty payload", async () => {
      mockComponentThenResult("comp", '{"result":"ok"}');
      await service.invokeAgentAwait(
        "app-1",
        "comp-id",
        "agent",
        "pkg:iface/ns.{fn}",
        undefined,
      );
      const agentCall = mockedInvoke.mock.calls.find(
        c =>
          (c[1] as { command: string }).command === "agent" &&
          (c[1] as { subcommands: string[] }).subcommands[0] === "invoke",
      );
      expect(agentCall).toBeDefined();
      const subs = (agentCall![1] as { subcommands: string[] }).subcommands;
      expect(subs).toEqual(["invoke", "comp/agent", "pkg:iface/ns.{fn}"]);
    });
  });

  describe("invokeEphemeralAwait", () => {
    // Mock crypto.randomUUID
    const mockUUID = "test-uuid-1234";
    beforeEach(() => {
      vi.stubGlobal("crypto", { randomUUID: () => mockUUID });
    });

    it("matches PascalCase constructor name against kebab-case agent type", async () => {
      // CLI returns PascalCase constructor names (e.g. "HumanAgent")
      // but function names use kebab-case (e.g. "human-agent")
      mockedInvoke
        .mockResolvedValueOnce(
          JSON.stringify([{ componentId: "comp-id", componentName: "comp" }]),
        )
        .mockResolvedValueOnce(
          JSON.stringify([
            {
              implementedBy: { componentId: "comp-id" },
              agentType: {
                constructor: {
                  name: "HumanAgent",
                  inputSchema: { elements: [{ type: "Str" }] },
                },
              },
            },
          ]),
        )
        .mockResolvedValueOnce('{"result":"ok"}');

      await service.invokeEphemeralAwait(
        "app-1",
        "comp-id",
        "pack:ts/human-agent.{request-approval}",
        undefined,
      );

      const agentCall = mockedInvoke.mock.calls.find(
        c =>
          (c[1] as { command: string }).command === "agent" &&
          (c[1] as { subcommands: string[] }).subcommands[0] === "invoke",
      );
      expect(agentCall).toBeDefined();
      const subs = (agentCall![1] as { subcommands: string[] }).subcommands;
      expect(subs[1]).toBe(`comp/human-agent("desktop-app-${mockUUID}")`);
    });

    it("sends correct command without constructor params", async () => {
      mockedInvoke
        .mockResolvedValueOnce(
          JSON.stringify([{ componentId: "comp-id", componentName: "comp" }]),
        )
        .mockResolvedValueOnce(
          JSON.stringify([
            {
              implementedBy: { componentId: "comp-id" },
              agentType: {
                constructor: {
                  name: "SimpleAgent",
                  inputSchema: { elements: [] },
                },
              },
            },
          ]),
        )
        .mockResolvedValueOnce('{"result":"ok"}');

      await service.invokeEphemeralAwait(
        "app-1",
        "comp-id",
        "pack:ts/simple-agent.{fn}",
        undefined,
      );

      const agentCall = mockedInvoke.mock.calls.find(
        c =>
          (c[1] as { command: string }).command === "agent" &&
          (c[1] as { subcommands: string[] }).subcommands[0] === "invoke",
      );
      expect(agentCall).toBeDefined();
      const subs = (agentCall![1] as { subcommands: string[] }).subcommands;
      expect(subs[1]).toBe("comp/simple-agent()");
    });
  });

  describe("getOplog", () => {
    it("sends correct command", async () => {
      mockComponentThenResult("comp");
      await service.getOplog("app-1", "comp-id", "agent", "search");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "agent",
        subcommands: ["oplog", "comp/agent", "--query=search"],
        folderPath: "/test/app",
      });
    });
  });

  describe("getAgentTypesForComponent", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce(
        JSON.stringify([
          {
            implementedBy: { componentId: "comp-id" },
            agentType: {
              constructor: { name: "test", inputSchema: { elements: [] } },
            },
          },
        ]),
      );
      await service.getAgentTypesForComponent("app-1", "comp-id");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "list-agent-types",
        subcommands: [],
        folderPath: "/test/app",
      });
    });
  });
});
