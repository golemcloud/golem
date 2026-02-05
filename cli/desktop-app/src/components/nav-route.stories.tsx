import type { Meta, StoryObj } from "@storybook/react-vite";
import { NavRoutes } from "./nav-route";
import { fn } from "storybook/test";
import { SidebarProvider } from "@/components/ui/sidebar";

const meta = {
  title: "Components/NavRoutes",
  component: NavRoutes,
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
} satisfies Meta<typeof NavRoutes>;

export default meta;
type Story = StoryObj<typeof meta>;

const sampleRoutes = [
  { method: "Get", name: "/api/products", url: "/app/my-app/apis/products" },
  {
    method: "Post",
    name: "/api/products",
    url: "/app/my-app/apis/products/create",
  },
  {
    method: "Get",
    name: "/api/products/{id}",
    url: "/app/my-app/apis/products/detail",
  },
  {
    method: "Put",
    name: "/api/products/{id}",
    url: "/app/my-app/apis/products/update",
  },
  {
    method: "Delete",
    name: "/api/products/{id}",
    url: "/app/my-app/apis/products/delete",
  },
  { method: "Get", name: "/api/health", url: "/app/my-app/apis/health" },
  { method: "Patch", name: "/api/cart", url: "/app/my-app/apis/cart" },
];

export const Default: Story = {
  args: {
    routes: sampleRoutes,
    activeItem: "",
  },
};

export const WithActiveRoute: Story = {
  args: {
    routes: sampleRoutes,
    activeItem: "/api/products/{id}",
  },
};
