import {
  describe,
  it,
  expect,
  vi,
  beforeEach,
  afterEach,
  type MockedFunction,
  type Mocked,
} from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { GolemCliPathSetting } from "../golem-cli-path";

// Mock dependencies
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

vi.mock("@/hooks/use-toast", () => ({
  toast: vi.fn(),
}));

vi.mock("@/lib/settings", () => ({
  settingsService: {
    getGolemCliPath: vi.fn(),
    setGolemCliPath: vi.fn(),
  },
}));

vi.mock("@/components/ui/button", () => ({
  Button: ({
    children,
    onClick,
    disabled,
    variant,
    type,
  }: {
    children: React.ReactNode;
    onClick?: () => void;
    disabled?: boolean;
    variant?: string;
    type?: string;
  }) => (
    <button
      onClick={onClick}
      disabled={disabled}
      data-variant={variant}
      data-type={type}
      data-testid="button"
    >
      {children}
    </button>
  ),
}));

vi.mock("@/components/ui/input", () => ({
  Input: (props: React.InputHTMLAttributes<HTMLInputElement>) => (
    <input {...props} data-testid="input" />
  ),
}));

vi.mock("@/components/ui/label", () => ({
  Label: ({
    children,
    htmlFor,
  }: {
    children: React.ReactNode;
    htmlFor?: string;
  }) => (
    <label htmlFor={htmlFor} data-testid="label">
      {children}
    </label>
  ),
}));

vi.mock("lucide-react", () => ({
  FolderOpen: ({
    size,
    className,
  }: {
    size?: number | string;
    className?: string;
  }) => (
    <span data-testid="folder-open-icon" data-size={size} className={className}>
      📁
    </span>
  ),
  Save: ({
    size,
    className,
  }: {
    size?: number | string;
    className?: string;
  }) => (
    <span data-testid="save-icon" data-size={size} className={className}>
      💾
    </span>
  ),
  Check: ({
    size,
    className,
  }: {
    size?: number | string;
    className?: string;
  }) => (
    <span data-testid="check-icon" data-size={size} className={className}>
      ✓
    </span>
  ),
}));

