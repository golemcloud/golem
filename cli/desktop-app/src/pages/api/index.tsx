import { useEffect, useState } from "react";
import {
  GitBranch,
  Layers,
  Lock as LockIcon,
  Plus,
  Search,
  Upload,
  CheckCircle2,
} from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { HttpApiDefinition } from "@/types/golemManifest";
import { API } from "@/service";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import ErrorBoundary from "@/components/errorBoundary";
import { Badge } from "@/components/ui/badge.tsx";
import { removeDuplicateApis } from "@/lib/utils";
import { useToast } from "@/hooks/use-toast";

export const APIs = () => {
  const navigate = useNavigate();
  const { toast } = useToast();
  const [apis, setApis] = useState(
    [] as (HttpApiDefinition & { count?: number; isUploaded?: boolean })[],
  );
  const [searchedApi, setSearchedApi] = useState(
    [] as (HttpApiDefinition & { count?: number; isUploaded?: boolean })[],
  );
  const [uploadingApi, setUploadingApi] = useState<string | null>(null);
  const { appId } = useParams<{ appId: string }>();

  const fetchApis = async () => {
    try {
      const [localApis, uploadedApis] = await Promise.all([
        API.apiService.getApiList(appId!),
        API.apiService.getUploadedDefinitions(appId!),
      ]);

      // Mark APIs as uploaded if they exist in uploaded list
      const newData = removeDuplicateApis(localApis).map(api => ({
        ...api,
        isUploaded: uploadedApis.some(
          uploaded =>
            uploaded.id === api.id && uploaded.version === api.version,
        ),
      }));

      setApis(newData);
      setSearchedApi(newData);
    } catch (error) {
      console.error("Failed to fetch APIs:", error);
    }
  };

  useEffect(() => {
    fetchApis();
  }, [appId]);

  const handleUpload = async (apiId: string, version: string) => {
    try {
      setUploadingApi(`${apiId}-${version}`);
      await API.apiService.deployDefinition(appId!, apiId);
      toast({
        title: "API Definition Uploaded",
        description: `${apiId} v${version} has been uploaded successfully`,
        duration: 3000,
      });
      // Refresh the list to update upload status
      await fetchApis();
    } catch (error) {
      console.error("Failed to upload API definition:", error);
      toast({
        title: "Upload Failed",
        description: `Failed to upload ${apiId}. Please try again.`,
        variant: "destructive",
        duration: 3000,
      });
    } finally {
      setUploadingApi(null);
    }
  };

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-6 py-10">
        <div className="flex items-center justify-between gap-4 mb-8">
          <div className="relative flex-1">
            <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-muted-foreground h-5 w-5" />
            <Input
              type="text"
              placeholder="Search APIs..."
              onChange={e =>
                setSearchedApi(
                  apis.filter(api =>
                    api.id
                      ?.toLocaleLowerCase()
                      .includes(e.target.value.toLocaleLowerCase()),
                  ),
                )
              }
              className="pl-10 text-white"
            />
          </div>
          <Button
            onClick={() => navigate(`/app/${appId}/apis/create`)}
            variant="default"
          >
            <Plus className="h-5 w-5" />
            <span>New</span>
          </Button>
        </div>

        {searchedApi.length > 0 ? (
          <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[75vh]">
            {searchedApi.map(api => (
              <APICard
                key={`${api.id}-${api.version}`}
                name={api.id || ""}
                version={api.version}
                routes={api.routes?.length || 0}
                count={api.count || 0}
                isUploaded={api.isUploaded || false}
                onUpload={() => handleUpload(api.id!, api.version)}
                isUploading={uploadingApi === `${api.id}-${api.version}`}
              />
            ))}
          </div>
        ) : (
          <div className="flex flex-col items-center justify-center py-12 border-2 border-dashed border-muted rounded-lg">
            <Layers className="h-12 w-12 text-muted-foreground mb-4" />
            <h3 className="text-lg font-medium mb-2">No APIs</h3>
            <p className="text-muted-foreground mb-4">
              Create your first API to get started
            </p>
          </div>
        )}
      </div>
    </ErrorBoundary>
  );
};

interface APICardProps {
  name: string;
  version: string;
  routes: number;
  count: number;
  isUploaded: boolean;
  onUpload: () => void;
  isUploading: boolean;
}

const APICard = ({
  name,
  version,
  routes,
  count,
  isUploaded,
  onUpload,
  isUploading,
}: APICardProps) => {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();

  return (
    <Card className="from-background to-muted bg-gradient-to-br border-border w-full hover:shadow-lg transition-all relative">
      <div
        className="cursor-pointer"
        onClick={() =>
          navigate(`/app/${appId}/apis/${name}/version/${version}`)
        }
      >
        <CardHeader className="flex flex-row items-center justify-between space-y-0 pb-2">
          <CardTitle className="text-base font-semibold text-emerald-400">
            {name}
          </CardTitle>
          <Badge
            variant="outline"
            className="bg-emerald-500 text-white border-emerald-400 hover:bg-emerald-600"
          >
            {count || 0}
            <GitBranch className="ml-2 h-4 w-4" />
          </Badge>
        </CardHeader>
        <CardContent>
          <div className="flex flex-col flex-grow mt-2 space-y-3">
            <div className="flex items-center justify-between text-sm text-gray-300">
              <span>Latest Version</span>
              <span>Routes</span>
            </div>
            <div className="grid grid-cols-[auto,1fr,auto,auto] items-center gap-2">
              <Badge
                variant="outline"
                className="bg-gray-600 text-white hover:bg-gray-500 transition-all duration-300"
              >
                {version}
              </Badge>
              <span className="w-4"></span>
              <LockIcon className="h-4 w-4 text-gray-400" />
              <div className="inline-flex items-center text-sm text-gray-300 w-3 justify-end">
                {routes}
              </div>
            </div>
          </div>
        </CardContent>
      </div>

      {/* Upload Status/Button - inline with other info */}
      <div className="px-4 pb-3 pt-2 border-t mt-2">
        <div className="flex items-center justify-between">
          <span className="text-xs text-muted-foreground">Status</span>
          {isUploaded ? (
            <div className="flex items-center gap-1.5 text-xs text-emerald-500">
              <CheckCircle2 className="h-3.5 w-3.5" />
              <span className="font-medium">Uploaded</span>
            </div>
          ) : (
            <Button
              size="sm"
              variant="ghost"
              className="h-6 gap-1.5 text-xs px-2"
              onClick={e => {
                e.stopPropagation();
                onUpload();
              }}
              disabled={isUploading}
            >
              {isUploading ? (
                <>
                  <div className="h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent" />
                  <span>Uploading...</span>
                </>
              ) : (
                <>
                  <Upload className="h-3.5 w-3.5" />
                  <span>Upload</span>
                </>
              )}
            </Button>
          )}
        </div>
      </div>
    </Card>
  );
};
