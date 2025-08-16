import ErrorBoundary from "@/components/errorBoundary";
import { Button } from "@/components/ui/button.tsx";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card.tsx";
import { API } from "@/service";
import { ComponentList } from "@/types/component.ts";
import { ArrowRight, LayoutGrid, PlusCircle } from "lucide-react";
import { useEffect, useState, useImperativeHandle, forwardRef } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { ComponentCard } from "../components";

export interface ComponentsSectionRef {
  refreshComponents: () => Promise<void>;
}

export const ComponentsSection = forwardRef<ComponentsSectionRef>((_, ref) => {
  const navigate = useNavigate();
  const { appId } = useParams<{ appId: string }>();
  const [components, setComponents] = useState<{
    [key: string]: ComponentList;
  }>({});

  const fetchComponents = async () => {
    if (appId) {
      let response = await API.componentService.getComponentByIdAsKey(appId);
      setComponents(response);
    }
  };

  useImperativeHandle(ref, () => ({
    refreshComponents: fetchComponents,
  }));

  useEffect(() => {
    fetchComponents();
  }, [appId]);
  return (
    <ErrorBoundary>
      <Card className="rounded-lg lg:col-span-2">
        <CardHeader>
          <div className="flex justify-between items-center mb-6">
            <CardTitle className="text-2xl font-bold text-primary">
              Components
            </CardTitle>
            <Button
              variant="ghost"
              onClick={() => navigate(`/app/${appId}/components`)}
            >
              View All
              <ArrowRight className="w-4 h-4 ml-1" />
            </Button>
          </div>
        </CardHeader>
        <CardContent>
          {Object.keys(components).length > 0 ? (
            <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-2 gap-6 overflow-scroll max-h-[70vh] px-4">
              {Object.values(components).map((data: ComponentList) => (
                <ComponentCard
                  key={data.componentId}
                  data={data}
                  onCardClick={() =>
                    navigate(`/app/${appId}/components/${data.componentId}`)
                  }
                />
              ))}
            </div>
          ) : (
            <div className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
              <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
                <LayoutGrid className="h-8 w-8 text-gray-400" />
              </div>
              <h2 className="text-xl font-semibold mb-2 text-center">
                No Components
              </h2>
              <p className="text-gray-500 mb-6 text-center">
                Create your first component to get started.
              </p>
              <Button
                onClick={() => navigate(`/app/${appId}/components/create`)}
              >
                <PlusCircle className="mr-2 size-4" />
                Create Component
              </Button>
            </div>
          )}
        </CardContent>
      </Card>
    </ErrorBoundary>
  );
});

ComponentsSection.displayName = "ComponentsSection";
