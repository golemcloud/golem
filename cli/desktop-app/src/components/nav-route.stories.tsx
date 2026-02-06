import type { Meta, StoryObj } from "@storybook/react-vite";
import { NavRoutes } from "./nav-route";
import { fn, userEvent, within, expect } from "storybook/test";
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
  play: async ({ canvasElement, args }) => {
    const canvas = within(canvasElement);

    // Verify route items and HTTP method badges
    const getBadges = canvas.getAllByText("Get");
    await expect(getBadges.length).toBeGreaterThan(0);
    await expect(canvas.getByText("Post")).toBeInTheDocument();
    await expect(canvas.getByText("Delete")).toBeInTheDocument();
    await expect(canvas.getByText("/api/health")).toBeInTheDocument();

    // Click a route -> assert setActiveItem
    await userEvent.click(canvas.getByText("/api/health"));
    await expect(args.setActiveItem).toHaveBeenCalledWith("/api/health");
  },
};

export const WithActiveRoute: Story = {
  args: {
    routes: sampleRoutes,
    activeItem: "/api/products/{id}",
  },
};
