import type { Meta, StoryObj } from "@storybook/react-vite";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
  DialogClose,
} from "./dialog";
import { Button } from "./button";
import { Input } from "./input";
import { Label } from "./label";
import { userEvent, expect, screen } from "storybook/test";

const meta = {
  title: "UI/Dialog",
  component: Dialog,
  parameters: {
    skipGlobalRouter: true,
  },
} satisfies Meta<typeof Dialog>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <Dialog>
      <DialogTrigger asChild>
        <Button variant="outline">Open Dialog</Button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Dialog Title</DialogTitle>
          <DialogDescription>
            This is a description of the dialog content and what it does.
          </DialogDescription>
        </DialogHeader>
        <p className="text-sm text-muted-foreground">
          Dialog body content goes here.
        </p>
      </DialogContent>
    </Dialog>
  ),
  play: async ({ canvasElement }) => {
    const button = canvasElement.querySelector("button");
    if (button) {
      await userEvent.click(button);
    }
    const title = await screen.findByText("Dialog Title");
    await expect(title).toBeInTheDocument();
  },
};

export const WithForm: Story = {
  render: () => (
    <Dialog defaultOpen>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit Profile</DialogTitle>
          <DialogDescription>
            Make changes to your profile here. Click save when you&apos;re done.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-4 py-4">
          <div className="grid grid-cols-4 items-center gap-4">
            <Label htmlFor="dialog-name" className="text-right">
              Name
            </Label>
            <Input
              id="dialog-name"
              defaultValue="my-component"
              className="col-span-3"
            />
          </div>
          <div className="grid grid-cols-4 items-center gap-4">
            <Label htmlFor="dialog-url" className="text-right">
              URL
            </Label>
            <Input
              id="dialog-url"
              defaultValue="http://localhost:9881"
              className="col-span-3"
            />
          </div>
        </div>
        <DialogFooter>
          <DialogClose asChild>
            <Button variant="outline">Cancel</Button>
          </DialogClose>
          <Button type="submit">Save changes</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  ),
  play: async () => {
    const title = await screen.findByText("Edit Profile");
    await expect(title).toBeInTheDocument();

    const nameInput = screen.getByDisplayValue("my-component");
    await expect(nameInput).toBeInTheDocument();

    await userEvent.clear(nameInput);
    await userEvent.type(nameInput, "new-component");
    await expect(nameInput).toHaveValue("new-component");
  },
};

export const Confirmation: Story = {
  render: () => (
    <Dialog defaultOpen>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Are you sure?</DialogTitle>
          <DialogDescription>
            This action cannot be undone. This will permanently delete the
            component and all associated workers.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <DialogClose asChild>
            <Button variant="outline">Cancel</Button>
          </DialogClose>
          <Button variant="destructive">Delete</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  ),
  play: async () => {
    const title = await screen.findByText("Are you sure?");
    await expect(title).toBeInTheDocument();

    await expect(screen.getByText("Cancel")).toBeInTheDocument();
    await expect(screen.getByText("Delete")).toBeInTheDocument();
  },
};
