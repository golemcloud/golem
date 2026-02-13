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
    getAppById: vi.fn(),
  },
}));
vi.mock("@/hooks/use-toast", () => ({
  toast: vi.fn(),
}));

import { CLIService } from "../client/cli-service";
import { settingsService } from "@/lib/settings";
import { toast } from "@/hooks/use-toast";

const mockedInvoke = invoke as MockedFunction<typeof invoke>;
const mockedGetAppById = settingsService.getAppById as MockedFunction<
  typeof settingsService.getAppById
>;

describe("CLIService CLI commands", () => {
  let service: CLIService;

  beforeEach(() => {
    vi.clearAllMocks();
    service = new CLIService();
    mockedGetAppById.mockResolvedValue({
      id: "app-1",
      name: "test",
      folderLocation: "/test/app",
      golemYamlLocation: "/test/app/golem.yaml",
      lastOpened: "2023-12-01T10:00:00Z",
    });
  });

  describe("callCLI", () => {
    it("passes command and subcommands to invoke", async () => {
      mockedInvoke.mockResolvedValue("true");
      await service.callCLI("app-1", "test", ["a", "b"]);
      expect(invoke).toHaveBeenCalledWith("call_golem_command", {
        command: "test",
        subcommands: ["a", "b"],
        folderPath: "/test/app",
      });
    });

    it("throws when app not found", async () => {
      mockedGetAppById.mockResolvedValue(undefined);
      await expect(service.callCLI("app-1", "test", [])).rejects.toThrow(
        "App not found",
      );
    });

    it("parses JSON object from result", async () => {
      mockedInvoke.mockResolvedValue('{"id":1}');
      const result = await service.callCLI("app-1", "test", []);
      expect(result).toEqual({ id: 1 });
    });

    it("parses JSON array from result", async () => {
      mockedInvoke.mockResolvedValue('[{"id":1}]');
      const result = await service.callCLI("app-1", "test", []);
      expect(result).toEqual([{ id: 1 }]);
    });

    it("returns true for non-JSON result", async () => {
      mockedInvoke.mockResolvedValue("success");
      const result = await service.callCLI("app-1", "test", []);
      expect(result).toBe(true);
    });

    it("shows toast and throws on invoke error", async () => {
      mockedInvoke.mockRejectedValue("CLI failed");
      await expect(service.callCLI("app-1", "test", [])).rejects.toThrow(
        "Error from golem CLI",
      );
      expect(toast).toHaveBeenCalledWith(
        expect.objectContaining({
          title: "Error from golem CLI",
          variant: "destructive",
        }),
      );
    });
  });

  describe("callCLIRaw", () => {
    it("returns raw string from invoke", async () => {
      mockedInvoke.mockResolvedValue("raw output");
      const result = await service.callCLIRaw("app-1", "test", []);
      expect(result).toBe("raw output");
    });

    it("throws when app not found", async () => {
      mockedGetAppById.mockResolvedValue(undefined);
      await expect(service.callCLIRaw("app-1", "test", [])).rejects.toThrow(
        "App not found",
      );
    });
  });

  describe("callCLIWithLogs", () => {
    it("returns result with success flag on success", async () => {
      mockedInvoke.mockResolvedValue('{"data":"ok"}');
      const result = await service.callCLIWithLogs("app-1", "test", []);
      expect(result).toEqual({
        result: { data: "ok" },
        logs: '{"data":"ok"}',
        success: true,
      });
    });

    it("returns error with success=false on failure", async () => {
      mockedInvoke.mockRejectedValue("CLI error");
      const result = await service.callCLIWithLogs("app-1", "test", []);
      expect(result).toEqual({
        result: true,
        logs: "CLI error",
        success: false,
      });
    });
  });
});
