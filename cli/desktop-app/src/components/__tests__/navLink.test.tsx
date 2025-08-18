import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
// import React from 'react';
import NavLink from "../navLink";

// Mock react-router-dom
const mockUseLocation = vi.fn();
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual("react-router-dom");
  return {
    ...actual,
    useLocation: () => mockUseLocation(),
    Link: ({
      to,
      children,
      className,
    }: {
      to: string;
      children: React.ReactNode;
      className?: string;
    }) => (
      <a href={to} className={className}>
        {children}
      </a>
    ),
  };
});

describe("NavLink", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  describe("Active state detection", () => {
    it("should apply active styles when pathname matches exactly", () => {
      mockUseLocation.mockReturnValue({ pathname: "/dashboard" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Dashboard");
      expect(link).toHaveClass("bg-primary-background");
      expect(link).toHaveClass("border-b-2");
      expect(link).toHaveClass("border-primary-soft");
      expect(link).toHaveClass("text-primary-soft");
      expect(link).toHaveClass("py-2");
    });

    it("should apply active styles when pathname starts with route", () => {
      mockUseLocation.mockReturnValue({ pathname: "/dashboard/settings" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Dashboard");
      expect(link).toHaveClass("bg-primary-background");
      expect(link).toHaveClass("border-b-2");
      expect(link).toHaveClass("border-primary-soft");
      expect(link).toHaveClass("text-primary-soft");
    });

    it("should apply inactive styles when pathname does not match", () => {
      mockUseLocation.mockReturnValue({ pathname: "/components" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Dashboard");
      expect(link).toHaveClass("text-gray-500");
      expect(link).toHaveClass("hover:text-gray-700");
      expect(link).toHaveClass("py-2");
      expect(link).not.toHaveClass("bg-primary-background");
    });

    it("should not be active for partial matches that dont start with route", () => {
      mockUseLocation.mockReturnValue({ pathname: "/some-dashboard" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Dashboard");
      expect(link).toHaveClass("text-gray-500");
      expect(link).not.toHaveClass("bg-primary-background");
    });
  });

  describe("Rendering", () => {
    it("should render children correctly", () => {
      mockUseLocation.mockReturnValue({ pathname: "/home" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">
            <span>Dashboard Icon</span>
            Dashboard
          </NavLink>
        </MemoryRouter>,
      );

      expect(screen.getByText("Dashboard Icon")).toBeInTheDocument();
      expect(screen.getByText("Dashboard")).toBeInTheDocument();
    });

    it("should set correct href attribute", () => {
      mockUseLocation.mockReturnValue({ pathname: "/home" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Dashboard");
      expect(link).toHaveAttribute("href", "/dashboard");
    });

    it("should render with complex children structure", () => {
      mockUseLocation.mockReturnValue({ pathname: "/dashboard" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">
            <div className="flex items-center">
              <svg>Icon</svg>
              <span>Dashboard</span>
            </div>
          </NavLink>
        </MemoryRouter>,
      );

      expect(screen.getByText("Icon")).toBeInTheDocument();
      expect(screen.getByText("Dashboard")).toBeInTheDocument();
    });
  });

  describe("Edge cases", () => {
    it("should handle root path correctly", () => {
      mockUseLocation.mockReturnValue({ pathname: "/" });

      render(
        <MemoryRouter>
          <NavLink to="/">Home</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Home");
      expect(link).toHaveClass("bg-primary-background");
    });

    it("should handle deep nested paths", () => {
      mockUseLocation.mockReturnValue({
        pathname: "/api/v1/components/details/settings",
      });

      render(
        <MemoryRouter>
          <NavLink to="/api">API</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("API");
      expect(link).toHaveClass("bg-primary-background");
    });

    it("should handle similar path names correctly", () => {
      mockUseLocation.mockReturnValue({ pathname: "/dashboard-admin" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Dashboard");
      expect(link).not.toHaveClass("bg-primary-background");
      expect(link).toHaveClass("text-gray-500");
    });

    it("should handle paths with query parameters", () => {
      mockUseLocation.mockReturnValue({
        pathname: "/dashboard/users",
        search: "?page=1&limit=10",
      });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByText("Dashboard");
      expect(link).toHaveClass("bg-primary-background");
    });

    it("should handle empty children", () => {
      mockUseLocation.mockReturnValue({ pathname: "/dashboard" });

      render(
        <MemoryRouter>
          <NavLink to="/dashboard">Dashboard</NavLink>
        </MemoryRouter>,
      );

      const link = screen.getByRole("link");
      expect(link).toBeInTheDocument();
    });
  });

  describe("Multiple NavLinks", () => {
    it("should handle multiple NavLinks with different active states", () => {
      mockUseLocation.mockReturnValue({ pathname: "/components" });

      render(
        <MemoryRouter>
          <nav>
            <NavLink to="/dashboard">Dashboard</NavLink>
            <NavLink to="/components">Components</NavLink>
            <NavLink to="/workers">Workers</NavLink>
          </nav>
        </MemoryRouter>,
      );

      const dashboardLink = screen.getByText("Dashboard");
      const componentsLink = screen.getByText("Components");
      const workersLink = screen.getByText("Workers");

      expect(dashboardLink).toHaveClass("text-gray-500");
      expect(componentsLink).toHaveClass("bg-primary-background");
      expect(workersLink).toHaveClass("text-gray-500");
    });

    it("should handle nested route activation correctly", () => {
      mockUseLocation.mockReturnValue({ pathname: "/components/details/123" });

      render(
        <MemoryRouter>
          <nav>
            <NavLink to="/dashboard">Dashboard</NavLink>
            <NavLink to="/components">Components</NavLink>
            <NavLink to="/components/details">Details</NavLink>
          </nav>
        </MemoryRouter>,
      );

      const dashboardLink = screen.getByText("Dashboard");
      const componentsLink = screen.getByText("Components");
      const detailsLink = screen.getByText("Details");

      expect(dashboardLink).toHaveClass("text-gray-500");
      expect(componentsLink).toHaveClass("bg-primary-background");
      expect(detailsLink).toHaveClass("bg-primary-background");
    });
  });
});
