import type { StorybookConfig } from "@storybook/react-vite";
import path from "path";

const currentDir = import.meta.dirname;

const config: StorybookConfig = {
  stories: ["../src/**/*.stories.@(js|jsx|mjs|ts|tsx)"],
  addons: [
    "@chromatic-com/storybook",
    "@storybook/addon-vitest",
    "@storybook/addon-a11y",
    "@storybook/addon-docs",
  ],
  framework: "@storybook/react-vite",
  viteFinal: async config => {
    const mocksDir = path.resolve(currentDir, "mocks");

    config.resolve = config.resolve ?? {};
    config.resolve.alias = {
      ...(config.resolve.alias ?? {}),
      "@": path.resolve(currentDir, "../src"),
      "@tauri-apps/api/core": path.resolve(mocksDir, "tauri-api-core.ts"),
      "@tauri-apps/api/event": path.resolve(mocksDir, "tauri-api-event.ts"),
      "@tauri-apps/api/path": path.resolve(mocksDir, "tauri-api-path.ts"),
      "@tauri-apps/plugin-dialog": path.resolve(
        mocksDir,
        "tauri-plugin-dialog.ts",
      ),
      "@tauri-apps/plugin-fs": path.resolve(mocksDir, "tauri-plugin-fs.ts"),
      "@tauri-apps/plugin-store": path.resolve(
        mocksDir,
        "tauri-plugin-store.ts",
      ),
      "@tauri-apps/plugin-websocket": path.resolve(
        mocksDir,
        "tauri-plugin-websocket.ts",
      ),
    };

    return config;
  },
};

export default config;
