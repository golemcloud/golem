import type { Meta, StoryObj } from "@storybook/react-vite";
import Navbar from "./navbar";
import { within, expect, userEvent } from "storybook/test";

const meta = {
  title: "Components/Navbar",
  component: Navbar,
} satisfies Meta<typeof Navbar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithNavigation: Story = {
  args: {
    showNav: true,
  },
  parameters: {
    router: {
      route: "/app/my-shopping-app/dashboard",
      path: "/app/:appId/*",
    },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Verify all 6 nav links
    await expect(canvas.getByText("Dashboard")).toBeInTheDocument();
    await expect(canvas.getByText("Components")).toBeInTheDocument();
    await expect(canvas.getByText("APIs")).toBeInTheDocument();
    await expect(canvas.getByText("Deployments")).toBeInTheDocument();
    await expect(canvas.getByText("Environments")).toBeInTheDocument();
    await expect(canvas.getByText("Plugins")).toBeInTheDocument();

    // Verify logo SVG is present
    const logoSvg = canvasElement.querySelector("svg.logo-light");
    await expect(logoSvg).toBeInTheDocument();

    // Verify theme toggle button
    await expect(
      canvas.getByRole("button", { name: /toggle theme/i }),
    ).toBeInTheDocument();

    // Click a nav link
    await userEvent.click(canvas.getByText("Components"));
  },
};

export const WithoutNavigation: Story = {
  args: {
    showNav: false,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Verify nav links are absent
    expect(canvas.queryByText("Dashboard")).not.toBeInTheDocument();
    expect(canvas.queryByText("Components")).not.toBeInTheDocument();

    // Logo and toggle still present
    const logoSvg = canvasElement.querySelector("svg.logo-light");
    await expect(logoSvg).toBeInTheDocument();
    await expect(
      canvas.getByRole("button", { name: /toggle theme/i }),
    ).toBeInTheDocument();
  },
};

export const NoAppId: Story = {
  args: {
    showNav: true,
  },
};
