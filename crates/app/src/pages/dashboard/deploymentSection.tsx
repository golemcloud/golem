import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useNavigate } from "react-router-dom";
import { Layers, PlusCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { API } from "@/service";
import ErrorBoundary from "@/components/errorBoundary";
import { removeDuplicateApis } from "@/lib/utils";
import { Deployment } from "@/types/deployments";

export function DeploymentSection() {
  const navigate = useNavigate();
  const [deployments, setDeployments] = useState([] as Deployment[]);

  useEffect(() => {
    const fetchDeployments = async () => {
      try {
        const response = await API.getApiList();
        const newData = removeDuplicateApis(response);
        const deploymentPromises = newData.map((api) =>
          API.getDeploymentApi(api.id)
        );
        const allDeployments = await Promise.all(deploymentPromises);
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
      <Card className={"rounded-lg max-h-[250px]"}>
        <CardHeader>
          <div className="flex justify-between items-center mb-6">
            <CardTitle>Deployments</CardTitle>
            <Button variant="outline" onClick={() => navigate("/deployments")}>
              View All
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {deployments.length > 0 ? (
            <div className="grid gap-0 overflow-scroll max-h-[110px]">
              {deployments.map((deployment, index) => (
                <div
                  key={index}
                  className="flex w-full items-center justify-between py-4 px-4 hover:bg-accent rounded-none border-t border-b border-gray cursor-pointer"
                  onClick={() => {
                    navigate(`/deployments`);
                  }}
                >
                  <span className="text-gray-500">{deployment.site.host}</span>
                </div>
              ))}
            </div>
          ) : (
            <div className="rounded-lg border-2 border-dashed border-border p-4 text-center grid place-items-center h-full w-full">
              <Layers className="h-5 w-5 text-gray-400 mb-2" />
              <h3 className="text-lg font-medium mb-1">No Deployments</h3>
              <Button onClick={() => navigate("/deployments/create")}>
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
