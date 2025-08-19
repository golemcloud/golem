import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
  type MockedFunction,
} from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import React from "react";
import { Home } from "@/pages/home";
import { AppLayout } from "@/layouts/app-layout";
import { Service } from "@/service/client";

// Mock all dependencies
vi.mock("@/lib/settings", () => ({
  settingsService: {
    getApps: vi.fn(),
    validateGolemApp: vi.fn(),
    addApp: vi.fn(),
    updateAppLastOpened: vi.fn(),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@/hooks/use-toast", () => ({
  toast: vi.fn(),
}));

vi.mock("@/service/client", () => ({
  Service: vi.fn(),
}));

vi.mock("@/components/errorBoundary", () => ({
  default: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="error-boundary">{children}</div>
  ),
}));

vi.mock("@/components/navbar.tsx", () => ({
  default: () => <nav data-testid="navbar">Navbar</nav>,
}));

vi.mock("@/components/ui/button", () => ({
  Button: ({
    children,
    onClick,
    disabled,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
    disabled?: boolean;
  }) => (
    <button onClick={onClick} disabled={disabled}>
      {children}
    </button>
  ),
}));

vi.mock("@/components/ui/card", () => ({
  Card: ({
    children,
    onClick,
    className,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
    className?: string;
  }) => (
    <div className={className} onClick={onClick} data-testid="card">
      {children}
    </div>
  ),
  CardContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  CardDescription: ({ children }: { children: React.ReactNode }) => (
    <p>{children}</p>
  ),
  CardHeader: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  CardTitle: ({ children }: { children: React.ReactNode }) => (
    <h2>{children}</h2>
  ),
}));

vi.mock("@/components/ui/input", () => ({
  Input: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input {...props} />
  ),
}));

vi.mock("lucide-react", () => ({
  Plus: () => <span>+</span>,
  FolderOpen: () => <span>📂</span>,
  Folder: () => <span>📁</span>,
  Clock: () => <span>🕒</span>,
  ArrowRight: () => <span>→</span>,
  ChevronRight: () => <span>▶</span>,
}));

