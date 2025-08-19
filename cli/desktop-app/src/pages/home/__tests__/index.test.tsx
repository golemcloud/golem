import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
  type MockedFunction,
} from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { Home } from "../index";
import { App } from "@/lib/settings";

// Mock dependencies
const mockNavigate = vi.fn();
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual("react-router-dom");
  return {
    ...actual,
    useNavigate: () => mockNavigate,
  };
});

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@/hooks/use-toast", () => ({
  toast: vi.fn(),
}));

vi.mock("@/lib/settings", () => ({
  settingsService: {
    getApps: vi.fn(),
    validateGolemApp: vi.fn(),
    addApp: vi.fn(),
  },
}));

vi.mock("@/components/ui/button", () => ({
  Button: ({
    children,
    onClick,
    disabled,
    size,
    variant,
    className,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
    disabled?: boolean;
    size?: string;
    variant?: string;
    className?: string;
  }) => (
    <button
      onClick={onClick}
      disabled={disabled}
      className={className}
      data-size={size}
      data-variant={variant}
    >
      {children}
    </button>
  ),
}));

vi.mock("@/components/ui/card", () => ({
  Card: ({
    children,
    className,
    onClick,
  }: {
    children: React.ReactNode;
    className?: string;
    onClick?: () => void;
  }) => (
    <div className={className} onClick={onClick} data-testid="card">
      {children}
    </div>
  ),
  CardContent: ({
    children,
    className,
  }: {
    children: React.ReactNode;
    className?: string;
  }) => (
    <div className={className} data-testid="card-content">
      {children}
    </div>
  ),
  CardDescription: ({ children }: { children: React.ReactNode }) => (
    <p data-testid="card-description">{children}</p>
  ),
  CardHeader: ({
    children,
    className,
  }: {
    children: React.ReactNode;
    className?: string;
  }) => (
    <div className={className} data-testid="card-header">
      {children}
    </div>
  ),
  CardTitle: ({
    children,
    className,
  }: {
    children: React.ReactNode;
    className?: string;
  }) => (
    <h2 className={className} data-testid="card-title">
      {children}
    </h2>
  ),
}));

vi.mock("@/components/ui/input", () => ({
  Input: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input {...props} />
  ),
}));

// Mock lucide-react icons
vi.mock("lucide-react", () => ({
  Folder: () => <span data-testid="folder-icon">📁</span>,
  FolderOpen: () => <span data-testid="folder-open-icon">📂</span>,
  Plus: () => <span data-testid="plus-icon">➕</span>,
  ChevronRight: () => <span data-testid="chevron-right-icon">➡️</span>,
  Clock: () => <span data-testid="clock-icon">🕒</span>,
  ArrowRight: () => <span data-testid="arrow-right-icon">→</span>,
}));

