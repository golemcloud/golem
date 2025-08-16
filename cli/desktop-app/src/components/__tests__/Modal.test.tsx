import { render, screen, fireEvent } from "@testing-library/react";
import Modal from "../Modal";

// Mock lucide-react
vi.mock("lucide-react", () => ({
  X: () => <div data-testid="close-icon">×</div>,
}));

describe("Modal", () => {
  const mockOnClose = vi.fn();

  beforeEach(() => {
    mockOnClose.mockClear();
  });

  it("renders modal content when isOpen is true", () => {
    render(
      <Modal isOpen={true} onClose={mockOnClose}>
        <div>
          <h2>Test Modal</h2>
          <p>Modal content</p>
        </div>
      </Modal>,
    );

    expect(screen.getByText("Test Modal")).toBeInTheDocument();
    expect(screen.getByText("Modal content")).toBeInTheDocument();
  });

  it("does not render modal content when isOpen is false", () => {
    render(
      <Modal isOpen={false} onClose={mockOnClose}>
        <div>
          <h2>Test Modal</h2>
          <p>Modal content</p>
        </div>
      </Modal>,
    );

    expect(screen.queryByText("Test Modal")).not.toBeInTheDocument();
    expect(screen.queryByText("Modal content")).not.toBeInTheDocument();
  });

  it("calls onClose when close button is clicked", () => {
    render(
      <Modal isOpen={true} onClose={mockOnClose}>
        <div>
          <h2>Test Modal</h2>
          <p>Modal content</p>
        </div>
      </Modal>,
    );

    const closeButton = screen.getByTestId("close-icon")
      .parentElement as HTMLElement;
    fireEvent.click(closeButton);

    expect(mockOnClose).toHaveBeenCalledTimes(1);
  });

  it("calls onClose when overlay is clicked", () => {
    render(
      <Modal isOpen={true} onClose={mockOnClose}>
        <div>
          <h2>Test Modal</h2>
          <p>Modal content</p>
        </div>
      </Modal>,
    );

    // Find the overlay element (fixed background)
    const overlay = document.querySelector(
      ".bg-black.bg-opacity-25",
    ) as HTMLElement;
    fireEvent.click(overlay);

    expect(mockOnClose).toHaveBeenCalledTimes(1);
  });
});
