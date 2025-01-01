import { useEffect, useState } from "react";
import { useParams, useNavigate } from "react-router-dom";
import { useToast } from "@/hooks/use-toast";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import APILeftNav from "./apiLeftNav";
import { SERVICE } from "@/service";
import { Api } from "@/types/api";

export default function APISettings() {
  const { toast } = useToast();
  const navigate = useNavigate();
  const [showConfirmDialog, setShowConfirmDialog] = useState(false);
  const [showConfirmAllDialog, setShowConfirmAllDialog] = useState(false);
  const [isDeleting, setIsDeleting] = useState(false);
  const { apiName } = useParams();
  const [apiDetails, setApiDetails] = useState([] as Api[]);
  const [activeApiDetails, setActiveApiDetails] = useState({} as Api);

  useEffect(() => {
    if (apiName) {
      SERVICE.getApi(apiName).then((response) => {
        setApiDetails(response);
        setActiveApiDetails(response[response.length - 1]);
      });
    }
  }, [apiName]);

  const handleDeleteVersion = async () => {
    setIsDeleting(true);
    SERVICE.deleteApi(activeApiDetails.id, activeApiDetails.version)
      .then(() => {
        toast({
          title: "Version deleted",
          description: `API version ${activeApiDetails.version} has been deleted successfully.`,
        });
        if (apiDetails.length === 1) {
          navigate(`/apis`);
        } else {
          setApiDetails(
            apiDetails.filter((api) => api.version !== activeApiDetails.version)
          );
        }
        setShowConfirmDialog(false);
        setIsDeleting(false);
      })
      .catch((error) => {
        console.log(error);
        setIsDeleting(false);
        toast({
          variant: "destructive",
          title: "Error",
          description: "Failed to delete the API version. Please try again.",
        });
      });
  };

  const handleDeleteAll = async () => {
    setIsDeleting(true);
    const promises = apiDetails.map((api) =>
      SERVICE.deleteApi(api.id, api.version)
    );
    Promise.all(promises)
      .then(() => {
        toast({
          title: "All versions deleted",
          description: "All API versions have been deleted successfully.",
        });
        setShowConfirmAllDialog(false);
        navigate(`/apis`);
      })
      .catch((error) => {
        console.log(error);
        toast({
          variant: "destructive",
          title: "Error",
          description: "Failed to delete all API versions. Please try again.",
        });
        setIsDeleting(false);
      });
  };

  const handleDeleteAllRoutes = async () => {
    const payload = {
      ...activeApiDetails,
      routes: [],
    };
    SERVICE.putApi(activeApiDetails.id, activeApiDetails.version, payload)
      .then(() => {
        toast({
          title: "All routes deleted",
          description: "All routes have been deleted successfully.",
        });
        setShowConfirmAllDialog(false);
        navigate(`/apis/${apiName}`);
      })
      .catch((error) => {
        console.log(error);
        toast({
          variant: "destructive",
          title: "Error",
          description:
            "Failed to delete all routes versions. Please try again.",
        });
      });
  };

  return (
    <div className="flex">
      <APILeftNav />
      <div className="flex-1">
        <div className="flex items-center justify-between">
          <header className="w-full border-b bg-background py-2">
            <div className="max-w-7xl px-6 lg:px-8">
              <div className="mx-auto max-w-2xl lg:max-w-none">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <h1 className="line-clamp-1 font-medium leading-tight sm:leading-normal">
                      {apiName}
                    </h1>
                    <div className="flex items-center gap-1">
                      {activeApiDetails.version && (
                        <Select
                          defaultValue={activeApiDetails.version}
                          onValueChange={(version) => {
                            const selectedApi = apiDetails.find(
                              (api) => api.version === version
                            );
                            if (selectedApi) {
                              setActiveApiDetails(selectedApi);
                            }
                          }}
                        >
                          <SelectTrigger className="w-20 h-6">
                            <SelectValue>
                              {activeApiDetails.version}
                            </SelectValue>
                          </SelectTrigger>
                          <SelectContent>
                            {apiDetails.map((api) => (
                              <SelectItem value={api.version} key={api.version}>
                                {api.version}{" "}
                                {api.draft ? "(Draft)" : "(Published)"}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                      )}
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </header>
        </div>
        <div className=" overflow-scroll h-[85vh] max-w-4xl mx-auto p-8">
          <h1 className="text-3xl font-semibold mb-2">API Settings</h1>
          <p className="text-gray-500 text-lg mb-8">Manage your API settings</p>

          <div className="border border-red-100 rounded-lg bg-red-50/50 p-6">
            <h2 className="text-2xl font-semibold mb-4">Danger Zone</h2>
            <p className="text-gray-600 mb-8">Proceed with caution.</p>

            <div className="space-y-8">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-xl font-semibold mb-2">
                    Delete API Version {activeApiDetails.version}
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
                  Delete Version {activeApiDetails.version}
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
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="text-xl font-semibold mb-2">
                    Delete All Routes
                  </h3>
                  <p className="text-gray-600">
                    Once you delete all routes, there is no going back. Please
                    be certain.
                  </p>
                </div>
                <Button
                  variant="outline"
                  className="border-red-200 text-red-700 hover:bg-red-50 hover:text-red-800"
                  onClick={handleDeleteAllRoutes}
                >
                  Delete All Routes
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
                  version {activeApiDetails.version}.
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
