import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Mock Tauri modules
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(vi.fn())),
  TauriEvent: { WINDOW_THEME_CHANGED: "window-theme-changed" },
}));

vi.mock("@tauri-apps/plugin-fs", () => ({
  writeFile: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-http", () => ({
  fetch: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-websocket", () => ({
  default: {
    connect: vi.fn(() =>
      Promise.resolve({
        send: vi.fn(),
        disconnect: vi.fn(),
        addListener: vi.fn(),
      }),
    ),
  },
}));

// Mock the BaseDirectory enum
vi.mock("@tauri-apps/api/path", () => ({
  BaseDirectory: {
    Download: 7, // This matches the actual enum value
  },
}));

describe("tauri&web utilities", () => {
  let mockMatchMedia: ReturnType<typeof vi.fn>;
  let mockGlobalFetch: ReturnType<typeof vi.fn>;
  let mockWebSocket: ReturnType<typeof vi.fn>;
  let mockCreateElement: ReturnType<typeof vi.fn>;
  let mockAppendChild: ReturnType<typeof vi.fn>;
  let mockRemoveChild: ReturnType<typeof vi.fn>;
  let mockCreateObjectURL: ReturnType<typeof vi.fn>;
  let mockRevokeObjectURL: ReturnType<typeof vi.fn>;
  let originalWindow: typeof globalThis.window | undefined;
  let originalDocument: typeof globalThis.document | undefined;
  let originalURL: typeof globalThis.URL | undefined;

  beforeEach(() => {
    vi.clearAllMocks();

    // Store originals
    originalWindow = global.window;
    originalDocument = global.document;
    originalURL = global.URL;

    // Set up mocks
    mockMatchMedia = vi.fn(() => ({
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    }));
    mockGlobalFetch = vi.fn();
    mockWebSocket = vi.fn(() => ({ send: vi.fn(), close: vi.fn() }));

    // Mock DOM methods
    mockCreateElement = vi.fn(() => ({
      href: "",
      download: "",
      click: vi.fn(),
    }));
    mockAppendChild = vi.fn();
    mockRemoveChild = vi.fn();
    mockCreateObjectURL = vi.fn(() => "blob:mock-url");
    mockRevokeObjectURL = vi.fn();

    // Set up global mocks
    global.fetch = mockGlobalFetch;
    global.WebSocket = mockWebSocket as unknown as typeof WebSocket;

    // Mock document
    Object.defineProperty(global, "document", {
      value: {
        createElement: mockCreateElement,
        body: {
          appendChild: mockAppendChild,
          removeChild: mockRemoveChild,
        },
      },
      writable: true,
      configurable: true,
    });

    // Mock URL
    Object.defineProperty(global, "URL", {
      value: {
        createObjectURL: mockCreateObjectURL,
        revokeObjectURL: mockRevokeObjectURL,
      },
      writable: true,
      configurable: true,
    });
  });

  afterEach(() => {
    // Restore originals
    Object.defineProperty(global, "window", {
      value: originalWindow,
      writable: true,
      configurable: true,
    });
    Object.defineProperty(global, "document", {
      value: originalDocument,
      writable: true,
      configurable: true,
    });
    Object.defineProperty(global, "URL", {
      value: originalURL,
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

  describe("saveFile", () => {
    it("uses Tauri writeFile when __TAURI__ is available", async () => {
      // Set up Tauri environment
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia, __TAURI__: true },
        writable: true,
        configurable: true,
      });

      // Re-import the module
      vi.resetModules();
      const { saveFile } = await import("../tauri&web");
      const { writeFile } = await import("@tauri-apps/plugin-fs");
      const mockWriteFile = vi.mocked(writeFile);

      const data = new Uint8Array([1, 2, 3]);
      await saveFile("test.txt", data);

      expect(mockWriteFile).toHaveBeenCalledWith("test.txt", data, {
        baseDir: 7,
      });
    });

    it("uses blob download when __TAURI__ is not available", async () => {
      // Set up browser environment (no __TAURI__)
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia },
        writable: true,
        configurable: true,
      });

      // Clear module cache and re-import
      vi.resetModules();
      const { saveFile } = await import("../tauri&web");

      const data = new Uint8Array([1, 2, 3]);
      await saveFile("test.txt", data);

      expect(mockCreateElement).toHaveBeenCalledWith("a");
      expect(mockCreateObjectURL).toHaveBeenCalled();
      expect(mockAppendChild).toHaveBeenCalled();
      expect(mockRemoveChild).toHaveBeenCalled();
    });
  });

  describe("fetchData", () => {
    it("uses Tauri fetch when __TAURI__ is available", async () => {
      // Set up Tauri environment
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia, __TAURI__: true },
        writable: true,
        configurable: true,
      });

      // Re-import the module
      vi.resetModules();
      const { fetchData } = await import("../tauri&web");
      const { fetch: tauriFetch } = await import("@tauri-apps/plugin-http");
      const mockTauriFetch = vi.mocked(tauriFetch);
      mockTauriFetch.mockResolvedValue({ ok: true } as Response);

      await fetchData("http://test.com");

      expect(mockTauriFetch).toHaveBeenCalledWith("http://test.com", undefined);
    });

    it("falls back to global fetch when __TAURI__ is not available", async () => {
      // Set up browser environment (no __TAURI__)
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia },
        writable: true,
        configurable: true,
      });

      // Clear module cache and re-import
      vi.resetModules();
      const { fetchData } = await import("../tauri&web");

      mockGlobalFetch.mockResolvedValue({ ok: true });
      await fetchData("http://test.com");

      expect(mockGlobalFetch).toHaveBeenCalledWith(
        "http://test.com",
        undefined,
      );
    });
  });

  describe("UniversalWebSocket", () => {
    it("creates Tauri WebSocket when __TAURI__ is available", async () => {
      // Set up Tauri environment
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia, __TAURI__: true },
        writable: true,
        configurable: true,
      });

      // Re-import the module
      vi.resetModules();
      const { UniversalWebSocket } = await import("../tauri&web");
      const TauriWebSocket = (await import("@tauri-apps/plugin-websocket"))
        .default;
      const mockConnect = vi.mocked(TauriWebSocket.connect);

      await UniversalWebSocket.connect("ws://test");

      expect(mockConnect).toHaveBeenCalledWith("ws://test");
    });

    it("creates browser WebSocket when __TAURI__ is not available", async () => {
      // Set up browser environment (no __TAURI__)
      Object.defineProperty(global, "window", {
        value: { matchMedia: mockMatchMedia },
        writable: true,
        configurable: true,
      });

      // Clear module cache and re-import
      vi.resetModules();
      const { UniversalWebSocket } = await import("../tauri&web");

      await UniversalWebSocket.connect("ws://test");

      expect(mockWebSocket).toHaveBeenCalledWith("ws://test");
    });
  });
});
