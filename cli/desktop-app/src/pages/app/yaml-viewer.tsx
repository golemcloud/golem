import { useLocation, useNavigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ArrowLeft, Download } from "lucide-react";
import { YamlEditor } from "@/components/yaml-editor";
import { useEffect, useState } from "react";
import { API } from "@/service";
import { toast } from "@/hooks/use-toast";
import ErrorBoundary from "@/components/errorBoundary";

interface LocationState {
  yamlContent?: string;
}

export default function YamlViewer() {
  const navigate = useNavigate();
  const { appId } = useParams();
  const location = useLocation();
  const state = location.state as LocationState;
  const [yamlContent, setYamlContent] = useState<string>("");
  const [isLoading, setIsLoading] = useState(false);

  useEffect(() => {
    if (state?.yamlContent) {
      setYamlContent(state.yamlContent);
    } else if (appId) {
      // If no content was passed via state, fetch it
      setIsLoading(true);
      API.manifestService
        .getAppYamlContent(appId)
        .then(content => {
          setYamlContent(content);
        })
        .catch(error => {
          toast({
            title: "Failed to Load YAML",
            description: String(error),
            variant: "destructive",
          });
        })
        .finally(() => {
          setIsLoading(false);
        });
    }
  }, [appId, state]);

  const handleDownload = () => {
    const blob = new Blob([yamlContent], { type: "text/yaml" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "golem.yaml";
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  };

  const handleGoBack = () => {
    navigate(`/app/${appId}/dashboard`);
  };

  if (isLoading) {
    return (
      <div className="container mx-auto px-4 py-8">
        <div className="flex items-center justify-center min-h-[50vh]">
          <div className="text-center">
            <div className="animate-spin rounded-full h-8 w-8 border-b-2 border-primary mx-auto mb-4"></div>
            <p className="text-muted-foreground">Loading YAML content...</p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <ErrorBoundary>
      <div className="container mx-auto px-4 py-8">
        <div className="flex justify-between items-center mb-6">
          <div className="flex items-center space-x-4">
            <Button variant="ghost" onClick={handleGoBack}>
              <ArrowLeft className="h-4 w-4 mr-2" />
              Back to Dashboard
            </Button>
            <h1 className="text-3xl font-bold">Application Manifest</h1>
          </div>
          <Button variant="outline" onClick={handleDownload}>
            <Download className="h-4 w-4 mr-2" />
            Download YAML
          </Button>
        </div>

        <Card>
          <CardHeader>
            <CardTitle>golem.yaml</CardTitle>
          </CardHeader>
          <CardContent>
            <div className="h-[600px]">
              <YamlEditor value={yamlContent} onChange={setYamlContent} />
            </div>
          </CardContent>
        </Card>

        <div className="mt-4 text-sm text-muted-foreground">
          <p>
            This is the application manifest file that defines your Golem
            application structure, components, and configurations. You can view
            and edit the YAML content above.
          </p>
        </div>
      </div>
    </ErrorBoundary>
  );
}
