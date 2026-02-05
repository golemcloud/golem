import type { Meta, StoryObj } from "@storybook/react-vite";
import { SidebarMenu } from "./sidebar";
import { fn } from "storybook/test";
import { SidebarProvider } from "@/components/ui/sidebar";
import {
  LayoutDashboard,
  Box,
  Globe,
  Rocket,
  Puzzle,
  Settings,
} from "lucide-react";
import type { SidebarMenuProps as MenuProps } from "./nav-main";

const meta = {
  title: "Components/SidebarMenu",
  component: SidebarMenu,
  decorators: [
    (Story) => (
      <SidebarProvider>
        <Story />
      </SidebarProvider>
    ),
  ],
  args: {
    setActiveItem: fn(),
  },
} satisfies Meta<typeof SidebarMenu>;

export default meta;
type Story = StoryObj<typeof meta>;

const fullMenus: MenuProps[] = [
  { title: "Dashboard", url: "/app/my-app/dashboard", icon: LayoutDashboard },
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
    ],
  },
  { title: "APIs", url: "/app/my-app/apis", icon: Globe },
  { title: "Deployments", url: "/app/my-app/deployments", icon: Rocket },
  { title: "Plugins", url: "/app/my-app/plugins", icon: Puzzle },
  { title: "Settings", url: "/settings", icon: Settings },
];

export const Default: Story = {
  args: {
    menus: fullMenus,
  },
};

export const WithActiveItem: Story = {
  args: {
    menus: fullMenus,
    activeItem: "Components",
  },
};

export const Collapsed: Story = {
  args: {
    menus: fullMenus,
    collapsible: "icon",
  },
};
