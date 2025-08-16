import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { Dashboard } from "../index";
import { BrowserRouter } from "react-router-dom";
import { ComponentList, ComponentType } from "@/types/component";
import { Deployment } from "@/types/deployments";
import {
  HttpApiDefinition,
  HttpApiDefinitionRoute,
} from "@/types/golemManifest";
import { App } from "@/lib/settings";

// Mock lucide-react icons
vi.mock("lucide-react", () => ({
  Play: () => <div data-testid="play-icon">▶</div>,
  RefreshCw: () => <div data-testid="refresh-icon">🔄</div>,
  Upload: () => <div data-testid="upload-icon">⬆</div>,
  Trash2: () => <div data-testid="trash-icon">🗑</div>,
  FileText: () => <div data-testid="file-text-icon">📄</div>,
  Send: () => <div data-testid="send-icon">📤</div>,
  Loader2: () => (
    <div data-testid="loader-icon" className="animate-spin">
      ⏳
    </div>
  ),
  ArrowRight: () => <div data-testid="arrow-right">→</div>,
  LayoutGrid: () => <div data-testid="layout-grid">⚏</div>,
  PlusCircle: () => <div data-testid="plus-circle">➕</div>,
  Globe: () => <div data-testid="globe">🌐</div>,
  Layers: () => <div data-testid="layers">📚</div>,
  Server: () => <div data-testid="server">🖥</div>,
}));

// Mock react-router-dom
const mockNavigate = vi.fn();
const mockAppId = "test-app-123";

vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual("react-router-dom");
  return {
    ...actual,
    useNavigate: () => mockNavigate,
    useParams: () => ({ appId: mockAppId }),
  };
});

// Mock API service
vi.mock("@/service", () => ({
  API: {
    appService: {
      buildApp: vi.fn(),
      updateWorkers: vi.fn(),
      deployWorkers: vi.fn(),
      cleanApp: vi.fn(),
    },
    componentService: {
      getComponentByIdAsKey: vi.fn(),
    },
    deploymentService: {
      getDeploymentApi: vi.fn(),
    },
    apiService: {
      getApiList: vi.fn(),
    },
    manifestService: {
      getAppYamlContent: vi.fn(),
    },
  },
}));

// Mock store service
vi.mock("@/lib/settings.ts", () => ({
  storeService: {
    getAppById: vi.fn(),
    updateAppLastOpened: vi.fn(),
  },
}));

// Mock toast hook
vi.mock("@/hooks/use-toast", () => ({
  toast: vi.fn(),
}));

// Mock log viewer context
const mockShowLog = vi.fn();
vi.mock("@/contexts/log-viewer-context", () => ({
  useLogViewer: () => ({ showLog: mockShowLog }),
}));

// Mock section components
vi.mock("@/pages/dashboard/componentSection.tsx", () => ({
  ComponentsSection: vi.fn().mockImplementation(() => (
    <div data-testid="components-section">
      <h2>Components Section</h2>
      <div>Component List Placeholder</div>
    </div>
  )),
}));

vi.mock("@/pages/dashboard/deploymentSection.tsx", () => ({
  DeploymentSection: () => (
    <div data-testid="deployment-section">
      <h2>Deployment Section</h2>
      <div>Deployment List Placeholder</div>
    </div>
  ),
}));

vi.mock("@/pages/dashboard/apiSection.tsx", () => ({
  APISection: () => (
    <div data-testid="api-section">
      <h2>API Section</h2>
      <div>API List Placeholder</div>
    </div>
  ),
}));

// Mock YAML Viewer Modal
vi.mock("@/components/yaml-viewer-modal", () => ({
  YamlViewerModal: ({
    isOpen,
    title,
    yamlContent,
  }: {
    isOpen: boolean;
    title: string;
    yamlContent: string;
  }) =>
    isOpen ? (
      <div data-testid="yaml-modal">
        <h3>{title}</h3>
        <pre data-testid="yaml-content">{yamlContent}</pre>
        <button data-testid="close-yaml-modal">Close</button>
      </div>
    ) : null,
}));

// Mock utils
vi.mock("@/lib/utils", () => ({
  cn: (...inputs: (string | undefined | null | boolean)[]) =>
    inputs.filter(Boolean).join(" "),
  removeDuplicateApis: (apis: HttpApiDefinition[]) => apis,
}));

// Import mocked modules for test access
import { API } from "@/service";
import { storeService } from "@/lib/settings.ts";
import { toast } from "@/hooks/use-toast";

const mockAPI = vi.mocked(API);
const mockStoreService = vi.mocked(storeService);
const mockToast = vi.mocked(toast);

// Test wrapper component
const TestWrapper = ({ children }: { children: React.ReactNode }) => (
  <BrowserRouter>{children}</BrowserRouter>
);

