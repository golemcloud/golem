import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  type MockedFunction,
} from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { ServerStatus } from "../server-status";
import { API } from "@/service";

// Mock the API
vi.mock("@/service", () => ({
  API: {
    appService: {
      checkHealth: vi.fn(),
    },
  },
}));

// Mock lucide-react icons
vi.mock("lucide-react", () => ({
  CheckCircle2: () => <div data-testid="check-icon">✓</div>,
  AlertCircle: () => <div data-testid="alert-icon">!</div>,
}));

// Mock tooltip components
vi.mock("@/components/ui/tooltip", () => ({
  TooltipProvider: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  Tooltip: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  TooltipTrigger: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
  TooltipContent: ({ children }: { children: React.ReactNode }) => (
    <div>{children}</div>
  ),
}));

describe("ServerStatus", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders server status component with loading state initially", () => {
    (
      API.appService.checkHealth as MockedFunction<
        typeof API.appService.checkHealth
      >
    ).mockImplementation(() => new Promise(() => {})); // Never resolves

    render(<ServerStatus />);

    expect(screen.getByText("Checking status...")).toBeInTheDocument();
  });

  it("displays healthy status when connected", async () => {
    (
      API.appService.checkHealth as MockedFunction<
        typeof API.appService.checkHealth
      >
    ).mockResolvedValue();

    render(<ServerStatus />);

    await waitFor(() => {
      expect(screen.getByText("Healthy")).toBeInTheDocument();
      expect(screen.getByTestId("check-icon")).toBeInTheDocument();
    });
  });

  it("displays unhealthy status when disconnected", async () => {
    (
      API.appService.checkHealth as MockedFunction<
        typeof API.appService.checkHealth
      >
    ).mockRejectedValue(new Error("Connection failed"));

    render(<ServerStatus />);

    await waitFor(() => {
      expect(screen.getByText("Unhealthy")).toBeInTheDocument();
      expect(screen.getByTestId("alert-icon")).toBeInTheDocument();
    });
  });

  it("displays connecting status when loading", () => {
    (
      API.appService.checkHealth as MockedFunction<
        typeof API.appService.checkHealth
      >
    ).mockImplementation(() => new Promise(() => {})); // Never resolves

    render(<ServerStatus />);

    expect(screen.getByText("Checking status...")).toBeInTheDocument();
    const loadingIndicator = document.querySelector(".animate-pulse");
    expect(loadingIndicator).toBeInTheDocument();
  });

  it("has correct text color based on status", async () => {
    (
      API.appService.checkHealth as MockedFunction<
        typeof API.appService.checkHealth
      >
    ).mockResolvedValue();

    render(<ServerStatus />);

    await waitFor(() => {
      const statusElement = screen.getByText("Healthy").closest("div");
      expect(statusElement).toHaveClass("text-green-500");
    });
  });
});
