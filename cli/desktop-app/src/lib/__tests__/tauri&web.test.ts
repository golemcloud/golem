import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Mock Tauri modules
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(vi.fn())),
  TauriEvent: { WINDOW_THEME_CHANGED: "window-theme-changed" },
}));

describe("tauri&web utilities", () => {
  let mockMatchMedia: ReturnType<typeof vi.fn>;
  let originalWindow: typeof globalThis.window | undefined;

  beforeEach(() => {
    vi.clearAllMocks();

    // Store originals
    originalWindow = global.window;

    // Set up mocks
    mockMatchMedia = vi.fn(() => ({
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }));
  });

  afterEach(() => {
    // Restore originals
    Object.defineProperty(global, "window", {
      value: originalWindow,
      writable: true,
      configurable: true,
    });
  });

  describe("listenThemeChange", () => {
    it("uses Tauri listener when __TAURI__ is available", async () => {
      // Set up Tauri environment
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia, __TAURI__: true },
        writable: true,
        configurable: true,
      });

      // Re-import the module to pick up the new window mock
      const { listenThemeChange } = await import("../tauri&web");
      const { listen } = await import("@tauri-apps/api/event");
      const mockListen = vi.mocked(listen);
      const callback = vi.fn();

      listenThemeChange(callback);

      expect(mockListen).toHaveBeenCalledWith("window-theme-changed", callback);
    });

    it("uses matchMedia when __TAURI__ is not available", async () => {
      // Set up browser environment (no __TAURI__)
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia },
        writable: true,
        configurable: true,
      });

      // Clear module cache and re-import
      vi.resetModules();
      const { listenThemeChange } = await import("../tauri&web");

      const callback = vi.fn();
      listenThemeChange(callback);

      expect(mockMatchMedia).toHaveBeenCalledWith(
        "(prefers-color-scheme: dark)",
      );
    });
  });
});
