import { render, screen } from "@testing-library/react";
import { BrowserRouter } from "react-router-dom";
import { ThemeProvider } from "../theme-provider";
import Navbar from "../navbar";

// Mock components
vi.mock("../logo", () => ({
  Logo: () => <div data-testid="logo">Logo</div>,
}));

vi.mock("../mode-toggle", () => ({
  ModeToggle: () => <div data-testid="mode-toggle">Mode Toggle</div>,
}));

vi.mock("../navLink", () => ({
  default: ({ to, children }: { to: string; children: React.ReactNode }) => (
    <a href={to} data-testid={`nav-link-${to.split("/").pop()}`}>
      {children}
    </a>
  ),
}));

vi.mock("../server-status", () => ({
  ServerStatus: () => <div data-testid="server-status">Server Status</div>,
}));

// Mock lucide-react
vi.mock("lucide-react", () => ({
  Settings: () => <div data-testid="settings-icon">Settings</div>,
}));

// Mock useParams
const mockUseParams = vi.fn();
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual("react-router-dom");
  return {
    ...actual,
    useParams: () => mockUseParams(),
  };
});

const renderNavbar = (props = {}) => {
  return render(
    <BrowserRouter>
      <ThemeProvider>
        <Navbar {...props} />
      </ThemeProvider>
    </BrowserRouter>,
  );
};

describe("Navbar", () => {
  beforeEach(() => {
    mockUseParams.mockReturnValue({});
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("renders navbar with basic components", () => {
    renderNavbar();

    expect(screen.getByTestId("logo")).toBeInTheDocument();
    expect(screen.getByTestId("server-status")).toBeInTheDocument();
    expect(screen.getByTestId("mode-toggle")).toBeInTheDocument();
    expect(screen.getByTestId("settings-icon")).toBeInTheDocument();
  });

  it("renders logo as a link to home", () => {
    renderNavbar();

    const logoLink = screen.getByTestId("logo").closest("a");
    expect(logoLink).toHaveAttribute("href", "/");
  });

  it("renders settings button with correct link", () => {
    renderNavbar();

    const settingsLink = screen.getByTestId("nav-link-settings");
    expect(settingsLink).toHaveAttribute("href", "/settings");
  });

  it("shows navigation links when showNav is true and appId is present", () => {
    mockUseParams.mockReturnValue({ appId: "test-app" });
    renderNavbar({ showNav: true });

    expect(screen.getByTestId("nav-link-dashboard")).toBeInTheDocument();
    expect(screen.getByTestId("nav-link-components")).toBeInTheDocument();
    expect(screen.getByTestId("nav-link-apis")).toBeInTheDocument();
    expect(screen.getByTestId("nav-link-deployments")).toBeInTheDocument();
    expect(screen.getByTestId("nav-link-plugins")).toBeInTheDocument();
  });

  it("hides navigation links when showNav is false", () => {
    mockUseParams.mockReturnValue({ appId: "test-app" });
    renderNavbar({ showNav: false });

    expect(screen.queryByTestId("nav-link-dashboard")).not.toBeInTheDocument();
    expect(screen.queryByTestId("nav-link-components")).not.toBeInTheDocument();
    expect(screen.queryByTestId("nav-link-apis")).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("nav-link-deployments"),
    ).not.toBeInTheDocument();
    expect(screen.queryByTestId("nav-link-plugins")).not.toBeInTheDocument();
  });

  it("hides navigation links when appId is not present", () => {
    mockUseParams.mockReturnValue({});
    renderNavbar({ showNav: true });

    expect(screen.queryByTestId("nav-link-dashboard")).not.toBeInTheDocument();
    expect(screen.queryByTestId("nav-link-components")).not.toBeInTheDocument();
    expect(screen.queryByTestId("nav-link-apis")).not.toBeInTheDocument();
    expect(
      screen.queryByTestId("nav-link-deployments"),
    ).not.toBeInTheDocument();
    expect(screen.queryByTestId("nav-link-plugins")).not.toBeInTheDocument();
  });

  it("generates correct URLs for navigation links with appId", () => {
    const appId = "my-test-app";
    mockUseParams.mockReturnValue({ appId });
    renderNavbar({ showNav: true });

    expect(screen.getByTestId("nav-link-dashboard")).toHaveAttribute(
      "href",
      `/app/${appId}/dashboard`,
    );
    expect(screen.getByTestId("nav-link-components")).toHaveAttribute(
      "href",
      `/app/${appId}/components`,
    );
    expect(screen.getByTestId("nav-link-apis")).toHaveAttribute(
      "href",
      `/app/${appId}/apis`,
    );
    expect(screen.getByTestId("nav-link-deployments")).toHaveAttribute(
      "href",
      `/app/${appId}/deployments`,
    );
    expect(screen.getByTestId("nav-link-plugins")).toHaveAttribute(
      "href",
      `/app/${appId}/plugins`,
    );
  });

  it("uses default showNav prop value", () => {
    mockUseParams.mockReturnValue({ appId: "test-app" });
    renderNavbar(); // No showNav prop provided, should default to true

    expect(screen.getByTestId("nav-link-dashboard")).toBeInTheDocument();
  });

  it("has correct CSS classes for layout", () => {
    renderNavbar();

    const nav = screen.getByRole("navigation");
    expect(nav).toHaveClass("border-b");

    const container = nav.firstChild as HTMLElement;
    expect(container).toHaveClass(
      "flex",
      "items-center",
      "justify-between",
      "px-4",
      "py-2",
    );
  });

  it("renders navigation text correctly", () => {
    mockUseParams.mockReturnValue({ appId: "test-app" });
    renderNavbar({ showNav: true });

    expect(screen.getByText("Dashboard")).toBeInTheDocument();
    expect(screen.getByText("Components")).toBeInTheDocument();
    expect(screen.getByText("APIs")).toBeInTheDocument();
    expect(screen.getByText("Deployments")).toBeInTheDocument();
    expect(screen.getByText("Plugins")).toBeInTheDocument();
  });
});