describe("Dashboard", () => {
  const sampleApp: App = {
    id: mockAppId,
    name: "Test Application",
    folderLocation: "/test/path",
    golemYamlLocation: "/test/path/golem.yaml",
    lastOpened: new Date().toISOString(),
  };

  const sampleComponents: { [key: string]: ComponentList } = {
    "test-component-1": {
      componentName: "user-service",
      componentType: "Durable",
      componentId: "test-component-1",
      versionList: [1, 2, 3],
      versions: [
        {
          componentId: "test-component-1",
          componentName: "user-service",
          componentVersion: 3,
          componentType: ComponentType.Durable,
          createdAt: "2024-01-01T00:00:00Z",
        },
      ],
    },
  };

  const sampleDeployments: Deployment[] = [
    {
      apiDefinitions: [{ id: "api-1", version: "1.0.0" }],
      createdAt: "2024-01-01T00:00:00Z",
      projectId: "project-1",
      site: {
        host: "production.example.com",
        subdomain: "production",
      },
    },
  ];

  const sampleApis: HttpApiDefinition[] = [
    {
      id: "api-1",
      version: "1.0.0",
      routes: [
        {
          method: "GET",
          path: "/users",
          binding: {
            type: "http-handler",
            componentName: "user-service",
            componentVersion: 1,
          },
        } as HttpApiDefinitionRoute,
      ],
    },
  ];

  beforeEach(() => {
    vi.clearAllMocks();
    mockShowLog.mockClear();

    // Setup default mock implementations
    vi.mocked(mockStoreService.getAppById).mockResolvedValue(sampleApp);
    vi.mocked(mockStoreService.updateAppLastOpened).mockResolvedValue(true);
    vi.mocked(mockAPI.componentService.getComponentByIdAsKey).mockResolvedValue(
      sampleComponents,
    );
    vi.mocked(mockAPI.deploymentService.getDeploymentApi).mockResolvedValue(
      sampleDeployments,
    );
    vi.mocked(mockAPI.apiService.getApiList).mockResolvedValue(sampleApis);
    vi.mocked(mockAPI.manifestService.getAppYamlContent).mockResolvedValue(
      "apiVersion: v1\nkind: App",
    );

    // Setup successful API responses by default
    vi.mocked(mockAPI.appService.buildApp).mockResolvedValue({
      success: true,
      logs: "Build completed",
      result: {},
    });
    vi.mocked(mockAPI.appService.updateWorkers).mockResolvedValue({
      success: true,
      logs: "Workers updated",
      result: {},
    });
    vi.mocked(mockAPI.appService.deployWorkers).mockResolvedValue({
      success: true,
      logs: "Workers deployed",
      result: {},
    });
    vi.mocked(mockAPI.appService.cleanApp).mockResolvedValue({
      success: true,
      logs: "App cleaned",
      result: {},
    });
  });

  it("renders the dashboard layout correctly", async () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    // Check main title and app name
    await waitFor(() => {
      expect(
        screen.getByText("Working in Test Application"),
      ).toBeInTheDocument();
    });

    // Check back button
    expect(screen.getByText("Back to Apps")).toBeInTheDocument();

    // Check app actions section
    expect(screen.getByText("App Actions")).toBeInTheDocument();

    // Check all action buttons are present
    expect(screen.getByText("Build App")).toBeInTheDocument();
    expect(screen.getByText("Update Workers")).toBeInTheDocument();
    expect(screen.getByText("Deploy Workers")).toBeInTheDocument();
    expect(screen.getByText("Deploy App")).toBeInTheDocument();
    expect(screen.getByText("Clean App")).toBeInTheDocument();
    expect(screen.getByText("View YAML")).toBeInTheDocument();

    // Check sections are rendered
    expect(screen.getByTestId("components-section")).toBeInTheDocument();
    expect(screen.getByTestId("deployment-section")).toBeInTheDocument();
    expect(screen.getByTestId("api-section")).toBeInTheDocument();
  });

  it("navigates back to apps page when back button is clicked", () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const backButton = screen.getByText("Back to Apps");
    fireEvent.click(backButton);

    expect(mockNavigate).toHaveBeenCalledWith("/");
  });

  it("handles build app action successfully", async () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const buildButton = screen.getByText("Build App");
    fireEvent.click(buildButton);

    // Check loading state
    expect(screen.getByTestId("loader-icon")).toBeInTheDocument();

    // Wait for completion and check toast
    await waitFor(() => {
      expect(mockAPI.appService.buildApp).toHaveBeenCalledWith(mockAppId);
      expect(mockToast).toHaveBeenCalledWith({
        title: "Build Completed",
        description: "Application build completed successfully.",
      });
    });
  });

  it("handles build app failure with log display", async () => {
    vi.mocked(mockAPI.appService.buildApp).mockResolvedValue({
      success: false,
      logs: "Build failed: compilation error",
      result: {},
    });

    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const buildButton = screen.getByText("Build App");
    fireEvent.click(buildButton);

    await waitFor(() => {
      expect(mockShowLog).toHaveBeenCalledWith({
        title: "Build Failed",
        logs: "Build failed: compilation error",
        status: "error",
        operation: "Build App",
      });
    });
  });

  it("handles update workers action successfully", async () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const updateButton = screen.getByText("Update Workers");
    fireEvent.click(updateButton);

    await waitFor(() => {
      expect(mockAPI.appService.updateWorkers).toHaveBeenCalledWith(mockAppId);
      expect(mockToast).toHaveBeenCalledWith({
        title: "Workers Update Completed",
        description: "Worker update process completed successfully.",
      });
    });
  });

  it("handles deploy workers action successfully", async () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const deployButton = screen.getByText("Deploy Workers");
    fireEvent.click(deployButton);

    await waitFor(() => {
      expect(mockAPI.appService.deployWorkers).toHaveBeenCalledWith(mockAppId);
      expect(mockToast).toHaveBeenCalledWith({
        title: "Deployment Completed",
        description: "Worker deployment completed successfully.",
      });
    });
  });

  it("opens YAML modal when View YAML is clicked", async () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const viewYamlButton = screen.getByText("View YAML");
    fireEvent.click(viewYamlButton);

    await waitFor(() => {
      expect(mockAPI.manifestService.getAppYamlContent).toHaveBeenCalledWith(
        mockAppId,
      );
      expect(screen.getByTestId("yaml-modal")).toBeInTheDocument();
      expect(
        screen.getByText("Application Manifest (golem.yaml)"),
      ).toBeInTheDocument();
      expect(screen.getByTestId("yaml-content").textContent).toMatch(
        /apiVersion:\s*v1[\s\S]*kind:\s*App/,
      );
    });
  });

  it("handles YAML loading failure", async () => {
    vi.mocked(mockAPI.manifestService.getAppYamlContent).mockRejectedValue(
      new Error("YAML not found"),
    );

    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const viewYamlButton = screen.getByText("View YAML");
    fireEvent.click(viewYamlButton);

    await waitFor(() => {
      expect(mockToast).toHaveBeenCalledWith({
        title: "Failed to Load YAML",
        description: "Error: YAML not found",
        variant: "destructive",
      });
    });
  });

  it("shows loading states for action buttons", async () => {
    // Mock a delayed response to test loading state
    vi.mocked(mockAPI.appService.buildApp).mockImplementation(
      () =>
        new Promise(resolve =>
          setTimeout(
            () => resolve({ success: true, logs: "", result: {} }),
            100,
          ),
        ),
    );

    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const buildButton = screen.getByText("Build App");
    fireEvent.click(buildButton);

    // Check that loader icon appears and button is disabled
    expect(screen.getByTestId("loader-icon")).toBeInTheDocument();
    expect(buildButton).toBeDisabled();

    await waitFor(() => {
      expect(screen.queryByTestId("loader-icon")).not.toBeInTheDocument();
      expect(buildButton).not.toBeDisabled();
    });
  });

  it("loads app data on component mount", async () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    await waitFor(() => {
      expect(mockStoreService.getAppById).toHaveBeenCalledWith(mockAppId);
      expect(mockStoreService.updateAppLastOpened).toHaveBeenCalledWith(
        mockAppId,
      );
    });
  });

  it("displays default app name when app name is not available", async () => {
    vi.mocked(mockStoreService.getAppById).mockResolvedValue({
      id: mockAppId,
      name: "",
      folderLocation: "/test/path",
      golemYamlLocation: "/test/path/golem.yaml",
      lastOpened: new Date().toISOString(),
    });

    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    await waitFor(() => {
      expect(screen.getByText("Working in App")).toBeInTheDocument();
    });
  });

  it("ensures all buttons have correct icons", () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    // Check that icons are present for each button
    expect(screen.getByTestId("play-icon")).toBeInTheDocument(); // Build App
    expect(screen.getByTestId("refresh-icon")).toBeInTheDocument(); // Update Workers
    expect(screen.getByTestId("upload-icon")).toBeInTheDocument(); // Deploy Workers
    expect(screen.getByTestId("send-icon")).toBeInTheDocument(); // Deploy App
    expect(screen.getByTestId("trash-icon")).toBeInTheDocument(); // Clean App
    expect(screen.getByTestId("file-text-icon")).toBeInTheDocument(); // View YAML
  });

  it("maintains responsive grid layout", () => {
    render(
      <TestWrapper>
        <Dashboard />
      </TestWrapper>,
    );

    const gridContainer =
      screen.getByTestId("components-section").parentElement;
    expect(gridContainer).toHaveClass(
      "grid",
      "flex-1",
      "grid-cols-1",
      "gap-4",
      "lg:grid-cols-3",
    );
  });
});
