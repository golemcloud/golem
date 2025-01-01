import { useParams } from 'react-router-dom';
import { MetricCard } from "./widgets/metrixCard";
import { ExportsList } from "./widgets/exportsList";
import { WorkerStatus } from "./widgets/workerStatus";
import ComponentLeftNav from './componentsLeftNav';

const ComponentDetails = () => {
  const { componentId } = useParams();

  return (
    <div className="flex">
      <ComponentLeftNav />
      <div className="p-6 max-w-7xl mx-auto space-y-6">
      <div className="flex justify-between items-center">
        <h1 className="text-2xl font-bold">{componentId}</h1>
      </div>

      <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
        <MetricCard
          title="Latest Component Version"
          value="v0"
          type="version"
        />
        <MetricCard
          title="Active Workers"
          value="1"
          type="active"
        />
        <MetricCard
          title="Running Workers"
          value="0"
          type="running"
        />
        <MetricCard
          title="Failed Workers"
          value="0"
          type="failed"
        />
      </div>

      <div className="grid gap-4 md:grid-cols-2">
        <ExportsList />
        <WorkerStatus totalWorkers={1} />
      </div>
    </div>
    </div>
  );
};

export default ComponentDetails;




