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
  readDir: vi.fn().mockResolvedValue([
    {
      name: "components-ts",
      isDirectory: true,
      isFile: false,
      isSymlink: false,
    },
  ]),
}));
vi.mock("@tauri-apps/api/path", () => ({
  join: vi.fn((...args: string[]) => Promise.resolve(args.join("/"))),
}));

import { CLIService } from "../client/cli-service";
import { ComponentService } from "../client/component-service";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;

describe("ComponentService CLI commands", () => {
  let service: ComponentService;

  beforeEach(() => {
    vi.clearAllMocks();
    mockedInvoke.mockResolvedValue("[]");

    const cliService = new CLIService();
    service = new ComponentService(cliService);
  });

  describe("getComponents", () => {
    it("sends correct command", async () => {
      // readDir for hasComponents returns a folder with subfolders
      const { readDir } = await import("@tauri-apps/plugin-fs");
      (readDir as MockedFunction<typeof readDir>)
        .mockResolvedValueOnce([
          {
            name: "components-ts",
            isDirectory: true,
            isFile: false,
            isSymlink: false,
          },
        ])
        .mockResolvedValueOnce([
          {
            name: "my-comp",
            isDirectory: true,
            isFile: false,
            isSymlink: false,
          },
        ]);

      await service.getComponents("app-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "component",
        subcommands: ["list"],
        folderPath: "/test/app",
      });
    });
  });

  describe("getComponentById", () => {
    it("calls component list", async () => {
      mockedInvoke.mockResolvedValueOnce(
        JSON.stringify([{ componentId: "comp-1", componentName: "my-comp" }]),
      );
      const result = await service.getComponentById("app-1", "comp-1");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "component",
        subcommands: ["list"],
        folderPath: "/test/app",
      });
      expect(result.componentName).toBe("my-comp");
    });
  });

  describe("getComponentByIdAndVersion", () => {
    it("calls component list", async () => {
      mockedInvoke.mockResolvedValueOnce(
        JSON.stringify([
          {
            componentId: "comp-1",
            componentName: "my-comp",
            componentRevision: 1,
          },
        ]),
      );
      await service.getComponentByIdAndVersion("app-1", "comp-1", 1);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "component",
        subcommands: ["list"],
        folderPath: "/test/app",
      });
    });
  });

  describe("createComponent", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce("true");
      await service.createComponent("app-1", "pkg:comp", "ts-template");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "component",
        subcommands: ["new", "ts-template", "pkg:comp"],
        folderPath: "/test/app",
      });
    });
  });

  describe("getComponentByName", () => {
    it("sends correct command", async () => {
      mockedInvoke.mockResolvedValueOnce(
        JSON.stringify([{ componentName: "my-comp" }]),
      );
      await service.getComponentByName("app-1", "my-comp");
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "component",
        subcommands: ["get", "my-comp"],
        folderPath: "/test/app",
      });
    });
  });
});