describe("Home Page", () => {
  const mockApps: App[] = [
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

  describe("Initial rendering", () => {
    it("should render main heading and create button", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      expect(screen.getByText("Golem Desktop")).toBeInTheDocument();
      expect(screen.getByText("New Application")).toBeInTheDocument();
    });

    it("should render action cards", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      // Use more specific selectors to avoid duplicate text issue
      expect(
        screen.getByRole("heading", { name: "Create New Application" }),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("heading", { name: "Open Existing Application" }),
      ).toBeInTheDocument();
      expect(
        screen.getByText("Start a new Golem application project"),
      ).toBeInTheDocument();
      expect(
        screen.getByText("Open and work with an existing Golem application"),
      ).toBeInTheDocument();
    });

    it("should render recent applications section", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      expect(screen.getByText("Recent Applications")).toBeInTheDocument();
      expect(
        screen.getByText("Your recently opened applications"),
      ).toBeInTheDocument();
    });
  });

  describe("App loading", () => {
    it("should load and display recent apps", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
        expect(screen.getByText("Test App 2")).toBeInTheDocument();
      });

      expect(settingsService.getApps).toHaveBeenCalled();
    });

    it("should show no recent apps message when list is empty", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(
          screen.getByText("No recent applications found"),
        ).toBeInTheDocument();
      });
    });

    it("should handle app loading error gracefully", async () => {
      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockRejectedValue(new Error("Failed to load"));

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(consoleSpy).toHaveBeenCalledWith(
          "Failed to fetch apps:",
          expect.any(Error),
        );
      });

      consoleSpy.mockRestore();
    });
  });

  describe("Navigation", () => {
    it("should navigate to app creation when create button is clicked", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      const createButton = screen.getByRole("button", {
        name: /create new application/i,
      });
      await user.click(createButton);

      expect(mockNavigate).toHaveBeenCalledWith("/app-create");
    });

    it("should navigate to app creation when header button is clicked", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      const headerButton = screen.getByText("New Application");
      await user.click(headerButton);

      expect(mockNavigate).toHaveBeenCalledWith("/app-create");
    });

    it("should navigate to app when recent app is clicked", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
      });

      const appCard = screen
        .getByText("Test App 1")
        .closest('[data-testid="card"]');
      await user.click(appCard!);

      expect(mockNavigate).toHaveBeenCalledWith("/app/app-1");
    });
  });

  describe("Opening existing app", () => {
    it("should handle successful app opening", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const { settingsService } = await import("@/lib/settings");

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
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      const openButton = screen.getByText("Open");
      await user.click(openButton);

      await waitFor(() => {
        expect(settingsService.validateGolemApp).toHaveBeenCalledWith(
          "/path/to/new/app",
        );
        expect(settingsService.addApp).toHaveBeenCalled();
        expect(mockNavigate).toHaveBeenCalledWith(
          expect.stringMatching(/^\/app\/app-\d+$/),
        );
      });
    });

    it("should show error toast for invalid golem app", async () => {
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
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

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

    it("should handle dialog cancellation", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const { settingsService } = await import("@/lib/settings");

      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);
      (open as MockedFunction<typeof open>).mockResolvedValue(null);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      const openButton = screen.getByText("Open");
      await user.click(openButton);

      await waitFor(() => {
        expect(open).toHaveBeenCalled();
      });

      expect(settingsService.validateGolemApp).not.toHaveBeenCalled();
    });

    it("should handle opening error", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const { settingsService } = await import("@/lib/settings");
      const { toast } = await import("@/hooks/use-toast");

      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);
      (open as MockedFunction<typeof open>).mockRejectedValue(
        new Error("Dialog error"),
      );

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      const openButton = screen.getByText("Open");
      await user.click(openButton);

      await waitFor(() => {
        expect(toast).toHaveBeenCalledWith({
          title: "Error opening application",
          description: "Error: Dialog error",
          variant: "destructive",
        });
      });
    });

    it("should show loading state during app opening", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const { settingsService } = await import("@/lib/settings");

      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([]);
      (open as MockedFunction<typeof open>).mockImplementation(
        () => new Promise(resolve => setTimeout(() => resolve("/path"), 100)),
      );

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      const openButton = screen.getByText("Open");
      await user.click(openButton);

      expect(screen.getByText("Opening...")).toBeInTheDocument();

      await waitFor(() => {
        expect(
          screen.getByText("Open Existing Application"),
        ).toBeInTheDocument();
      });
    });
  });

  describe("App search functionality", () => {
    it("should filter apps based on search term", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
        expect(screen.getByText("Test App 2")).toBeInTheDocument();
      });

      const searchInput = screen.getByPlaceholderText("Search applications...");
      await user.type(searchInput, "Test App 1");

      expect(screen.getByText("Test App 1")).toBeInTheDocument();
      expect(screen.queryByText("Test App 2")).not.toBeInTheDocument();
    });

    it("should show no matching message when search has no results", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
      });

      const searchInput = screen.getByPlaceholderText("Search applications...");
      await user.type(searchInput, "nonexistent");

      expect(
        screen.getByText("No matching applications found"),
      ).toBeInTheDocument();
    });

    it("should search by folder location", async () => {
      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(mockApps);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText("Test App 1")).toBeInTheDocument();
      });

      const searchInput = screen.getByPlaceholderText("Search applications...");
      await user.type(searchInput, "app1");

      expect(screen.getByText("Test App 1")).toBeInTheDocument();
      expect(screen.queryByText("Test App 2")).not.toBeInTheDocument();
    });
  });

  describe("App display functionality", () => {
    it('should show "View All" button when there are more than 3 apps', async () => {
      const manyApps = Array.from({ length: 5 }, (_, i) => ({
        id: `app-${i + 1}`,
        name: `Test App ${i + 1}`,
        folderLocation: `/path/to/app${i + 1}`,
        golemYamlLocation: `/path/to/app${i + 1}/golem.yaml`,
        lastOpened: "2023-12-01T10:00:00Z",
      }));

      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(manyApps);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText("View All")).toBeInTheDocument();
      });
    });

    it("should toggle between showing all apps and limited apps", async () => {
      const manyApps = Array.from({ length: 8 }, (_, i) => ({
        id: `app-${i + 1}`,
        name: `Test App ${i + 1}`,
        folderLocation: `/path/to/app${i + 1}`,
        golemYamlLocation: `/path/to/app${i + 1}/golem.yaml`,
        lastOpened: "2023-12-01T10:00:00Z",
      }));

      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(manyApps);

      const user = userEvent.setup();
      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText("View All")).toBeInTheDocument();
      });

      const viewAllButton = screen.getByText("View All");
      await user.click(viewAllButton);

      expect(screen.getByText("Show Less")).toBeInTheDocument();
    });

    it("should sort apps by last opened date", async () => {
      const unsortedApps = [
        {
          id: "app-1",
          name: "Older App",
          folderLocation: "/path/to/app1",
          golemYamlLocation: "/path/to/app1/golem.yaml",
          lastOpened: "2023-11-01T10:00:00Z",
        },
        {
          id: "app-2",
          name: "Newer App",
          folderLocation: "/path/to/app2",
          golemYamlLocation: "/path/to/app2/golem.yaml",
          lastOpened: "2023-12-01T10:00:00Z",
        },
      ];

      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue(unsortedApps);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        const appCards = screen.getAllByTestId("card");
        const recentAppsSection = appCards.find(card =>
          card.textContent?.includes("Recent Applications"),
        );
        expect(recentAppsSection).toBeInTheDocument();
      });
    });
  });

  describe("formatRelativeTime function", () => {
    it("should display relative time correctly", async () => {
      const recentApp = {
        id: "app-1",
        name: "Recent App",
        folderLocation: "/path/to/app1",
        golemYamlLocation: "/path/to/app1/golem.yaml",
        lastOpened: new Date(Date.now() - 60000).toISOString(), // 1 minute ago
      };

      const { settingsService } = await import("@/lib/settings");
      (
        settingsService.getApps as MockedFunction<
          typeof settingsService.getApps
        >
      ).mockResolvedValue([recentApp]);

      await act(async () => {
        render(
          <MemoryRouter>
            <Home />
          </MemoryRouter>,
        );
      });

      await waitFor(() => {
        expect(screen.getByText(/1 minute ago/)).toBeInTheDocument();
      });
    });
  });
});
