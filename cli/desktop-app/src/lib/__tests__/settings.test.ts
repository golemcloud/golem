import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  SettingsService,
  storeService,
  settingsService,
  type App,
} from "../settings";
import { exists } from "@tauri-apps/plugin-fs";

// Mock dependencies
vi.mock("@tauri-apps/plugin-store", () => ({
  load: vi.fn(),
  Store: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-fs", () => ({
  exists: vi.fn(),
}));

interface MockStore {
  get: ReturnType<typeof vi.fn>;
  set: ReturnType<typeof vi.fn>;
  save: ReturnType<typeof vi.fn>;
}

describe("SettingsService", () => {
  let service: SettingsService;
  let mockStore: MockStore;

  beforeEach(async () => {
    vi.clearAllMocks();

    mockStore = {
      get: vi.fn(),
      set: vi.fn(),
      save: vi.fn(),
    };

    const { load } = await import("@tauri-apps/plugin-store");
    (load as ReturnType<typeof vi.fn>).mockResolvedValue(mockStore);

    service = new SettingsService();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("constructor", () => {
    it("should initialize with settings.json as store name", () => {
      expect(service).toBeInstanceOf(SettingsService);
    });
  });

  describe("getGolemCliPath", () => {
    it("should return CLI path when it exists", async () => {
      const expectedPath = "/usr/local/bin/golem";
      mockStore.get.mockResolvedValue(expectedPath);

      const result = await service.getGolemCliPath();

      expect(mockStore.get).toHaveBeenCalledWith("golem_cli_path");
      expect(result).toBe(expectedPath);
    });

    it("should return null when path does not exist", async () => {
      mockStore.get.mockResolvedValue(null);

      const result = await service.getGolemCliPath();

      expect(result).toBeNull();
    });

    it("should return null and log error on exception", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      mockStore.get.mockRejectedValue(new Error("Store error"));

      const result = await service.getGolemCliPath();

      expect(result).toBeNull();
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error getting golem-cli path:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });
  });

  describe("setGolemCliPath", () => {
    it("should save CLI path successfully", async () => {
      const path = "/usr/local/bin/golem";
      mockStore.set.mockResolvedValue(undefined);
      mockStore.save.mockResolvedValue(undefined);

      const result = await service.setGolemCliPath(path);

      expect(mockStore.set).toHaveBeenCalledWith("golem_cli_path", path);
      expect(mockStore.save).toHaveBeenCalled();
      expect(result).toBe(true);
    });

    it("should return false and log error on exception", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      const path = "/usr/local/bin/golem";
      mockStore.set.mockRejectedValue(new Error("Store error"));

      const result = await service.setGolemCliPath(path);

      expect(result).toBe(false);
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error saving golem-cli path:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });
  });

  describe("getApps", () => {
    it("should return valid apps only", async () => {
      const mockApps: App[] = [
        {
          id: "app1",
          name: "App 1",
          folderLocation: "/valid/path1",
          golemYamlLocation: "/valid/path1/golem.yaml",
          lastOpened: "2023-01-01T00:00:00Z",
        },
        {
          id: "app2",
          name: "App 2",
          folderLocation: "/invalid/path2",
          golemYamlLocation: "/invalid/path2/golem.yaml",
          lastOpened: "2023-01-02T00:00:00Z",
        },
        {
          id: "app3",
          name: "App 3",
          folderLocation: "/valid/path3",
          golemYamlLocation: "/valid/path3/golem.yaml",
          lastOpened: "2023-01-03T00:00:00Z",
        },
      ];

      mockStore.get.mockResolvedValue(mockApps);
      (exists as ReturnType<typeof vi.fn>)
        .mockResolvedValueOnce(true) // app1 - valid
        .mockResolvedValueOnce(false) // app2 - invalid
        .mockResolvedValueOnce(true); // app3 - valid

      const result = await service.getApps();

      expect(result).toHaveLength(2);
      expect(result).toEqual([mockApps[0], mockApps[2]]);
      expect(mockStore.set).toHaveBeenCalledWith("apps", [
        mockApps[0],
        mockApps[2],
      ]);
      expect(mockStore.save).toHaveBeenCalled();
    });

    it("should return empty array when no apps exist", async () => {
      mockStore.get.mockResolvedValue(null);

      const result = await service.getApps();

      expect(result).toEqual([]);
    });

    it("should handle non-array apps data", async () => {
      mockStore.get.mockResolvedValue("invalid data");

      const result = await service.getApps();

      expect(result).toEqual([]);
    });

    it("should return empty array and log error on exception", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      mockStore.get.mockRejectedValue(new Error("Store error"));

      const result = await service.getApps();

      expect(result).toEqual([]);
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error getting apps:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });

    it("should not update store when all apps are valid", async () => {
      const mockApps: App[] = [
        {
          id: "app1",
          name: "App 1",
          folderLocation: "/valid/path1",
          golemYamlLocation: "/valid/path1/golem.yaml",
          lastOpened: "2023-01-01T00:00:00Z",
        },
      ];

      mockStore.get.mockResolvedValue(mockApps);
      (exists as ReturnType<typeof vi.fn>).mockResolvedValue(true);

      await service.getApps();

      expect(mockStore.set).not.toHaveBeenCalled();
      expect(mockStore.save).not.toHaveBeenCalled();
    });
  });

  describe("addApp", () => {
    it("should add new app with formatted name", async () => {
      const newApp: App = {
        id: "new-app",
        folderLocation: "/path/to/my-awesome_project",
        golemYamlLocation: "/path/to/my-awesome_project/golem.yaml",
        lastOpened: "2023-01-01T00:00:00Z",
      };

      const existingApps: App[] = [];

      // Mock getApps to return empty array
      vi.spyOn(service, "getApps").mockResolvedValue(existingApps);

      const result = await service.addApp(newApp);

      expect(result).toBe(true);
      expect(newApp.name).toBe("My Awesome Project");
      expect(mockStore.set).toHaveBeenCalledWith("apps", [newApp]);
      expect(mockStore.save).toHaveBeenCalled();
    });

    it("should update existing app with same folder location", async () => {
      const existingApp: App = {
        id: "existing-app",
        name: "Old Name",
        folderLocation: "/path/to/project",
        golemYamlLocation: "/path/to/project/golem.yaml",
        lastOpened: "2023-01-01T00:00:00Z",
      };

      const updatedApp: App = {
        id: "updated-app",
        name: "New Name",
        folderLocation: "/path/to/project",
        golemYamlLocation: "/path/to/project/golem.yaml",
        lastOpened: "2023-01-02T00:00:00Z",
      };

      vi.spyOn(service, "getApps").mockResolvedValue([existingApp]);

      const result = await service.addApp(updatedApp);

      expect(result).toBe(true);
      expect(mockStore.set).toHaveBeenCalledWith("apps", [updatedApp]);
    });

    it("should handle app name generation from folder path", async () => {
      const newApp: App = {
        id: "new-app",
        folderLocation: "/path/to/project",
        golemYamlLocation: "/path/to/project/golem.yaml",
        lastOpened: "2023-01-01T00:00:00Z",
      };

      vi.spyOn(service, "getApps").mockResolvedValue([]);

      await service.addApp(newApp);

      expect(newApp.name).toBe("Project");
    });

    it("should handle empty folder path", async () => {
      const newApp: App = {
        id: "new-app",
        folderLocation: "",
        golemYamlLocation: "/golem.yaml",
        lastOpened: "2023-01-01T00:00:00Z",
      };

      vi.spyOn(service, "getApps").mockResolvedValue([]);

      await service.addApp(newApp);

      expect(newApp.name).toBe("");
    });

    it("should return false and log error on exception", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      const newApp: App = {
        id: "new-app",
        folderLocation: "/path/to/project",
        golemYamlLocation: "/path/to/project/golem.yaml",
        lastOpened: "2023-01-01T00:00:00Z",
      };

      vi.spyOn(service, "getApps").mockRejectedValue(new Error("Store error"));

      const result = await service.addApp(newApp);

      expect(result).toBe(false);
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error saving app:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });
  });

  describe("updateAppLastOpened", () => {
    it("should update app last opened timestamp", async () => {
      const mockApps: App[] = [
        {
          id: "app1",
          name: "App 1",
          folderLocation: "/path1",
          golemYamlLocation: "/path1/golem.yaml",
          lastOpened: "2023-01-01T00:00:00Z",
        },
        {
          id: "app2",
          name: "App 2",
          folderLocation: "/path2",
          golemYamlLocation: "/path2/golem.yaml",
          lastOpened: "2023-01-02T00:00:00Z",
        },
      ];

      vi.spyOn(service, "getApps").mockResolvedValue(mockApps);
      const dateNowSpy = vi
        .spyOn(Date.prototype, "toISOString")
        .mockReturnValue("2023-01-03T00:00:00Z");

      const result = await service.updateAppLastOpened("app1");

      expect(result).toBe(true);
      expect(mockApps[0]?.lastOpened).toBe("2023-01-03T00:00:00Z");
      expect(mockStore.set).toHaveBeenCalledWith("apps", mockApps);
      expect(mockStore.save).toHaveBeenCalled();

      dateNowSpy.mockRestore();
    });

    it("should return false when app not found", async () => {
      vi.spyOn(service, "getApps").mockResolvedValue([]);

      const result = await service.updateAppLastOpened("nonexistent");

      expect(result).toBe(false);
      expect(mockStore.set).not.toHaveBeenCalled();
      expect(mockStore.save).not.toHaveBeenCalled();
    });

    it("should return false and log error on exception", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      vi.spyOn(service, "getApps").mockRejectedValue(new Error("Store error"));

      const result = await service.updateAppLastOpened("app1");

      expect(result).toBe(false);
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error updating app last opened:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });
  });

  describe("getAppById", () => {
    it("should return app when found", async () => {
      const mockApps: App[] = [
        {
          id: "app1",
          name: "App 1",
          folderLocation: "/path1",
          golemYamlLocation: "/path1/golem.yaml",
          lastOpened: "2023-01-01T00:00:00Z",
        },
        {
          id: "app2",
          name: "App 2",
          folderLocation: "/path2",
          golemYamlLocation: "/path2/golem.yaml",
          lastOpened: "2023-01-02T00:00:00Z",
        },
      ];

      vi.spyOn(service, "getApps").mockResolvedValue(mockApps);

      const result = await service.getAppById("app2");

      expect(result).toEqual(mockApps[1]);
    });

    it("should return undefined when app not found", async () => {
      vi.spyOn(service, "getApps").mockResolvedValue([]);

      const result = await service.getAppById("nonexistent");

      expect(result).toBeUndefined();
    });

    it("should return undefined and log error on exception", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      vi.spyOn(service, "getApps").mockRejectedValue(new Error("Store error"));

      const result = await service.getAppById("app1");

      expect(result).toBeUndefined();
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error getting app by ID:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });
  });

  describe("validateGolemApp", () => {
    it("should validate existing golem app", async () => {
      const folderPath = "/path/to/golem/project";
      (exists as ReturnType<typeof vi.fn>).mockResolvedValue(true);

      const result = await service.validateGolemApp(folderPath);

      expect(exists).toHaveBeenCalledWith("/path/to/golem/project/golem.yaml");
      expect(result).toEqual({
        isValid: true,
        yamlPath: "/path/to/golem/project/golem.yaml",
      });
    });

    it("should invalidate non-golem app", async () => {
      const folderPath = "/path/to/regular/project";
      (exists as ReturnType<typeof vi.fn>).mockResolvedValue(false);

      const result = await service.validateGolemApp(folderPath);

      expect(result).toEqual({
        isValid: false,
        yamlPath: "",
      });
    });

    it("should handle validation error", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      const folderPath = "/path/to/project";
      (exists as ReturnType<typeof vi.fn>).mockRejectedValue(
        new Error("File system error"),
      );

      const result = await service.validateGolemApp(folderPath);

      expect(result).toEqual({
        isValid: false,
        yamlPath: "",
      });
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error validating golem app:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });
  });

  describe("exported instances", () => {
    it("should export storeService instance", () => {
      expect(storeService).toBeInstanceOf(SettingsService);
    });

    it("should export settingsService as backward compatibility alias", () => {
      expect(settingsService).toBe(storeService);
    });
  });

  describe("validFolderFilter function", () => {
    it("should filter valid folders during getApps", async () => {
      const mockApps: App[] = [
        {
          id: "app1",
          name: "App 1",
          folderLocation: "/valid/path",
          golemYamlLocation: "/valid/path/golem.yaml",
          lastOpened: "2023-01-01T00:00:00Z",
        },
        {
          id: "app2",
          name: "App 2",
          folderLocation: "/invalid/path",
          golemYamlLocation: "/invalid/path/golem.yaml",
          lastOpened: "2023-01-02T00:00:00Z",
        },
      ];

      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      mockStore.get.mockResolvedValue(mockApps);
      (exists as ReturnType<typeof vi.fn>)
        .mockResolvedValueOnce(true)
        .mockRejectedValueOnce(new Error("File system error"));

      const result = await service.getApps();

      expect(result).toHaveLength(1);
      expect(result[0]).toEqual(mockApps[0]);
      expect(consoleSpy).toHaveBeenCalledWith(
        "Error checking existence of folder /invalid/path:",
        expect.any(Error),
      );

      consoleSpy.mockRestore();
    });
  });
});
