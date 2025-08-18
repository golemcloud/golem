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
(global as unknown as { __TAURI__: unknown }).__TAURI__ = {
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
}));

vi.mock("@tauri-apps/plugin-store", () => ({
  Store: vi.fn(() => ({
    get: vi.fn(),
    set: vi.fn(),
    save: vi.fn(),
  })),
}));
