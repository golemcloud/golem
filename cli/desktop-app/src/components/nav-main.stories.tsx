import type { Meta, StoryObj } from "@storybook/react-vite";
import { NavMain, SidebarMenuProps } from "./nav-main";
import { fn, userEvent, within, expect } from "storybook/test";
import { SidebarProvider } from "@/components/ui/sidebar";
import {
  LayoutDashboard,
  Box,
  Globe,
  Rocket,
  Settings,
  Puzzle,
} from "lucide-react";

const meta = {
  title: "Components/NavMain",
  component: NavMain,
  decorators: [
    Story => (
      <SidebarProvider>
        <div className="w-64">
          <Story />
        </div>
      </SidebarProvider>
    ),
  ],
  args: {
    setActiveItem: fn(),
  },
} satisfies Meta<typeof NavMain>;

export default meta;
type Story = StoryObj<typeof meta>;

const menuItems: SidebarMenuProps[] = [
  { title: "Dashboard", url: "/app/my-app/dashboard", icon: LayoutDashboard },
  { title: "Components", url: "/app/my-app/components", icon: Box },
  { title: "APIs", url: "/app/my-app/apis", icon: Globe },
  { title: "Deployments", url: "/app/my-app/deployments", icon: Rocket },
  { title: "Plugins", url: "/app/my-app/plugins", icon: Puzzle },
  { title: "Settings", url: "/settings", icon: Settings },
];

export const Default: Story = {
  args: {
    items: menuItems,
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Verify all 6 menu items are visible
    await expect(canvas.getByText("Dashboard")).toBeInTheDocument();
    await expect(canvas.getByText("Components")).toBeInTheDocument();
    await expect(canvas.getByText("APIs")).toBeInTheDocument();
    await expect(canvas.getByText("Deployments")).toBeInTheDocument();
    await expect(canvas.getByText("Plugins")).toBeInTheDocument();
    await expect(canvas.getByText("Settings")).toBeInTheDocument();

    // Click "Components" -> assert setActiveItem("Components")
    await userEvent.click(canvas.getByText("Components"));
    await expect(args.setActiveItem).toHaveBeenCalledWith("Components");
  },
};

export const WithSubItems: Story = {
  args: {
    items: [
      {
        title: "Components",
        url: "/app/my-app/components",
        icon: Box,
        items: [
          {
            title: "shopping-cart",
            url: "/app/my-app/components/shopping-cart",
          },
          {
            title: "auth-service",
            url: "/app/my-app/components/auth-service",
          },
          {
            title: "email-sender",
            url: "/app/my-app/components/email-sender",
          },
        ],
      },
      { title: "APIs", url: "/app/my-app/apis", icon: Globe },
      { title: "Deployments", url: "/app/my-app/deployments", icon: Rocket },
    ],
  },
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Click "Components" to expand the collapsible
    await userEvent.click(canvas.getByText("Components"));

    // Verify sub-items are visible
    await expect(canvas.getByText("shopping-cart")).toBeInTheDocument();
    await expect(canvas.getByText("auth-service")).toBeInTheDocument();
    await expect(canvas.getByText("email-sender")).toBeInTheDocument();

    // Click a sub-item -> assert callback
    await userEvent.click(canvas.getByText("shopping-cart"));
    await expect(args.setActiveItem).toHaveBeenCalledWith("shopping-cart");
  },
};

export const WithActiveItem: Story = {
  args: {
    items: menuItems,
    activeItem: "Components",
  },
};
