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
  readTextFile: vi.fn().mockResolvedValue("httpApi:\n  definitions: {}"),
  writeTextFile: vi.fn().mockResolvedValue(undefined),
  readDir: vi.fn().mockResolvedValue([]),
  exists: vi.fn().mockResolvedValue(true),
}));
vi.mock("@tauri-apps/api/path", () => ({
  join: vi.fn((...args: string[]) => Promise.resolve(args.join("/"))),
}));

import { CLIService } from "../client/cli-service";
import { ComponentService } from "../client/component-service";
import { ManifestService } from "../client/manifest-service";
import { APIService } from "../client/api-service";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;

describe("APIService CLI commands", () => {
  let service: APIService;

  beforeEach(() => {
    vi.clearAllMocks();
    mockedInvoke.mockResolvedValue("[]");

    const cliService = new CLIService();
    const componentService = new ComponentService(cliService);
    const manifestService = new ManifestService(cliService);
    service = new APIService(cliService, componentService, manifestService);
  });

  describe("getUploadedDefinitions", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("[]");
      await service.getUploadedDefinitions("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "api",
        subcommands: ["definition", "list"],
        folderPath: "/test/app",
      });
    });
  });

  describe("deployDefinition", () => {
    it("sends correct deploy command", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.deployDefinition("app-1", "def-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "deploy",
        subcommands: [],
        folderPath: "/test/app",
      });
    });
  });
});
