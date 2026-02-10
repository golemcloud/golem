import "@testing-library/jest-dom";
import { vi } from "vitest";

// Mock window.matchMedia
Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: vi.fn().mockImplementation(query => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
});

// Mock Tauri API
(globalThis as unknown as { __TAURI__: unknown }).__TAURI__ = {
  invoke: vi.fn(),
  convertFileSrc: vi.fn(),
};

// Mock window.__TAURI_METADATA__
Object.defineProperty(window, "__TAURI_METADATA__", {
  value: {},
  writable: true,
});

// Mock tauri plugins
vi.mock("@tauri-apps/api", () => ({
  invoke: vi.fn(),
  convertFileSrc: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  transformCallback: vi.fn(callback => callback),
  addPluginListener: vi.fn(),
  removePluginListener: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
  emit: vi.fn(),
  unlisten: vi.fn(),
  TauriEvent: {
    WINDOW_THEME_CHANGED: "window_theme_changed",
    WINDOW_RESIZED: "window_resized",
    WINDOW_MOVED: "window_moved",
    WINDOW_FOCUS: "window_focus",
    WINDOW_BLUR: "window_blur",
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-fs", () => ({
  readTextFile: vi.fn(),
  writeTextFile: vi.fn(),
  writeFile: vi.fn(),
  exists: vi.fn().mockResolvedValue(false),
  readDir: vi.fn().mockResolvedValue([]),
}));

vi.mock("@tauri-apps/plugin-store", () => {
  class Store {
    private data: Map<string, unknown> = new Map();
    async get(key: string): Promise<unknown> {
      return this.data.get(key) ?? null;
    }
    async set(key: string, value: unknown): Promise<void> {
      this.data.set(key, value);
    }
    async save(): Promise<void> {}
    async delete(key: string): Promise<boolean> {
      return this.data.delete(key);
    }
    async clear(): Promise<void> {
      this.data.clear();
    }
    async keys(): Promise<string[]> {
      return [...this.data.keys()];
    }
    async values(): Promise<unknown[]> {
      return [...this.data.values()];
    }
    async entries(): Promise<[string, unknown][]> {
      return [...this.data.entries()];
    }
    async length(): Promise<number> {
      return this.data.size;
    }
    async has(key: string): Promise<boolean> {
      return this.data.has(key);
    }
  }
  return {
    Store,
    load: vi.fn().mockImplementation(() => Promise.resolve(new Store())),
  };
});
