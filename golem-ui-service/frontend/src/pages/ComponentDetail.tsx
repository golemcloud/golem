import { AlertCircle, Code2, Pause, Play, Plus, Settings, Trash2 } from 'lucide-react';
import { deleteComponent, useComponent, } from '../api/components';
import { useDeleteWorker, useWorkers } from '../api/workers';
import { useNavigate, useParams } from 'react-router-dom';

import CreateComponentModal from '../components/components/CreateComponentModal';
import { CreateWorkerModal } from '../components/workers/CreateWorkerModal';
import { WorkerActionModal } from '../components/workers/UpdateWorkerModal';
import { useState } from 'react';

// Stats Card Component
const StatCard = ({ title, value }: { title: string; value: number | string }) => (
    <div className="bg-gray-800 p-4 rounded-lg">
        <h3 className="text-sm text-gray-400">{title}</h3>
        <p className="text-2xl font-semibold mt-1">{value}</p>
    </div>
);

export const ComponentDetail = () => {
    const { id } = useParams<{ id: string }>();
    const navigate = useNavigate();
    const [showCreateWorkerModal, setShowCreateWorkerModal] = useState(false);
    const [showUpdateModal, setShowUpdateModal] = useState(false);
    const deleteWorker = useDeleteWorker();
    const [actionModal, setActionModal] = useState<{
        isOpen: boolean;
        workerId: { componentId: string; workerName: string } | null;
        action: 'interrupt' | 'resume';
        currentStatus: string;
    }>({
        isOpen: false,
        workerId: null,
        action: 'interrupt',
        currentStatus: '',
    });

    const { data: component, isLoading } = useComponent(id!);
    const { data: workers } = useWorkers(id!);

    if (isLoading) {
        return <div className="text-gray-400">Loading...</div>;
    }

    if (!component) {
        return <div className="text-gray-400">Component not found</div>;
    }

    const deleteWorkerA = async (workerName: string, componentId: string) => {
        if (window.confirm('Are you sure you want to delete this worker?')) {
            deleteWorker.mutate({
                componentId,
                workerName
            });
        }
    }

    const handleAction = (
        workerId: { componentId: string; workerName: string },
        action: 'interrupt' | 'resume',
        currentStatus: string
    ) => {
        setActionModal({
            isOpen: true,
            workerId,
            action,
            currentStatus,
        });
    };

    // Calculate worker stats
    const activeWorkers = workers?.workers.filter(w => w.status != "Failed").length ?? 0;
    const runningWorkers = workers?.workers.filter(w => w.status === 'Running').length ?? 0;
    const failedWorkers = workers?.workers.filter(w => w.status === 'Failed').length ?? 0;

    return (
        <div className="space-y-6">
            {/* Header */}
            <div className="flex justify-between items-center">
                <div>
                    <h1 className="text-2xl font-bold">{component.componentName}</h1>
                    <p className="text-gray-400">Version {component.versionedComponentId.version}</p>
                </div>
                <div className="flex gap-2">
                    <button
                        onClick={() => setShowCreateWorkerModal(true)}
                        className="flex items-center gap-2 bg-blue-500 text-white px-4 py-2 rounded hover:bg-blue-600"
                    >
                        <Plus size={18} />
                        Create Worker
                    </button>
                    <button
                        onClick={() => setShowUpdateModal(true)}
                        className="flex items-center gap-2 px-4 py-2 bg-gray-700 rounded-md hover:bg-gray-600"
                    >
                        <Settings size={18} />
                        Update
                    </button>
                    <button
                        onClick={async () => {
                            if (window.confirm('Are you sure you want to delete this component?')) {
                                await deleteComponent(component.versionedComponentId.componentId)
                                navigate('/components')
                            }
                        }}
                        className="hidden items-center gap-2 px-4 py-2 bg-red-500/10 text-red-500 rounded-md hover:bg-red-500/20"
                    >
                        <Trash2 size={18} />
                        Delete
                    </button>
                </div>
            </div>

            {/* Stats Grid */}
            <div className="grid grid-cols-4 gap-4">
                <StatCard
                    title="Latest Component Version"
                    value={component.versionedComponentId.version}
                />
                <StatCard
                    title="Active Workers"
                    value={activeWorkers}
                />
                <StatCard
                    title="Running Workers"
                    value={runningWorkers}
                />
                <StatCard
                    title="Failed Workers"
                    value={failedWorkers}
                />
            </div>

            {/* Main Content Grid */}
            <div className="grid grid-cols-7 gap-6">
                {/* Exports Section - 2/7 width */}
                <div className="col-span-3 bg-gray-800 rounded-lg p-6">
                    <h2 className="text-xl font-semibold mb-4 flex items-center gap-2">
                        <Code2 size={20} />
                        Exports
                    </h2>
                    <div className="space-y-3">
                        {component.metadata.exports.map((exp, index) => (
                            <div key={index} className="p-3 bg-gray-700 rounded-lg">
                                <div className="font-medium">{exp.name}</div>
                                <div className="text-sm text-gray-400 mt-1">
                                    {exp.functions.length} functions
                                </div>
                                {/* show each function  */}
                                {exp.functions.map((func, index) => (
                                    <div key={index} className="flex items-center justify-between p-2 bg-gray-600 rounded-lg mt-2">
                                        <div>
                                            <h3 className="text-normal">{
                                                `${exp.name}.{${func.name}}`
                                            }</h3>
                                        </div>
                                    </div>
                                ))}
                            </div>
                        ))}
                        {component.metadata.exports.length === 0 && (
                            <div className="text-center py-4 text-gray-400">
                                No exports available
                            </div>
                        )}
                    </div>
                </div>

                {/* Workers Section - 5/7 width */}
                <div className="col-span-4 bg-gray-800 rounded-lg p-6">
                    <h2 className="text-xl font-semibold mb-4">Workers</h2>
                    <div className="space-y-3">
                        {workers?.workers.map((worker) => (
                            <div
                                key={worker.workerId.workerName}
                                className="flex items-center justify-between p-4 bg-gray-700 rounded-lg"
                            >
                                <div>
                                    <h3 className="font-medium">{worker.workerId.workerName}</h3>
                                    <div className="flex gap-4 mt-1">
                                        <span className="text-sm text-gray-400">
                                            Status: {worker.status}
                                        </span>
                                        {worker.env && Object.keys(worker.env).length > 0 && (
                                            <span className="text-sm text-gray-400">
                                                {Object.keys(worker.env).length} env variables
                                            </span>
                                        )}
                                    </div>
                                </div>
                                <div className="flex gap-2">
                                    <button
                                        onClick={() => handleAction(worker.workerId, 'resume', worker.status)}
                                        className="p-2 text-gray-400 hover:text-white rounded-md hover:bg-gray-600">
                                        <Play size={18} />
                                    </button>
                                    <button
                                        onClick={() => handleAction(worker.workerId, 'interrupt', worker.status)}
                                        className="p-2 text-gray-400 hover:text-white rounded-md hover:bg-gray-600">
                                        <Pause size={18} />
                                    </button>
                                    <button
                                        onClick={() => deleteWorkerA(worker.workerId.workerName, worker.workerId.componentId)}
                                        className="p-2 text-red-400 hover:text-red-300 rounded-md hover:bg-gray-600">
                                        <Trash2 size={18} />
                                    </button>
                                </div>
                            </div>
                        ))}

                        {(!workers?.workers || workers.workers.length === 0) && (
                            <div className="text-center py-8 text-gray-400">
                                <AlertCircle className="h-8 w-8 mx-auto mb-2" />
                                <p>No workers found</p>
                            </div>
                        )}
                    </div>
                </div>
            </div>

            {/* Create Worker Modal */}
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