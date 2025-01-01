/* eslint-disable @typescript-eslint/no-unused-vars */
import { useEffect, useState } from "react";
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
import APILeftNav from "./APILeftNav";
import { invoke } from "@tauri-apps/api/core";

const ApiMockData = [
  {
    createdAt: "2024-12-31T15:55:12.838362+00:00",
    draft: true,
    id: "great",
    routes: [],
    version: "0.2.0",
  },
  {
    createdAt: "2024-12-31T05:34:20.197542+00:00",
    draft: false,
    id: "vvvvv",
    routes: [],
    version: "0.1.0",
  },
];

const ApiDetailsMock = ApiMockData[0];

export default function APISettings() {
  const { toast } = useToast();
  const [showConfirmDialog, setShowConfirmDialog] = useState(false);
  const [showConfirmAllDialog, setShowConfirmAllDialog] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const { apiName } = useParams();
  const [apiDetails, setApiDetails] = useState(ApiDetailsMock);

  useEffect(() => {
    const fetchData = async () => {
      //check the api https://release.api.golem.cloud/v1/api/definitions/305e832c-f7c1-4da6-babc-cb2422e0f5aa
      // eslint-disable-next-line @typescript-eslint/no-explicit-any, @typescript-eslint/no-unused-vars
      const response: any = await invoke("get_api");
      const apiData = ApiMockData.find((api) => api.id === apiName);
      if (apiData) {
        setApiDetails(apiData);
      } else {
        setApiDetails(ApiDetailsMock); // or handle the undefined case as needed
      }
    };
    fetchData().then((r) => r);
  }, []);

  const handleDeleteVersion = async () => {
    setIsDeleting(true);
    try {
      // Simulate API call
      //https://release.api.golem.cloud/v1/api/definitions/305e832c-f7c1-4da6-babc-cb2422e0f5aa/vvvvv/0.1.0
      //Delete method
      await new Promise((resolve) => setTimeout(resolve, 1000));

      toast({
        title: "Version deleted",
        description: `API version ${apiDetails.version} has been deleted successfully.`,
      });
      setShowConfirmDialog(false);
    } catch (error) {
      // Simulate API call
      //https://release.api.golem.cloud/v1/api/definitions/305e832c-f7c1-4da6-babc-cb2422e0f5aa/vvvvv/0.1.0
      toast({
        variant: "destructive",
        title: "Error",
        description: "Failed to delete the API version. Please try again.",
      });
    } finally {
      setIsDeleting(false);
    }
  };

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
      <APILeftNav />
      <div className="flex-1">
        <div className="flex items-center justify-between">
          <header className="w-full border-b bg-background py-2">
            <div className="mx-auto max-w-7xl px-6 lg:px-8">
              <div className="mx-auto max-w-2xl lg:max-w-none">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <h1 className="line-clamp-1 font-medium leading-tight sm:leading-normal">
                      {apiName}
                    </h1>
                    <div className="flex items-center gap-1">
                      <div className="inline-flex items-center rounded-md px-2.5 py-0.5 text-xs font-semibold focus:outline-none bg-primary-background text-primary-soft  border border-primary-border w-fit font-mono">
                        {apiDetails?.version}
                      </div>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </header>
        </div>
        <div className="max-w-4xl mx-auto p-8">
          <h1 className="text-3xl font-semibold mb-2">API Settings</h1>
          <p className="text-gray-500 text-lg mb-8">Manage your API settings</p>

          <div className="border border-red-100 rounded-lg bg-red-50/50 p-6">
            <h2 className="text-2xl font-semibold mb-4">Danger Zone</h2>
            <p className="text-gray-600 mb-8">Proceed with caution.</p>

            <div className="space-y-8">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-xl font-semibold mb-2">
                    Delete API Version {apiDetails.version}
                  </h3>
                  <p className="text-gray-600">
                    Once you delete an API, there is no going back. Please be
                    certain.
                  </p>
                </div>
                <Button
                  variant="outline"
                  className="border-red-200 text-red-700 hover:bg-red-50 hover:text-red-800"
                  onClick={() => setShowConfirmDialog(true)}
                >
                  Delete Version {apiDetails.version}
                </Button>
              </div>

              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-xl font-semibold mb-2">
                    Delete all API Versions
                  </h3>
                  <p className="text-gray-600">
                    Once you delete all API versions, there is no going back.
                    Please be certain.
                  </p>
                </div>
                <Button
                  variant="outline"
                  className="border-red-200 text-red-700 hover:bg-red-50 hover:text-red-800"
                  onClick={() => setShowConfirmAllDialog(true)}
                >
                  Delete All Versions
                </Button>
              </div>
            </div>
          </div>

          {/* Confirmation Dialog for Single Version Delete */}
          <Dialog open={showConfirmDialog} onOpenChange={setShowConfirmDialog}>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Are you sure you want to delete?</DialogTitle>
                <DialogDescription>
                  This action cannot be undone. This will permanently delete API
                  version {apiDetails.version}.
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <Button
                  variant="outline"
                  onClick={() => setShowConfirmDialog(false)}
                  disabled={isDeleting}
                >
                  Cancel
                </Button>
                <Button
                  variant="destructive"
                  onClick={handleDeleteVersion}
                  disabled={isDeleting}
                >
                  {isDeleting ? "Deleting..." : "Yes, delete"}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>

          {/* Confirmation Dialog for Delete All */}
          <Dialog
            open={showConfirmAllDialog}
            onOpenChange={setShowConfirmAllDialog}
          >
            <DialogContent>
              <DialogHeader>
                <DialogTitle>
                  Are you sure you want to delete all versions?
                </DialogTitle>
                <DialogDescription>
                  This action cannot be undone. This will permanently delete all
                  API versions and remove all associated data.
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
