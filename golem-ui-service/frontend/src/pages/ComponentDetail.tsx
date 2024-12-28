import {
  Activity,
  AlertCircle,
  CircleDot,
  Code2,
  ExternalLink,
  Package,
  Pause,
  Play,
  Plus,
  Server,
  Settings,
  Tag,
  Terminal,
  Trash2,
  XCircle,
} from "lucide-react";
import { deleteComponent, useComponent } from "../api/components";
import { useDeleteWorker, useWorkers } from "../api/workers";
import { useNavigate, useParams } from "react-router-dom";

import CreateComponentModal from "../components/components/CreateComponentModal";
import { CreateWorkerModal } from "../components/workers/CreateWorkerModal";
import { WorkerActionModal } from "../components/workers/UpdateWorkerModal";
import { useState } from "react";

// Stats Card Component
const StatCard = ({
  title,
  value,
  icon: Icon,
}: {
  title: string;
  value: number | string;
  icon: React.ComponentType<{ size: number }>;
}) => (
  <div className="bg-card p-6 rounded-lg hover:bg-card/80 transition-colors group">
    <div className="flex items-center gap-3 mb-3">
      <div className="p-2 rounded-md bg-card/50 text-primary group-hover:text-primary-accent transition-colors">
        <Icon size={20} />
      </div>
      <h3 className="text-sm text-muted-foreground">{title}</h3>
    </div>
    <p className="text-2xl font-semibold">{value}</p>
  </div>
);

const getStatusColor = (status: string) => {
  switch (status.toLowerCase()) {
    case "running":
      return "text-green-400";
    case "failed":
      return "text-red-400";
    default:
      return "text-yellow-400";
  }
};

