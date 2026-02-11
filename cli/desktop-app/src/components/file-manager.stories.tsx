import type { Meta, StoryObj } from "@storybook/react-vite";
import { FolderStructure } from "./file-manager";
import { FileStructure } from "@/types/component";
import { within, expect, userEvent } from "storybook/test";

const meta = {
  title: "Components/FolderStructure",
  component: FolderStructure,
} satisfies Meta<typeof FolderStructure>;

export default meta;
type Story = StoryObj<typeof meta>;

const sampleFiles: FileStructure[] = [
  { key: "1", path: "/src/main.rs", permissions: "rw-r--r--" },
  { key: "2", path: "/src/lib.rs", permissions: "rw-r--r--" },
  { key: "3", path: "/src/handlers/auth.rs", permissions: "rw-r--r--" },
  { key: "4", path: "/src/handlers/cart.rs", permissions: "rw-r--r--" },
  { key: "5", path: "/src/models/user.rs", permissions: "rw-r--r--" },
  { key: "6", path: "/src/models/product.rs", permissions: "rw-r--r--" },
  { key: "7", path: "/Cargo.toml", permissions: "rw-r--r--" },
  { key: "8", path: "/wit/main.wit", permissions: "r--r--r--" },
];

export const WithFiles: Story = {
  args: {
    data: sampleFiles,
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Verify folder structure is visible
    await expect(canvas.getByText("src")).toBeInTheDocument();

    // Collapse the "src" folder by clicking its trigger
    const srcFolder = canvas.getByText("src");
    await userEvent.click(srcFolder);

    // Re-expand
    await userEvent.click(srcFolder);

    // Verify children are visible again
    await expect(canvas.getByText("main.rs")).toBeInTheDocument();
  },
};

export const Empty: Story = {
  args: {
    data: [],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(canvas.getByText("No files found")).toBeInTheDocument();
  },
};
