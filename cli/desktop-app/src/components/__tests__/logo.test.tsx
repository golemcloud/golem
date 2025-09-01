import { render } from "@testing-library/react";
import { Logo } from "../logo";

describe("Logo", () => {
  it("renders the logo component", () => {
    render(<Logo />);

    const logoSvg = document.querySelector("svg.logo-light");
    expect(logoSvg).toBeInTheDocument();
  });

  it("has the correct viewBox attribute", () => {
    render(<Logo />);

    const logoSvg = document.querySelector("svg.logo-light");
    expect(logoSvg).toHaveAttribute("viewBox", "0 0 114.01 113.5");
  });

  it("has the correct CSS classes", () => {
    render(<Logo />);

    const logoSvg = document.querySelector("svg.logo-light");
    expect(logoSvg).toHaveClass(
      "logo-light",
      "fill-foreground",
      "stroke-foreground",
      "h-8",
      "w-8",
    );
  });
});