export const ComponentDetail = () => {
  const { id } = useParams<{ id: string }>();
  const navigate = useNavigate();
  const [showCreateWorkerModal, setShowCreateWorkerModal] = useState(false);
  const [showUpdateModal, setShowUpdateModal] = useState(false);
  const deleteWorker = useDeleteWorker();
  const [actionModal, setActionModal] = useState<{
    isOpen: boolean;
    workerId: { componentId: string; workerName: string } | null;
    action: "interrupt" | "resume";
    currentStatus: string;
  }>({
    isOpen: false,
    workerId: null,
    action: "interrupt",
    currentStatus: "",
  });

  const { data: component, isLoading } = useComponent(id!);
  const { data: workers } = useWorkers(id!);

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64">
        <div className="text-muted-foreground flex items-center gap-2">
          <Activity className="animate-spin" size={20} />
          <span>Loading component details...</span>
        </div>
      </div>
    );
  }

  if (!component) {
    return (
      <div className="flex flex-col items-center justify-center h-64 text-muted-foreground">
        <XCircle size={48} className="mb-4 text-gray-600" />
        <p>Component not found</p>
      </div>
    );
  }

  const deleteWorkerA = async (workerName: string, componentId: string) => {
    if (window.confirm("Are you sure you want to delete this worker?")) {
      deleteWorker.mutate({ componentId, workerName });
    }
  };

  const handleAction = (
    workerId: { componentId: string; workerName: string },
    action: "interrupt" | "resume",
    currentStatus: string,
  ) => {
    setActionModal({ isOpen: true, workerId, action, currentStatus });
  };

  const activeWorkers =
    workers?.workers.filter((w) => w.status != "Failed").length ?? 0;
  const runningWorkers =
    workers?.workers.filter((w) => w.status === "Running").length ?? 0;
  const failedWorkers =
    workers?.workers.filter((w) => w.status === "Failed").length ?? 0;

  return (
    <div className="space-y-8">
      {/* Header */}
      <div className="bg-card/50 p-6 rounded-lg">
        <div className="flex justify-between items-start">
          <div>
            <div className="flex items-center gap-3">
              <Package size={24} className="text-primary" />
              <div>
                <h1 className="text-2xl font-bold">
                  {component.componentName}
                </h1>
                <div className="flex items-center gap-2 mt-1 text-muted-foreground">
                  <Terminal size={14} />
                  <span>Version {component.versionedComponentId.version}</span>
                </div>
              </div>
            </div>
          </div>
          <div className="flex gap-3">
            <button
              onClick={() => setShowCreateWorkerModal(true)}
              className="flex items-center gap-2 bg-primary text-white px-4 py-2 rounded-lg 
                                     hover:bg-blue-600 transition-all duration-200 shadow-lg hover:shadow-xl"
            >
              <Plus size={18} />
              Create Worker
            </button>
            <button
              onClick={() => setShowUpdateModal(true)}
              className="flex items-center gap-2 px-4 py-2 bg-card/80 rounded-lg hover:bg-gray-600 
                                     transition-all duration-200"
            >
              <Settings size={18} />
              Update
            </button>
            <button
              onClick={async () => {
                if (
                  window.confirm(
                    "Are you sure you want to delete this component?",
                  )
                ) {
                  await deleteComponent(
                    component.versionedComponentId.componentId,
                  );
                  navigate("/components");
                }
              }}
              className="hidden items-center gap-2 px-4 py-2 bg-red-500/10 text-red-500 rounded-lg 
                                     hover:bg-red-500/20 transition-all duration-200"
            >
              <Trash2 size={18} />
              Delete
            </button>
          </div>
        </div>
      </div>

      {/* Stats Grid */}
      <div className="grid grid-cols-4 gap-6">
        <StatCard
          title="Latest Version"
          value={component.versionedComponentId.version}
          icon={Tag}
        />
        <StatCard title="Active Workers" value={activeWorkers} icon={Server} />
        <StatCard
          title="Running Workers"
          value={runningWorkers}
          icon={Activity}
        />
        <StatCard
          title="Failed Workers"
          value={failedWorkers}
          icon={AlertCircle}
        />
      </div>

      {/* Main Content Grid */}
      <div className="grid grid-cols-7 gap-6">
        {/* Exports Section */}
        <div className="col-span-3 bg-card rounded-lg p-6">
          <h2 className="text-xl font-semibold mb-6 flex items-center gap-3">
            <Code2 size={20} className="text-primary" />
            Exports
          </h2>
          <div className="space-y-4">
            {component.metadata.exports.map((exp, index) => (
              <div
                key={index}
                className="p-4 bg-card/50 rounded-lg hover:bg-card/80 transition-colors"
              >
                <div className="flex items-center gap-2 mb-3">
                  <ExternalLink size={16} className="text-primary" />
                  <span className="font-medium">{exp.name}</span>
                  <span className="text-sm text-muted-foreground ml-auto">
                    {exp.functions.length} functions
                  </span>
                </div>
                <div className="space-y-2">
                  {exp.functions.map((func, index) => (
                    <div
                      key={index}
                      className="flex items-center gap-2 p-3 bg-card/50 rounded-lg
                                                      hover:bg-card transition-colors"
                    >
                      <Terminal size={14} className="text-muted-foreground" />
                      <span className="text-sm">
                        {`${exp.name}.${func.name}`}
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            ))}
            {component.metadata.exports.length === 0 && (
              <div className="text-center py-8 text-muted-foreground">
                <Code2 size={32} className="mx-auto mb-2 text-gray-600" />
                <p>No exports available</p>
              </div>
            )}
          </div>
        </div>

        {/* Workers Section */}
        <div className="col-span-4 bg-card rounded-lg p-6">
          <h2 className="text-xl font-semibold mb-6 flex items-center gap-3">
            <Server size={20} className="text-primary" />
            Workers
          </h2>
          <div className="space-y-4">
            {workers?.workers.map((worker) => (
              <div
                key={worker.workerId.workerName}
                className="group flex items-center justify-between p-4 bg-card/50 rounded-lg
                                         hover:bg-card/80 transition-all duration-200"
              >
                <div className="flex items-center gap-4">
                  <div
                    className={`p-2 rounded-md bg-card/50 ${getStatusColor(worker.status)}`}
                  >
                    <CircleDot size={16} />
                  </div>
                  <div>
                    <h3 className="font-medium flex items-center gap-2">
                      {worker.workerId.workerName}
                    </h3>
                    <div className="flex gap-4 mt-1">
                      <span className="text-sm text-muted-foreground">
                        Status: {worker.status}
                      </span>
                      {worker.env && Object.keys(worker.env).length > 0 && (
                        <span className="text-sm text-muted-foreground">
                          {Object.keys(worker.env).length} env variables
                        </span>
                      )}
                    </div>
                  </div>
                </div>
                <div className="flex gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                  <button
                    onClick={() =>
                      handleAction(worker.workerId, "resume", worker.status)
                    }
                    className="p-2 text-muted-foreground hover:text-green-400 rounded-md hover:bg-gray-600
                                                 transition-colors"
                    title="Resume worker"
                  >
                    <Play size={18} />
                  </button>
                  <button
                    onClick={() =>
                      handleAction(worker.workerId, "interrupt", worker.status)
                    }
                    className="p-2 text-muted-foreground hover:text-yellow-400 rounded-md hover:bg-gray-600
                                                 transition-colors"
                    title="Interrupt worker"
                  >
                    <Pause size={18} />
                  </button>
                  <button
                    onClick={() =>
                      deleteWorkerA(
                        worker.workerId.workerName,
                        worker.workerId.componentId,
                      )
                    }
                    className="p-2 text-muted-foreground hover:text-red-400 rounded-md hover:bg-gray-600
                                                 transition-colors"
                    title="Delete worker"
                  >
                    <Trash2 size={18} />
                  </button>
                </div>
              </div>
            ))}

            {(!workers?.workers || workers.workers.length === 0) && (
              <div className="text-center py-12 text-muted-foreground">
                <Server size={32} className="mx-auto mb-2 text-gray-600" />
                <p>No workers found</p>
                <p className="text-sm text-gray-500 mt-1">
                  Create a worker to get started
                </p>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Modals */}
      <CreateWorkerModal
        isOpen={showCreateWorkerModal}
        onClose={() => setShowCreateWorkerModal(false)}
        componentId={id!}
      />

      <CreateComponentModal
        isOpen={showUpdateModal}
        onClose={() => setShowUpdateModal(false)}
        existingComponent={component}
      />

      {actionModal.workerId && (
        <WorkerActionModal
          isOpen={actionModal.isOpen}
          onClose={() => setActionModal({ ...actionModal, isOpen: false })}
          workerId={actionModal.workerId}
          action={actionModal.action}
          currentStatus={actionModal.currentStatus}
        />
      )}
    </div>
  );
};
