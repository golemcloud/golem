/* eslint-disable @typescript-eslint/no-unused-vars */
import * as React from "react";
import { useParams } from "react-router-dom";
import { useToast } from "@/hooks/use-toast";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import ComponentLeftNav from "./componentsLeftNav";

export default function ComponentSettings() {
  const { toast } = useToast();
  const [showConfirmAllDialog, setShowConfirmAllDialog] = React.useState(false);
  const [isDeleting, setIsDeleting] = React.useState(false);
  const { componentId } = useParams();

  const handleDeleteAll = async () => {
    setIsDeleting(true);
    try {
      // Simulate API call
      await new Promise((resolve) => setTimeout(resolve, 1000));

      toast({
        title: "All versions deleted",
        description: "All API versions have been deleted successfully.",
      });
      setShowConfirmAllDialog(false);
    } catch (error) {
      toast({
        variant: "destructive",
        title: "Error",
        description: "Failed to delete all API versions. Please try again.",
      });
    } finally {
      setIsDeleting(false);
    }
  };

  return (
    <div className="flex">
      <ComponentLeftNav />
      <div className="flex-1 p-8">
        <div className="flex items-center justify-between mb-8">
          <div className="grid grid-cols-2 gap-4">
            <h1 className="text-2xl font-semibold mb-2">{componentId}</h1>
            <div className="flex items-center gap-2">
              <span className="inline-flex items-center rounded-md px-2.5 py-0.5 text-xs font-semibold transition-colors focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 bg-primary-background text-primary-soft hover:bg-primary/50 active:bg-primary/50 border border-primary-border w-fit font-mono">
                0.1.0
              </span>
            </div>
          </div>
        </div>
        <div className="max-w-4xl mx-auto p-6">
          <h1 className="text-3xl font-semibold mb-2">General Settings</h1>
          <p className="text-gray-500 text-lg mb-8">
            Manage your component settings
          </p>

          <div className="border border-red-100 rounded-lg bg-red-50/50 p-6">
            <h2 className="text-2xl font-semibold mb-4">Danger Zone</h2>
            <p className="text-gray-600 mb-8">Proceed with caution.</p>

            <div className="space-y-8">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-xl font-semibold mb-2">
                    Delete All Workers
                  </h3>
                  <p className="text-gray-600">
                    This will permanently delete all workers associated with
                    this component.
                  </p>
                </div>
                <Button
                  variant="outline"
                  className="border-red-200 text-red-700 hover:bg-red-50 hover:text-red-800"
                  onClick={() => setShowConfirmAllDialog(true)}
                >
                  Delete all Workers
                </Button>
              </div>
            </div>
          </div>

          <Dialog
            open={showConfirmAllDialog}
            onOpenChange={setShowConfirmAllDialog}
          >
            <DialogContent>
              <DialogHeader>
                <DialogTitle>
                  Are you sure you want to delete all workers?
                </DialogTitle>
                <DialogDescription>
                  This action cannot be undone. This will permanently delete all
                  the workers associated with this component.
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <Button
                  variant="outline"
                  onClick={() => setShowConfirmAllDialog(false)}
                  disabled={isDeleting}
                >
                  Cancel
                </Button>
                <Button
                  variant="destructive"
                  onClick={handleDeleteAll}
                  disabled={isDeleting}
                >
                  {isDeleting ? "Deleting..." : "Yes, delete all"}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </div>
      </div>
    </div>
  );
}
