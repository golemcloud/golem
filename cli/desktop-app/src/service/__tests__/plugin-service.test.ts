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
  writeTextFile: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@tauri-apps/api/path", () => ({
  join: vi.fn((...args: string[]) => Promise.resolve(args.join("/"))),
}));
vi.mock("yaml", () => ({
  stringify: vi.fn().mockReturnValue("name: test-plugin\n"),
}));

import { CLIService } from "../client/cli-service";
import { PluginService } from "../client/plugin-service";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;

describe("PluginService CLI commands", () => {
  let service: PluginService;

  beforeEach(() => {
    vi.clearAllMocks();
    mockedInvoke.mockResolvedValue("[]");

    const cliService = new CLIService();
    service = new PluginService(cliService);
  });

  describe("getPlugins", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("[]");
      await service.getPlugins("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "plugin",
        subcommands: ["list"],
        folderPath: "/test/app",
      });
    });
  });

  describe("getPluginByName", () => {
    it("calls plugin list", async () => {
      mockedInvoke.mockResolvedValueOnce(
        JSON.stringify([{ name: "my-plugin", version: "1.0.0" }]),
      );
      await service.getPluginByName("app-1", "my-plugin");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "plugin",
        subcommands: ["list"],
        folderPath: "/test/app",
      });
    });
  });

  describe("registerPlugin", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.registerPlugin("app-1", "/path/to/manifest");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "plugin",
        subcommands: ["register", "/path/to/manifest"],
        folderPath: "/test/app",
      });
    });
  });

  describe("deletePlugin", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.deletePlugin("app-1", "my-plugin", "1.0.0");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "plugin",
        subcommands: ["unregister", "my-plugin", "1.0.0"],
        folderPath: "/test/app",
      });
    });
  });

  describe("createPlugin", () => {
    it("writes yaml file and calls register", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.createPlugin("app-1", {
        name: "test-plugin",
        version: "1.0.0",
        description: "A test plugin",
        icon: "plugin.wasm",
        homepage: "https://example.com",
        specs: { type: "App" as const },
      });
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "plugin",
        subcommands: ["register", "/test/app/test-plugin.yaml"],
        folderPath: "/test/app",
      });
    });
  });
});