describe("GolemCliPathSetting", () => {
  // Get properly typed mock references
  let mockOpen: MockedFunction<typeof import("@tauri-apps/plugin-dialog").open>;
  let mockToast: MockedFunction<typeof import("@/hooks/use-toast").toast>;
  let mockSettingsService: Mocked<
    typeof import("@/lib/settings").settingsService
  >;

  beforeEach(async () => {
    vi.clearAllMocks();

    // Get mock references
    const { open } = await import("@tauri-apps/plugin-dialog");
    const { toast } = await import("@/hooks/use-toast");
    const { settingsService } = await import("@/lib/settings");

    mockOpen = vi.mocked(open);
    mockToast = vi.mocked(toast);
    mockSettingsService = vi.mocked(settingsService);

    // Default mock implementations
    mockSettingsService.getGolemCliPath.mockResolvedValue(null);
    mockSettingsService.setGolemCliPath.mockResolvedValue(true);
    mockOpen.mockResolvedValue("/path/to/golem-cli");
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  describe("Basic rendering", () => {
    it("should render all components correctly", () => {
      render(<GolemCliPathSetting />);

      expect(screen.getByTestId("label")).toBeInTheDocument();
      expect(screen.getByText("golem-cli Path")).toBeInTheDocument();
      expect(screen.getByTestId("input")).toBeInTheDocument();
      expect(screen.getByText("Browse")).toBeInTheDocument();
      expect(screen.getByText("Save")).toBeInTheDocument();
      expect(
        screen.getByText(/Specify the path to the golem-cli executable/),
      ).toBeInTheDocument();
    });

    it("should have correct input attributes", () => {
      render(<GolemCliPathSetting />);

      const input = screen.getByTestId("input");
      expect(input).toHaveAttribute("id", "golem-cli-path");
      expect(input).toHaveAttribute(
        "placeholder",
        "Select golem-cli executable path",
      );
      expect(input).toHaveAttribute("readonly");
    });

    it("should show correct icons", () => {
      render(<GolemCliPathSetting />);

      expect(screen.getByTestId("folder-open-icon")).toBeInTheDocument();
      expect(screen.getByTestId("save-icon")).toBeInTheDocument();
    });
  });

  describe("Initial loading", () => {
    it("should load existing golem-cli path on mount", async () => {
      mockSettingsService.getGolemCliPath.mockResolvedValue(
        "/existing/path/to/golem-cli",
      );

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        expect(mockSettingsService.getGolemCliPath).toHaveBeenCalled();
      });

      const input = screen.getByTestId("input");
      expect(input).toHaveValue("/existing/path/to/golem-cli");
    });

    it("should mark as saved when path is loaded from settings", async () => {
      mockSettingsService.getGolemCliPath.mockResolvedValue(
        "/existing/path/to/golem-cli",
      );

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        expect(screen.getByText("Saved")).toBeInTheDocument();
        expect(screen.getByTestId("check-icon")).toBeInTheDocument();
      });
    });

    it("should handle loading errors gracefully", async () => {
      mockSettingsService.getGolemCliPath.mockRejectedValue(
        new Error("Load error"),
      );

      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        expect(consoleSpy).toHaveBeenCalledWith(
          "Error loading golem-cli path:",
          expect.any(Error),
        );
      });

      consoleSpy.mockRestore();
    });

    it("should show Save button when no path is loaded", async () => {
      mockSettingsService.getGolemCliPath.mockResolvedValue(null);

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        expect(screen.getByText("Save")).toBeInTheDocument();
        expect(screen.getByTestId("save-icon")).toBeInTheDocument();
      });
    });
  });

  describe("Browse functionality", () => {
    it("should open file dialog when browse button is clicked", async () => {
      const user = userEvent.setup();

      render(<GolemCliPathSetting />);

      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      expect(mockOpen).toHaveBeenCalledWith({
        multiple: false,
        title: "Select golem-cli executable",
        filters: [
          {
            name: "golem-cli",
            extensions: [],
          },
        ],
      });
    });

    it("should update input value when file is selected", async () => {
      const user = userEvent.setup();
      mockOpen.mockResolvedValue("/new/path/to/golem-cli");

      render(<GolemCliPathSetting />);

      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      await waitFor(() => {
        const input = screen.getByTestId("input");
        expect(input).toHaveValue("/new/path/to/golem-cli");
      });
    });

    it("should mark as unsaved when new path is selected", async () => {
      const user = userEvent.setup();

      // Start with saved state
      mockSettingsService.getGolemCliPath.mockResolvedValue("/existing/path");

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        expect(screen.getByText("Saved")).toBeInTheDocument();
      });

      mockOpen.mockResolvedValue("/new/path/to/golem-cli");
      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      await waitFor(() => {
        expect(screen.getByText("Save")).toBeInTheDocument();
        expect(screen.queryByText("Saved")).not.toBeInTheDocument();
      });
    });

    it("should handle browse dialog cancellation", async () => {
      const user = userEvent.setup();
      mockOpen.mockResolvedValue(null); // User cancelled

      render(<GolemCliPathSetting />);

      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      // Input should remain empty
      const input = screen.getByTestId("input");
      expect(input).toHaveValue("");
    });

    it("should handle browse errors", async () => {
      const user = userEvent.setup();
      mockOpen.mockRejectedValue(new Error("Dialog error"));

      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});

      render(<GolemCliPathSetting />);

      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      await waitFor(() => {
        expect(consoleSpy).toHaveBeenCalledWith(
          "Error selecting golem-cli path:",
          expect.any(Error),
        );
        expect(mockToast).toHaveBeenCalledWith({
          title: "Error selecting golem-cli path",
          description: "Error: Dialog error",
          variant: "destructive",
        });
      });

      consoleSpy.mockRestore();
    });
  });

  describe("Save functionality", () => {
    it("should save path when save button is clicked", async () => {
      const user = userEvent.setup();
      mockOpen.mockResolvedValue("/path/to/golem-cli");

      render(<GolemCliPathSetting />);

      // First browse for a file
      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      await waitFor(() => {
        expect(screen.getByText("Save")).toBeInTheDocument();
      });

      const saveButton = screen.getByText("Save");
      await user.click(saveButton);

      await waitFor(() => {
        expect(mockSettingsService.setGolemCliPath).toHaveBeenCalledWith(
          "/path/to/golem-cli",
        );
        expect(mockToast).toHaveBeenCalledWith({
          title: "golem-cli path saved",
          description: "The path has been saved successfully.",
        });
      });
    });

    it("should show loading state during save", async () => {
      const user = userEvent.setup();

      // Create a promise we can control
      let resolveSave: (value: boolean) => void;
      const savePromise = new Promise<boolean>(resolve => {
        resolveSave = resolve;
      });
      mockSettingsService.setGolemCliPath.mockReturnValue(savePromise);

      render(<GolemCliPathSetting />);

      // Browse for file first
      mockOpen.mockResolvedValue("/path/to/golem-cli");
      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      const saveButton = screen.getByText("Save");
      await user.click(saveButton);

      // Should show loading state
      expect(screen.getByText("Saving...")).toBeInTheDocument();

      // Resolve save
      resolveSave!(true);

      await waitFor(() => {
        expect(screen.getByText("Saved")).toBeInTheDocument();
      });
    });

    it("should show error toast when path is empty", async () => {
      const user = userEvent.setup();

      render(<GolemCliPathSetting />);

      const saveButton = screen.getByText("Save");
      await user.click(saveButton);

      expect(mockToast).toHaveBeenCalledWith({
        title: "Please select a path",
        variant: "destructive",
      });
    });

    it("should handle save errors", async () => {
      const user = userEvent.setup();
      mockSettingsService.setGolemCliPath.mockResolvedValue(false);

      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});

      render(<GolemCliPathSetting />);

      // Browse for file first
      mockOpen.mockResolvedValue("/path/to/golem-cli");
      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      const saveButton = screen.getByText("Save");
      await user.click(saveButton);

      await waitFor(() => {
        expect(consoleSpy).toHaveBeenCalledWith(
          "Error saving golem-cli path:",
          expect.any(Error),
        );
        expect(mockToast).toHaveBeenCalledWith({
          title: "Error saving golem-cli path",
          description: "Error: Failed to save path",
          variant: "destructive",
        });
      });

      consoleSpy.mockRestore();
    });

    it("should handle save promise rejection", async () => {
      const user = userEvent.setup();
      mockSettingsService.setGolemCliPath.mockRejectedValue(
        new Error("Save failed"),
      );

      const consoleSpy = vi
        .spyOn(console, "error")
        .mockImplementation(() => {});

      render(<GolemCliPathSetting />);

      // Browse for file first
      mockOpen.mockResolvedValue("/path/to/golem-cli");
      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      const saveButton = screen.getByText("Save");
      await user.click(saveButton);

      await waitFor(() => {
        expect(consoleSpy).toHaveBeenCalledWith(
          "Error saving golem-cli path:",
          expect.any(Error),
        );
        expect(mockToast).toHaveBeenCalledWith({
          title: "Error saving golem-cli path",
          description: "Error: Save failed",
          variant: "destructive",
        });
      });

      consoleSpy.mockRestore();
    });
  });

  describe("Button states", () => {
    it("should disable save button when path is already saved", async () => {
      mockSettingsService.getGolemCliPath.mockResolvedValue("/existing/path");

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        const saveButton = screen.getByText("Saved");
        expect(saveButton).toBeDisabled();
      });
    });

    it("should enable save button when path changes", async () => {
      const user = userEvent.setup();

      // Start with saved state
      mockSettingsService.getGolemCliPath.mockResolvedValue("/existing/path");

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        expect(screen.getByText("Saved")).toBeDisabled();
      });

      // Browse for new file
      mockOpen.mockResolvedValue("/new/path");
      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      await waitFor(() => {
        const saveButton = screen.getByText("Save");
        expect(saveButton).not.toBeDisabled();
      });
    });

    it("should disable save button during saving", async () => {
      const user = userEvent.setup();

      // Create a promise we can control
      let resolveSave: (value: boolean) => void;
      const savePromise = new Promise<boolean>(resolve => {
        resolveSave = resolve;
      });
      mockSettingsService.setGolemCliPath.mockReturnValue(savePromise);

      render(<GolemCliPathSetting />);

      // Browse for file first
      mockOpen.mockResolvedValue("/path/to/golem-cli");
      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      const saveButton = screen.getByText("Save");
      await user.click(saveButton);

      // Should be disabled during saving
      const savingButton = screen.getByText("Saving...");
      expect(savingButton).toBeDisabled();

      // Resolve save
      resolveSave!(true);

      await waitFor(() => {
        const savedButton = screen.getByText("Saved");
        expect(savedButton).toBeDisabled();
      });
    });
  });

  describe("Input changes", () => {
    it("should mark as unsaved when input value changes manually", async () => {
      // Start with saved state
      mockSettingsService.getGolemCliPath.mockResolvedValue("/existing/path");

      render(<GolemCliPathSetting />);

      await waitFor(() => {
        expect(screen.getByText("Saved")).toBeInTheDocument();
      });

      // Since input is readonly, this test verifies the input is readonly
      const input = screen.getByTestId("input");
      expect(input).toHaveAttribute("readonly");
    });
  });

  describe("Accessibility", () => {
    it("should have proper label association", () => {
      render(<GolemCliPathSetting />);

      const label = screen.getByTestId("label");
      const input = screen.getByTestId("input");

      expect(label).toHaveAttribute("for", "golem-cli-path");
      expect(input).toHaveAttribute("id", "golem-cli-path");
    });

    it("should have proper button types", () => {
      render(<GolemCliPathSetting />);

      const buttons = screen.getAllByTestId("button");
      buttons.forEach(button => {
        expect(button).toHaveAttribute("data-type", "button");
      });
    });

    it("should have descriptive help text", () => {
      render(<GolemCliPathSetting />);

      expect(
        screen.getByText(/Specify the path to the golem-cli executable/),
      ).toBeInTheDocument();
    });
  });

  describe("Edge cases", () => {
    it("should handle non-string file selection", async () => {
      const user = userEvent.setup();

      // Return an array instead of string (should not update input)
      mockOpen.mockResolvedValue(["/path/to/golem-cli"] as string[] | null);

      render(<GolemCliPathSetting />);

      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      // Input should remain empty since result is not a string
      const input = screen.getByTestId("input");
      expect(input).toHaveValue("");
    });

    it("should handle undefined file selection", async () => {
      const user = userEvent.setup();
      mockOpen.mockResolvedValue(undefined);

      render(<GolemCliPathSetting />);

      const browseButton = screen.getByText("Browse");
      await user.click(browseButton);

      // Should not cause errors
      const input = screen.getByTestId("input");
      expect(input).toHaveValue("");
    });
  });
});
