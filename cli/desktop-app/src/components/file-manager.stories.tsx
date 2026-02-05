import type { Meta, StoryObj } from "@storybook/react-vite";
import { FolderStructure } from "./file-manager";
import { FileStructure } from "@/types/component";

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
};

export const Empty: Story = {
  args: {
    data: [],
  },
};
