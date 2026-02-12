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
  readTextFile: vi.fn().mockResolvedValue("httpApi:\n  deployments: {}"),
  writeTextFile: vi.fn().mockResolvedValue(undefined),
  readDir: vi.fn().mockResolvedValue([]),
  exists: vi.fn().mockResolvedValue(true),
}));
vi.mock("@tauri-apps/api/path", () => ({
  join: vi.fn((...args: string[]) => Promise.resolve(args.join("/"))),
}));

import { CLIService } from "../client/cli-service";
import { ManifestService } from "../client/manifest-service";
import { DeploymentService } from "../client/deployment-service";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;

describe("DeploymentService CLI commands", () => {
  let service: DeploymentService;

  beforeEach(() => {
    vi.clearAllMocks();
    mockedInvoke.mockResolvedValue("[]");

    const cliService = new CLIService();
    const manifestService = new ManifestService(cliService);
    service = new DeploymentService(cliService, manifestService);
  });

  describe("getDeploymentApi", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("[]");
      await service.getDeploymentApi("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "api",
        subcommands: ["deployment", "list"],
        folderPath: "/test/app",
      });
    });
  });

  describe("createDeployment", () => {
    it("sends correct deploy command", async () => {
      // First call: getActiveProfileName → profile get
      mockedInvoke.mockResolvedValueOnce(JSON.stringify({ name: "local" }));
      // Second call: deploy
      mockedInvoke.mockResolvedValueOnce("true");

      // Mock the manifest service methods used internally
      const manifestGetAppYamlPath = vi
        .fn()
        .mockResolvedValue("/test/app/golem.yaml");
      const manifestSaveAppManifest = vi.fn().mockResolvedValue(undefined);
      Object.defineProperty(service, "manifestService", {
        value: {
          getAppYamlPath: manifestGetAppYamlPath,
          saveAppManifest: manifestSaveAppManifest,
        },
        writable: true,
      });

      await service.createDeployment("app-1", "example.com", null, [
        { id: "api-1", version: "0.1.0" },
      ]);

      // The deploy command should have been called
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "deploy",
        subcommands: [],
        folderPath: "/test/app",
      });
    });
  });
});
