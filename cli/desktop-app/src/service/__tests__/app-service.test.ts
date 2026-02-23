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

import { CLIService } from "../client/cli-service";
import { AppService } from "../client/app-service";
import { ManifestService } from "../client/manifest-service";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;

describe("AppService CLI commands", () => {
  let service: AppService;

  beforeEach(() => {
    vi.clearAllMocks();
    mockedInvoke.mockResolvedValue("true");

    const cliService = new CLIService();
    const manifestService = new ManifestService(cliService);
    vi.spyOn(manifestService, "migrateDeploymentSchema").mockResolvedValue();
    service = new AppService(cliService, manifestService);
  });

  describe("buildApp", () => {
    it("sends correct command with no components", async () => {
      await service.buildApp("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "build",
        subcommands: [],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with component names", async () => {
      await service.buildApp("app-1", ["comp1", "comp2"]);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "build",
        subcommands: ["comp1", "comp2"],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with empty array", async () => {
      await service.buildApp("app-1", []);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "build",
        subcommands: [],
        folderPath: "/test/app",
      });
    });
  });

  describe("updateAgents", () => {
    it("sends correct command with default mode", async () => {
      await service.updateAgents("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "update-agents",
        subcommands: ["--update-mode", "auto"],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with custom mode and components", async () => {
      await service.updateAgents("app-1", ["comp1"], "manual");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "update-agents",
        subcommands: ["--update-mode", "manual", "comp1"],
        folderPath: "/test/app",
      });
    });
  });

  describe("deployAgents", () => {
    it("sends correct command without update flag", async () => {
      await service.deployAgents("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "deploy",
        subcommands: [],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with update flag", async () => {
      await service.deployAgents("app-1", true);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "deploy",
        subcommands: ["--update-agents"],
        folderPath: "/test/app",
      });
    });
  });

  describe("cleanApp", () => {
    it("sends correct command with no components", async () => {
      await service.cleanApp("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "clean",
        subcommands: [],
        folderPath: "/test/app",
      });
    });

    it("sends correct command with components", async () => {
      await service.cleanApp("app-1", ["comp1", "comp2"]);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "clean",
        subcommands: ["comp1", "comp2"],
        folderPath: "/test/app",
      });
    });
  });
});
