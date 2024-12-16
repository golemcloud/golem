import {
    ArrowLeft,
    Box,
    Code2,
    Globe,
    Plus,
    Route as RouteIcon,
    Share2,
    Trash2,
    Upload
} from 'lucide-react';
import { Link, useParams } from 'react-router-dom';
import { useApiDefinition, useUpdateApiDefinition } from '../api/api-definitions';

import RouteModal from '../components/api/ApiRoutesModal';
import toast from 'react-hot-toast';
import { useState } from 'react';

export const ApiDefinitionView = () => {
    const { id, version } = useParams<{ id: string; version: string }>();
    const [showRouteModal, setShowRouteModal] = useState(false);
    const [showDeployModal, setShowDeployModal] = useState(false);
    const [editingRoute, setEditingRoute] = useState<any>(null);

    const {
        data: apiDefinition,
        isLoading
    } = useApiDefinition(id!, version!);

    const updateDefinition = useUpdateApiDefinition();

    // --- Add Route ---
    const handleAddRoute = (route: any) => {
        if (!apiDefinition) return;

        const updatedDefinition = {
            ...apiDefinition,
            routes: [...apiDefinition.routes, route]
        };

        updateDefinition.mutate(
            { id, version, definition: updatedDefinition },
            {
                onSuccess: () => toast.success('Route added successfully'),
                onError: () => toast.error('Failed to add route')
            }
        );
    };

    // --- Delete Route ---
    const handleDeleteRoute = (index: number) => {
        if (!apiDefinition) return;

        const updatedDefinition = {
            ...apiDefinition,
            routes: apiDefinition.routes.filter((_, i) => i !== index)
        };

        updateDefinition.mutate(
            { id, version, definition: updatedDefinition },
            {
                onSuccess: () => toast.success('Route deleted successfully'),
                onError: () => toast.error('Failed to delete route')
            }
        );
    };

    // --- Edit Route (Open Modal) ---
    const handleEditRoute = (route: any, index: number) => {
        setEditingRoute({ ...route, index });
        setShowRouteModal(true);
    };

    // --- Update Route ---
    const handleUpdateRoute = (updatedRoute: any) => {
        if (!apiDefinition || editingRoute === null) return;

        const updatedRoutes = [...apiDefinition.routes];
        updatedRoutes[editingRoute.index] = updatedRoute;

        const updatedDefinition = {
            ...apiDefinition,
            routes: updatedRoutes
        };

        updateDefinition.mutate(
            { id, version, definition: updatedDefinition },
            {
                onSuccess: () => {
                    toast.success('Route updated successfully');
                    setEditingRoute(null);
                },
                onError: () => toast.error('Failed to update route')
            }
        );
    };

    if (isLoading) {
        return <div className="text-gray-400">Loading...</div>;
    }

    if (!apiDefinition) {
        return <div className="text-gray-400">API definition not found</div>;
    }

    return (
        <div className="space-y-6">
            {/* Header */}
            <div className="flex items-center justify-between">
                <div className="flex items-center gap-4">
                    <Link
                        to="/api/definitions"
                        className="p-2 text-gray-400 hover:text-gray-300 rounded-md hover:bg-gray-800"
                    >
                        <ArrowLeft size={20} />
                    </Link>
                    <div>
                        <h1 className="text-2xl font-bold flex items-center gap-2">
                            <Globe className="h-6 w-6 text-blue-400" />
                            {apiDefinition.id}
                            {apiDefinition.draft && (
                                <span className="text-sm bg-yellow-500/10 text-yellow-500 px-2 py-0.5 rounded">
                                    Draft
                                </span>
                            )}
                        </h1>
                        <p className="text-gray-400">Version {apiDefinition.version}</p>
                    </div>
                </div>

                <div className="flex gap-2">
                    <button
                        onClick={() => setShowDeployModal(true)}
                        className="flex items-center gap-2 px-4 py-2 bg-green-500 text-white rounded hover:bg-green-600"
                    >
                        <Upload size={18} />
                        Deploy
                    </button>
                    <button
                        onClick={() => setShowRouteModal(true)}
                        className="flex items-center gap-2 bg-blue-500 text-white px-4 py-2 rounded hover:bg-blue-600"
                    >
                        <Plus size={18} />
                        Add Route
                    </button>
                </div>
            </div>

            {/* Routes List */}
            <div className="bg-gray-800 rounded-lg p-6">
                <div className="space-y-3">
                    {apiDefinition.routes.map((route, index) => (
                        <div key={index} className="bg-gray-700 rounded-lg p-4">
                            <div className="flex justify-between items-start">
                                <div>
                                    <p className="font-mono">{route.method} {route.path}</p>
                                </div>
                                <div className="flex gap-2">
                                    <button
                                        onClick={() => handleEditRoute(route, index)}
                                        className="p-1.5 text-blue-400 hover:text-blue-300"
                                    >
                                        Edit
                                    </button>
                                    <button
                                        onClick={() => handleDeleteRoute(index)}
                                        className="p-1.5 text-red-400 hover:text-red-300"
                                    >
                                        Delete
                                    </button>
                                </div>
                            </div>
                        </div>
                    ))}

                    {apiDefinition.routes.length === 0 && (
                        <div className="text-center py-8 text-gray-400">
                            <p>No routes defined yet</p>
                            <button
                                onClick={() => setShowRouteModal(true)}
                                className="text-blue-400 hover:text-blue-300"
                            >
                                Add your first route
                            </button>
                        </div>
                    )}
                </div>
            </div>

            {/* Modals */}
            <RouteModal
                isOpen={showRouteModal}
                onClose={() => setShowRouteModal(false)}
                onSave={editingRoute ? handleUpdateRoute : handleAddRoute}
                existingRoute={editingRoute}
            />
        </div>
    );
};