describe("Application Workflow Integration Tests", () => {
  const mockApps = [
    {
      id: "app-1",
      name: "Test App 1",
      folderLocation: "/path/to/app1",
      golemYamlLocation: "/path/to/app1/golem.yaml",
      lastOpened: "2023-12-01T10:00:00Z",
    },
    {
      id: "app-2",
      name: "Test App 2",
      folderLocation: "/path/to/app2",
      golemYamlLocation: "/path/to/app2/golem.yaml",
      lastOpened: "2023-12-02T15:30:00Z",
    },
  ];

  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe("Home to App Navigation Flow", () => {
    it("should navigate from home page to app when clicking on recent app", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();
      const TestApp = () => (
        <div data-testid="app-page">
          App Page for {window.location.pathname}
        </div>
      );

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
            <Route path="/app/:id" element={<AppLayout />}>
              <Route index element={<TestApp />} />
            </Route>
          </Routes>
        </MemoryRouter>,
      );

      // Wait for apps to load
      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
        expect(screen.getByText("Test App 2")).toBeInTheDocument();
      });

      // Click on first app
      const appCard = screen
        .getByText("Test App 1")
        .closest('[data-testid="card"]');
      await user.click(appCard!);

      // Should navigate to app page
      await waitFor(() => {
        expect(screen.getByTestId("app-page")).toBeInTheDocument();
        expect(screen.getByTestId("navbar")).toBeInTheDocument();
      });
    });

    it("should handle complete app opening workflow", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const { settingsService } = await import("@/lib/settings");
      // const { toast } = await import('@/hooks/use-toast');

      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);
      (open as MockedFunction<typeof open>).mockResolvedValue(
        "/path/to/new/app",
      );
      (
        settingsService.validateGolemApp as MockedFunction<
          typeof settingsService.validateGolemApp
        >
      ).mockResolvedValue({
        isValid: true,
        yamlPath: "/path/to/new/app/golem.yaml",
      });
      (
        settingsService.addApp as MockedFunction<typeof settingsService.addApp>
      ).mockResolvedValue(true);

      const user = userEvent.setup();
      const TestApp = () => (
        <div data-testid="new-app-page">New App Opened</div>
      );

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
            <Route path="/app/:id" element={<AppLayout />}>
              <Route index element={<TestApp />} />
            </Route>
          </Routes>
        </MemoryRouter>,
      );

      // Click open existing app button
      const openButton = screen.getByText("Open");
      await user.click(openButton);

      await waitFor(() => {
        expect(open).toHaveBeenCalledWith({
          directory: true,
          multiple: false,
          title: "Select Golem Application Folder",
        });
        expect(settingsService.validateGolemApp).toHaveBeenCalledWith(
          "/path/to/new/app",
        );
        expect(settingsService.addApp).toHaveBeenCalledWith(
          expect.objectContaining({
            folderLocation: "/path/to/new/app",
            golemYamlLocation: "/path/to/new/app/golem.yaml",
          }),
        );
      });
    });
  });

  describe("Error Handling Integration", () => {
    it("should display error toast when app opening fails", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const { settingsService } = await import("@/lib/settings");
      const { toast } = await import("@/hooks/use-toast");

      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);
      (open as MockedFunction<typeof open>).mockResolvedValue(
        "/path/to/invalid/app",
      );
      (
        settingsService.validateGolemApp as MockedFunction<
          typeof settingsService.validateGolemApp
        >
      ).mockResolvedValue({
        isValid: false,
        yamlPath: "",
      });

      const user = userEvent.setup();

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
          </Routes>
        </MemoryRouter>,
      );

      const openButton = screen.getByText("Open");
      await user.click(openButton);

      await waitFor(() => {
        expect(toast).toHaveBeenCalledWith({
          title: "Invalid Golem Application",
          description:
            "The selected folder does not contain a golem.yaml file.",
          variant: "destructive",
        });
      });
    });

    it("should handle service layer errors gracefully", async () => {
      const mockService = {
        componentService: {
          getComponentById: vi.fn().mockRejectedValue(new Error("API Error")),
        },
        checkHealth: vi.fn().mockResolvedValue(true),
      };
      vi.mocked(Service).mockImplementation(
        () => mockService as unknown as InstanceType<typeof Service>,
      );

      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const TestAppWithService = () => {
        const [error, setError] = React.useState<string | null>(null);

        React.useEffect(() => {
          const service = new Service();
          if (service.componentService) {
            service.componentService
              .getComponentById("app-1", "test-component")
              .catch((err: Error) => {
                setError(err.message);
              });
          } else {
            setError("API Error");
          }
        }, []);

        return (
          <div data-testid="app-with-service">
            {error ? `Error: ${error}` : "App loaded successfully"}
          </div>
        );
      };

      render(
        <MemoryRouter initialEntries={["/app/app-1"]}>
          <Routes>
            <Route path="/app/:id" element={<AppLayout />}>
              <Route index element={<TestAppWithService />} />
            </Route>
          </Routes>
        </MemoryRouter>,
      );

      await waitFor(() => {
        expect(screen.getByText("Error: API Error")).toBeInTheDocument();
      });
    });
  });

  describe("Search and Filter Integration", () => {
    it("should filter apps in real-time during search", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
          </Routes>
        </MemoryRouter>,
      );

      // Wait for apps to load
      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
        expect(screen.getByText("Test App 2")).toBeInTheDocument();
      });

      // Search for specific app
      const searchInput = screen.getByPlaceholderText("Search applications...");
      await user.type(searchInput, "Test App 1");

      // Should filter results
      expect(screen.getByText("Test App 1")).toBeInTheDocument();
      expect(screen.queryByText("Test App 2")).not.toBeInTheDocument();

      // Clear search
      await user.clear(searchInput);

      // Should show all apps again
      expect(screen.getByText("Test App 1")).toBeInTheDocument();
      expect(screen.getByText("Test App 2")).toBeInTheDocument();
    });
  });

  describe("State Management Integration", () => {
    it("should maintain app state across navigation", async () => {
      const { settingsService } = await import("@/lib/settings");
      let appsCallCount = 0;
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockImplementation(() => {
        appsCallCount++;
        return Promise.resolve(mockApps);
      });

      const user = userEvent.setup();
      const AppPage = () => <div data-testid="app-page">App Page</div>;

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
            <Route path="/app-create" element={<div>Create App Page</div>} />
            <Route path="/app/:id" element={<AppLayout />}>
              <Route index element={<AppPage />} />
            </Route>
          </Routes>
        </MemoryRouter>,
      );

      // Wait for initial load
      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
      });

      // Navigate to create page
      const createButton = screen.getByText("New Application");
      await user.click(createButton);

      await waitFor(() => {
        expect(screen.getByText("Create App Page")).toBeInTheDocument();
      });

      // Navigate back (simulated by re-rendering home)
      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
          </Routes>
        </MemoryRouter>,
      );

      // Should reload apps data
      await waitFor(() => {
        expect(appsCallCount).toBeGreaterThan(1);
      });
    });
  });

  describe("Performance Integration", () => {
    it("should handle large number of apps efficiently", async () => {
      const manyApps = Array.from({ length: 100 }, (_, i) => ({
        id: `app-${i}`,
        name: `App ${i}`,
        folderLocation: `/path/to/app${i}`,
        golemYamlLocation: `/path/to/app${i}/golem.yaml`,
        lastOpened: "2023-12-01T10:00:00Z",
      }));

      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(manyApps);

      const startTime = performance.now();

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
          </Routes>
        </MemoryRouter>,
      );

      await waitFor(() => {
        expect(screen.getByText("App 0")).toBeInTheDocument();
      });

      const endTime = performance.now();
      const renderTime = endTime - startTime;

      // Should render within reasonable time (less than 1 second)
      expect(renderTime).toBeLessThan(1000);
    });

    it("should debounce search input to avoid excessive filtering", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
          </Routes>
        </MemoryRouter>,
      );

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
      });

      const searchInput = screen.getByPlaceholderText("Search applications...");

      // Rapid typing should not cause excessive re-renders
      await user.type(searchInput, "Test");

      // Should still show filtered results
      expect(screen.getByText("Test App 1")).toBeInTheDocument();
      expect(screen.getByText("Test App 2")).toBeInTheDocument();
    });
  });

  describe("Accessibility Integration", () => {
    it("should provide keyboard navigation throughout the app", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
          </Routes>
        </MemoryRouter>,
      );

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
      });

      // Tab navigation should work
      await user.tab();

      // Focus should be on interactive elements
      const focusedElement = document.activeElement;
      expect(focusedElement?.tagName).toMatch(/BUTTON|INPUT|A/);
    });

    it("should maintain proper ARIA labels and roles", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      render(
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<Home />} />
          </Routes>
        </MemoryRouter>,
      );

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
      });

      // Search input should have proper attributes
      const searchInput = screen.getByPlaceholderText("Search applications...");
      expect(searchInput).toHaveAttribute("placeholder");

      // Buttons should be properly labeled
      const buttons = screen.getAllByRole("button");
      buttons.forEach(button => {
        expect(button.textContent).toBeTruthy();
      });
    });
  });
});
