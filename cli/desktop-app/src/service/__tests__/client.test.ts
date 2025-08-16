import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { Service } from "../client";
import { toast as _toast } from "@/hooks/use-toast";
import { ComponentType as _ComponentType } from "@/types/component";

// Mock dependencies
vi.mock("@/lib/settings", () => ({
  settingsService: {
    getAppById: vi.fn(),
  },
}));

vi.mock("@/hooks/use-toast", () => ({
  toast: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-fs", () => ({
  exists: vi.fn(),
  readDir: vi.fn(),
  readTextFile: vi.fn(),
  writeTextFile: vi.fn(),
}));

vi.mock("@tauri-apps/api/path", () => ({
  join: vi.fn(),
}));

vi.mock("yaml", () => ({
  parse: vi.fn(),
  parseDocument: vi.fn(),
  stringify: vi.fn(),
}));

describe("Service", () => {
  let service: Service;

  beforeEach(() => {
    service = new Service();
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("constructor", () => {
    it("should initialize all services", () => {
      expect(service.cliService).toBeDefined();
      expect(service.componentService).toBeDefined();
      expect(service.workerService).toBeDefined();
      expect(service.apiService).toBeDefined();
      expect(service.pluginService).toBeDefined();
      expect(service.deploymentService).toBeDefined();
      expect(service.appService).toBeDefined();
      expect(service.manifestService).toBeDefined();
    });
  });
});
