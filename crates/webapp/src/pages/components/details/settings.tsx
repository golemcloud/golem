import * as React from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useToast } from "@/hooks/use-toast";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import ComponentLeftNav from "./componentsLeftNav";
import {
  Card,
  CardContent,
  CardDescription,
  CardTitle,
} from "@/components/ui/card.tsx";
import { Separator } from "@/components/ui/separator.tsx";
import { API } from "@/service";
import { Worker } from "@/types/worker.ts";
import ErrorBoundary from "@/components/errorBoundary";

export default function ComponentSettings() {
  const { toast } = useToast();
  const [showConfirmAllDialog, setShowConfirmAllDialog] = React.useState(false);
  const [isDeleting, setIsDeleting] = React.useState(false);
  const { componentId } = useParams();
  const navigate = useNavigate();

  const handleDeleteAll = async () => {
    setIsDeleting(true);
    const response = await API.findWorker(componentId!, {
      count: 100,
      precise: true,
    });
    await Promise.all(
      response?.workers.map(async (worker: Worker) => {
        await API.deleteWorker(componentId!, worker.workerId.workerName).then(
          () => {}
        );
      })
    );
    toast({
      title: "All versions deleted",
      description: "All API versions have been deleted successfully.",
    });
    navigate(`/components`);
  };

  return (
    <ErrorBoundary>
      <div className="flex">
        <ComponentLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {componentId}
                </h1>
              </div>
            </div>
          </header>
          <div className="flex-1 p-8">
            <Card className="max-w-4xl mx-auto border-0 shadow-none">
              <CardTitle>
                <h1 className="text-2xl font-semibold mb-1">
                  General Settings
                </h1>
              </CardTitle>
              <CardDescription>
                <p className="text-sm ">Manage your component settings</p>
              </CardDescription>
              <Separator className="my-4" />
              <CardContent className="py-6 px-0">
                <Card className="border border-red-100 bg-red-50/50 rounded-lg  p-6">
                  <CardTitle>
                    <h1 className="text-2xl font-semibold mb-1">Danger Zone</h1>
                  </CardTitle>
                  <CardDescription>
                    <p className="text-sm ">Proceed with caution.</p>
                  </CardDescription>
                  <Separator className="my-4" />
                  <CardContent className="px-0 py-2">
                    <div className="flex items-center justify-between">
                      <div>
                        <h3 className="text-xl font-semibold mb-2">
                          Delete All Workers
                        </h3>
                        <p className="text-gray-600">
                          This will permanently delete all workers associated
                          with this component.
                        </p>
                      </div>
                      <Dialog
                        open={showConfirmAllDialog}
                        onOpenChange={setShowConfirmAllDialog}
                      >
                        <DialogTrigger asChild>
                          <Button
                            variant="outline"
                            className="border-red-200 text-red-700 hover:bg-red-50 hover:text-red-800"
                          >
                            Delete all Workers
                          </Button>
                        </DialogTrigger>
                        <DialogContent>
                          <DialogHeader>
                            <DialogTitle>
                              Are you sure you want to delete all workers?
                            </DialogTitle>
                            <DialogDescription>
                              This action cannot be undone. This will
                              permanently delete all the workers associated with
                              this component.
                            </DialogDescription>
                          </DialogHeader>
                          <DialogFooter>
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
                  </CardContent>
                </Card>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}
