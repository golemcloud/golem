import type { Meta, StoryObj } from "@storybook/react-vite";
import { NavMain, SidebarMenuProps } from "./nav-main";
import { fn } from "storybook/test";
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
    (Story) => (
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
};

export const WithActiveItem: Story = {
  args: {
    items: menuItems,
    activeItem: "Components",
  },
};
