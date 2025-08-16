import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { SettingsPage } from "../index";

// Mock dependencies
vi.mock("@/components/ui/card", () => ({
  Card: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="card">{children}</div>
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

vi.mock("@/components/golem-cli-path", () => ({
  GolemCliPathSetting: () => (
    <div data-testid="golem-cli-path-setting">Golem CLI Path Setting</div>
  ),
}));

describe("SettingsPage", () => {
  const renderSettingsPage = () => {
    return render(
      <MemoryRouter>
        <SettingsPage />
      </MemoryRouter>,
    );
  };

  describe("Component Rendering", () => {
    it("should render the settings page", () => {
      renderSettingsPage();
      expect(screen.getByText("Settings")).toBeInTheDocument();
    });

    it("should render the main heading", () => {
      renderSettingsPage();
      expect(
        screen.getByRole("heading", { name: "Settings" }),
      ).toBeInTheDocument();
    });

    it("should render basic components", () => {
      renderSettingsPage();
      expect(screen.getByText("CLI Profiles")).toBeInTheDocument();
      expect(screen.getByText("CLI Path")).toBeInTheDocument();
    });
  });
});
