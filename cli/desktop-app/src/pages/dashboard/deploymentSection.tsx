import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useNavigate, useParams } from "react-router-dom";
import { ArrowRight, Globe, Layers, PlusCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { API } from "@/service";
import ErrorBoundary from "@/components/errorBoundary";
import { Deployment } from "@/types/deployments";

export function DeploymentSection() {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();
  const [deployments, setDeployments] = useState([] as Deployment[]);

  useEffect(() => {
    const fetchDeployments = async () => {
      try {
        const [allDeployments] = await Promise.all([
          API.deploymentService.getDeploymentApi(appId!),
        ]);
        const combinedDeployments = allDeployments.flat().filter(Boolean);
        setDeployments(combinedDeployments);
      } catch (error) {
        console.error("Error fetching deployments:", error);
      }
    };

    fetchDeployments();
  }, []);

  return (
    <ErrorBoundary>
      <Card>
        <CardHeader className="flex flex-row items-center justify-between">
          <CardTitle className="text-xl font-semibold flex items-center gap-2 text-primary">
            <Globe className="w-5 h-5 text-muted-foreground" />
            Deployments
          </CardTitle>
          <Button
            variant="ghost"
            className="text-sm font-medium"
            size="sm"
            onClick={() => navigate(`/app/${appId}/deployments`)}
          >
            View All
            <ArrowRight className="w-4 h-4 ml-1" />
          </Button>
        </CardHeader>
        <CardContent className="space-y-2">
          {deployments.length > 0 ? (
            deployments.map((deployment, index) => (
              <div
                key={index}
                className="border rounded-lg p-3 hover:bg-muted/50 cursor-pointer bg-gradient-to-br from-background to-muted hover:shadow-lg transition-all"
                onClick={() => {
                  navigate(`/app/${appId}/deployments`);
                }}
              >
                <p className="text-sm font-medium">{deployment.site.host}</p>
              </div>
            ))
          ) : (
            <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
              <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
                <Layers className="h-8 w-8 text-gray-400" />
              </div>
              <h2 className="text-xl font-semibold mb-2 text-center">
                No Deployments
              </h2>
              <p className="text-gray-500 mb-6 text-center">
                Create your first deployment to get started.
              </p>
              <Button
                onClick={() => navigate(`/app/${appId}/deployments/create`)}
              >
                <PlusCircle className="mr-2 size-4" />
                Create Deployment
              </Button>
            </div>
          )}
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
}
