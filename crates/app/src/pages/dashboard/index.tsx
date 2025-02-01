import { ComponentsSection } from "@/pages/dashboard/componentSection.tsx";
import { APISection } from "@/pages/dashboard/apiSection.tsx";
import { DeploymentSection } from "@/pages/dashboard/deploymentSection.tsx";

export const Dashboard = () => {
  return (
    <div className="container mx-auto px-4 py-8">
      <div className="grid flex-1 grid-cols-1 gap-4 lg:grid-cols-3 lg:gap-6 min-h-[70vh] mb-8">
        <ComponentsSection />
        <div className="flex gap-4 flex-col">
          <DeploymentSection />
          <APISection />
        </div>
      </div>
    </div>
  );
};
