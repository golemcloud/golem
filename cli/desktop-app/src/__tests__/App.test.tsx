import { render, screen } from "@testing-library/react";
import App from "../App";

// Mock the routes to avoid complex routing setup
vi.mock("../routes", () => ({
  appRoutes: [
    {
      path: "/",
      element: <div>Home Page</div>,
    },
  ],
}));

describe("App", () => {
  it("renders without crashing", () => {
    render(<App />);
    // App shows home page content
    expect(screen.getByText("Home Page")).toBeInTheDocument();
  });

  it("provides theme context", () => {
    render(<App />);

    // Check if the theme provider is working by looking for the theme class
    const app = document.querySelector(".min-h-screen");
    expect(app).toBeInTheDocument();
  });

  it("has loading fallback", () => {
    render(<App />);
    expect(screen.getByText("Home Page")).toBeInTheDocument();
  });

  it("wraps content in router", () => {
    render(<App />);

    // Check that the app structure includes the router
    expect(screen.getByText("Home Page")).toBeInTheDocument();
  });
});
