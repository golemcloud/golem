import React from "react";
import type { Preview } from "@storybook/react-vite";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { ThemeProvider } from "../src/components/theme-provider";
import { LogViewerProvider } from "../src/contexts/log-viewer-context";
import { Toaster } from "../src/components/ui/toaster";
import "../src/index.css";

const preview: Preview = {
  parameters: {
    controls: {
      matchers: {
        color: /(background|color)$/i,
        date: /Date$/i,
      },
    },
    a11y: {
      test: "todo",
    },
  },
  decorators: [
    (Story, context) => {
      const skipRouter = context.parameters?.skipGlobalRouter === true;
      const routerRoute = context.parameters?.router?.route ?? "/";
      const routerPath = context.parameters?.router?.path ?? "*";

      const content = (
        <ThemeProvider defaultTheme="light" storageKey="storybook-theme">
          <LogViewerProvider>
            <Story />
            <Toaster />
          </LogViewerProvider>
        </ThemeProvider>
      );

      if (skipRouter) {
        return content;
      }

      return (
        <MemoryRouter initialEntries={[routerRoute]}>
          <Routes>
            <Route path={routerPath} element={content} />
          </Routes>
        </MemoryRouter>
      );
    },
  ],
};

export default preview;
