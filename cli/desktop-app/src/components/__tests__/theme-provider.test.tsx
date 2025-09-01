import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ThemeProvider, useTheme } from "../theme-provider";
import { vi } from "vitest";

// Mock the tauri&web module
vi.mock("@/lib/tauri&web.ts", () => ({
  listenThemeChange: vi.fn(() => vi.fn()),
  Theme: {},
}));

// Test component to interact with the theme provider
const TestComponent = () => {
  const { theme, setTheme } = useTheme();

  return (
    <div>
      <div data-testid="current-theme">{theme}</div>
      <button onClick={() => setTheme("light")} data-testid="set-light">
        Set Light
      </button>
      <button onClick={() => setTheme("dark")} data-testid="set-dark">
        Set Dark
      </button>
      <button onClick={() => setTheme("system")} data-testid="set-system">
        Set System
      </button>
    </div>
  );
};

describe("ThemeProvider", () => {
  beforeEach(() => {
    // Clear localStorage before each test
    localStorage.clear();

    // Mock matchMedia
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
  });

  it("renders children correctly", () => {
    render(
      <ThemeProvider>
        <div data-testid="child">Test Child</div>
      </ThemeProvider>,
    );

    expect(screen.getByTestId("child")).toBeInTheDocument();
  });

  it("provides default theme value", () => {
    render(
      <ThemeProvider>
        <TestComponent />
      </ThemeProvider>,
    );

    expect(screen.getByTestId("current-theme")).toHaveTextContent("system");
  });

  it("allows setting custom default theme", () => {
    render(
      <ThemeProvider defaultTheme="dark">
        <TestComponent />
      </ThemeProvider>,
    );

    expect(screen.getByTestId("current-theme")).toHaveTextContent("dark");
  });

  it("persists theme to localStorage", () => {
    render(
      <ThemeProvider>
        <TestComponent />
      </ThemeProvider>,
    );

    fireEvent.click(screen.getByTestId("set-light"));

    expect(localStorage.getItem("vite-ui-theme")).toBe("light");
    expect(screen.getByTestId("current-theme")).toHaveTextContent("light");
  });

  it("loads theme from localStorage on mount", () => {
    localStorage.setItem("vite-ui-theme", "dark");

    render(
      <ThemeProvider>
        <TestComponent />
      </ThemeProvider>,
    );

    expect(screen.getByTestId("current-theme")).toHaveTextContent("dark");
  });

  it("uses custom storage key", () => {
    render(
      <ThemeProvider storageKey="custom-theme">
        <TestComponent />
      </ThemeProvider>,
    );

    fireEvent.click(screen.getByTestId("set-dark"));

    expect(localStorage.getItem("custom-theme")).toBe("dark");
  });

  it("updates document classes when theme changes", async () => {
    const root = document.documentElement;

    render(
      <ThemeProvider>
        <TestComponent />
      </ThemeProvider>,
    );

    fireEvent.click(screen.getByTestId("set-light"));

    await waitFor(() => {
      expect(root.classList.contains("light")).toBe(true);
      expect(root.classList.contains("dark")).toBe(false);
    });

    fireEvent.click(screen.getByTestId("set-dark"));

    await waitFor(() => {
      expect(root.classList.contains("dark")).toBe(true);
      expect(root.classList.contains("light")).toBe(false);
    });
  });

  it("handles system theme detection", async () => {
    // Mock dark theme preference
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: vi.fn().mockImplementation(query => ({
        matches: query.includes("prefers-color-scheme: dark"),
        media: query,
        onchange: null,
        addListener: vi.fn(),
        removeListener: vi.fn(),
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        dispatchEvent: vi.fn(),
      })),
    });

    render(
      <ThemeProvider>
        <TestComponent />
      </ThemeProvider>,
    );

    fireEvent.click(screen.getByTestId("set-system"));

    await waitFor(() => {
      expect(document.documentElement.classList.contains("dark")).toBe(true);
    });
  });
});
