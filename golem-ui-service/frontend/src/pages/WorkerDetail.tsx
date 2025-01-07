import {
    Activity,
    AlertCircle,
    ArrowLeft,
    Box,
    CircleDot,
    Clock,
    Folder,
    Loader2,
    Menu,
    Pause,
    Play,
    Settings,
    Shield,
    Terminal,
    Timer,
    XCircle,
} from "lucide-react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useDeleteWorker, useInterruptWorker, useResumeWorker, useWorker, useWorkerLogs } from "../api/workers";
import { useEffect, useState } from "react";

import AdvancedTab from "../components/workers/worker-details/AdvancedTab";
import ConfigTab from "../components/workers/worker-details/Configuration";
import FilesTab from "../components/workers/worker-details/Files";
import LogsViewer from "../components/workers/LogsViewer";
import Overview from "../components/workers/worker-details/Overview";
import { WorkerUpdate } from "../types/api";
import toast from "react-hot-toast";
import { useComponent } from "../api/components";

const StatusIndicator = ({ status }: { status: string }) => {
    const getStatusColor = (status: string) => {
        switch (status.toLowerCase()) {
            case "running":
                return "text-success bg-success-background";
            case "failed":
                return "text-destructive bg-destructive-background";
            case "suspended":
                return "text-primary bg-primary-background";
            case "interrupted":
                return "text-destructive-accent bg-destructive-background";
            default:
                return "text-muted-foreground bg-muted";
        }
    };

    return (
        <div className={`inline-flex items-center gap-2 px-3 py-1 rounded-full text-sm font-medium ${getStatusColor(status)}`}>
            <CircleDot size={12} />
            {status}
        </div>
    );
};

const TabButton = ({
    active,
    icon: Icon,
    children,
    onClick,
    className = "",
}: {
    active: boolean;
    icon: any;
    children: React.ReactNode;
    onClick: () => void;
    className?: string;
}) => (
    <button
        onClick={onClick}
        className={`flex items-center gap-2 px-4 py-2 rounded-lg transition-colors ${active
            ? "bg-primary text-primary-foreground"
            : "text-muted-foreground hover:text-foreground hover:bg-card/60"
            } ${className}`}>
        <Icon size={16} />
        {children}
    </button>
);

type TabType = "overview" | "logs" | "events" | "files" | "config" | "advanced";

export default function WorkerDetail() {
    const { componentId, workerName } = useParams<{
        componentId: string;
        workerName: string;
    }>();
    const navigate = useNavigate();
    const [searchParams, setSearchParams] = useSearchParams();
    const [activeTab, setActiveTab] = useState<TabType>(() => {
        const tabParam = searchParams.get("tab");
        return (tabParam as TabType) || "overview";
    });
    const [isMobileMenuOpen, setIsMobileMenuOpen] = useState(false);

    const handleTabChange = (tab: TabType) => {
        setActiveTab(tab);
        setSearchParams({ tab });
        setIsMobileMenuOpen(false);
    };

    const { data: worker, isLoading, error } = useWorker(componentId!, workerName!);
    const interruptWorker = useInterruptWorker();
    const { data: component } = useComponent(componentId!);
    const resumeWorker = useResumeWorker();
    const deleteWorker = useDeleteWorker();
    const {
        data: logs,
        isLoading: isLoadingLogs,
        error: errorLogs,
    } = useWorkerLogs(componentId!, workerName!, 100);

    useEffect(() => {
        if (worker) {
            document.title = `worker ${worker.workerId.workerName} - ${worker.workerId.componentId} - Golem UI`;
        }
    }, [worker]);

    const handleAction = async (action: "interrupt" | "resume" | "delete") => {
        try {
            if (action === "interrupt") {
                await interruptWorker.mutateAsync({
                    componentId: componentId!,
                    workerName: workerName!,
                    recoverImmediately: false,
                });
                toast.success("Worker interrupted successfully");
            } else if (action === "resume") {
                await resumeWorker.mutateAsync({
                    componentId: componentId!,
                    workerName: workerName!,
                });
                toast.success("Worker resumed successfully");
            } else {
                await deleteWorker.mutateAsync({
                    componentId: componentId!,
                    workerName: workerName!,
                });
                toast.success("Worker deleted successfully");
                navigate(`/components/${componentId}`);
            }
        } catch (error) {
            console.error(error);
        }
    };

    if (isLoading) {
        return (
            <div className="flex items-center justify-center h-screen">
                <div className="flex items-center gap-2 text-muted-foreground">
                    <Loader2 className="animate-spin" size={20} />
                    <span>Loading worker details...</span>
                </div>
            </div>
        );
    }

    if (error || !worker) {
        return (
            <div className="flex flex-col items-center justify-center h-screen text-muted-foreground">
                <XCircle size={48} className="mb-4" />
                <p>Failed to load worker details</p>
                <button
                    onClick={() => navigate("/components")}
                    className="mt-4 flex items-center gap-2 text-primary hover:text-primary-accent transition-colors">
                    <ArrowLeft size={16} />
                    Back to Components
                </button>
            </div>
        );
    }

    return (
        <div className="space-y-6 mx-auto px-4 md:px-6 max-w-7xl">
            {/* Header */}
            <div className="bg-card/80 border border-border/10 rounded-lg p-4 md:p-6">
                <div className="flex flex-col md:flex-row md:items-center md:justify-between gap-4">
                    <div className="flex items-center gap-4">
                        <Link
                            to={`/components/${componentId}`}
                            className="p-2 text-muted-foreground hover:text-foreground rounded-lg hover:bg-card/60 transition-colors">
                            <ArrowLeft size={20} />
                        </Link>
                        <div>
                            <h1 className="text-xl md:text-2xl font-bold flex items-center gap-2 break-all">
                                <Box className="text-primary flex-shrink-0" size={24} />
                                {worker.workerId.workerName}
                            </h1>
                            <div className="flex flex-wrap items-center gap-4 mt-2">
                                <StatusIndicator status={worker.status} />
                                <span className="text-muted-foreground">
                                    Version {worker.componentVersion}
                                </span>
                            </div>
                        </div>
                    </div>

                    <div className="flex items-center gap-2">
                        {["Running"].includes(worker.status) && (
                            <button
                                onClick={() => handleAction("interrupt")}
                                className="flex items-center gap-2 px-4 py-2 bg-destructive/10 text-destructive rounded-lg hover:bg-destructive/20 transition-colors">
                                <Pause size={16} />
                                <span className="hidden sm:inline">Interrupt</span>
                            </button>
                        )}
                        {["Interrupted", "Failed", "Exited"].includes(worker.status) && (
                            <button
                                onClick={() => handleAction("resume")}
                                disabled={worker.status === "Failed"}
                                className="flex items-center gap-2 px-4 py-2 bg-success/10 text-success rounded-lg hover:bg-success/20 transition-colors disabled:opacity-50">
                                <Play size={16} />
                                <span className="hidden sm:inline">Resume</span>
                            </button>
                        )}
                    </div>
                </div>
            </div>

            {/* Tabs */}
            <div className="border-b border-border/20">
                {/* Mobile Menu Button */}
                <div className="md:hidden">
                    <button
                        onClick={() => setIsMobileMenuOpen(!isMobileMenuOpen)}
                        className="w-full flex items-center justify-between p-4 text-muted-foreground">
                        <span className="flex items-center gap-2">
                            <Menu size={20} />
                            {activeTab.charAt(0).toUpperCase() + activeTab.slice(1)}
                        </span>
                    </button>
                    {isMobileMenuOpen && (
                        <div className="absolute z-50 left-0 right-0 bg-background border-b border-border/20 shadow-lg">
                            <div className="flex flex-col p-2">
                                {["overview", "logs", "events", "files", "config", "advanced"].map((tab) => (
                                    <TabButton
                                        key={tab}
                                        active={activeTab === tab}
                                        icon={
                                            tab === "overview"
                                                ? Activity
                                                : tab === "logs"
                                                    ? Terminal
                                                    : tab === "events"
                                                        ? Clock
                                                        : tab === "files"
                                                            ? Folder
                                                            : tab === "config"
                                                                ? Settings
                                                                : Shield
                                        }
                                        onClick={() => handleTabChange(tab as TabType)}
                                        className="w-full justify-start">
                                        {tab.charAt(0).toUpperCase() + tab.slice(1)}
                                    </TabButton>
                                ))}
                            </div>
                        </div>
                    )}
                </div>

                {/* Desktop Tabs */}
                <div className="hidden md:flex gap-2">
                    <TabButton
                        active={activeTab === "overview"}
                        icon={Activity}
                        onClick={() => handleTabChange("overview")}>
                        Overview
                    </TabButton>
                    <TabButton
                        active={activeTab === "logs"}
                        icon={Terminal}
                        onClick={() => handleTabChange("logs")}>
                        Logs
                    </TabButton>
                    <TabButton
                        active={activeTab === "events"}
                        icon={Clock}
                        onClick={() => handleTabChange("events")}>
                        Events
                    </TabButton>
                    <TabButton
                        active={activeTab === "files"}
                        icon={Folder}
                        onClick={() => handleTabChange("files")}>
                        Files
                    </TabButton>
                    <TabButton
                        active={activeTab === "config"}
                        icon={Settings}
                        onClick={() => handleTabChange("config")}>
                        Configuration
                    </TabButton>
                    <TabButton
                        active={activeTab === "advanced"}
                        icon={Shield}
                        onClick={() => handleTabChange("advanced")}>
                        Advanced
                    </TabButton>
                </div>
            </div>

            {/* Tab Content */}
            <div className="space-y-6">
                {activeTab === "overview" && (
                    <Overview worker={worker} component={component} />
                )}

                {activeTab === "logs" && (
                    <div className="bg-card/80 border border-border/10 rounded-lg p-4 md:p-6">
                        <div className="flex items-center justify-between mb-4">
                            <h3 className="text-lg font-semibold flex items-center gap-2">
                                <Terminal size={20} className="text-primary" />
                                Worker Logs
                            </h3>
                        </div>
                        <div className="bg-card/60 rounded-lg p-4 font-mono text-sm h-96 overflow-auto">
                            <div className="text-center text-muted-foreground">
                                {!isLoadingLogs && <LogsViewer logs={logs} />}
                            </div>
                        </div>
                    </div>
                )}

                {activeTab === "events" && (
                    <div className="bg-card/80 border border-border/10 rounded-lg p-4 md:p-6">
                        <div className="flex items-center justify-between mb-4">
                            <h3 className="text-lg font-semibold flex items-center gap-2">
                                <Activity size={20} className="text-primary" />
                                System Events
                            </h3>
                        </div>
                        <div className="space-y-2">
                            {worker.updates.map((update: WorkerUpdate, index: number) => (
                                <div
                                    key={index}
                                    className="p-4 bg-card/60 rounded-lg hover:bg-card/70 transition-colors">
                                    <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-4">
                                        <div className="flex items-center gap-3">
                                            <div className="p-2 rounded-full bg-primary/10 text-primary">
                                                {update.type !== "pendingUpdate" && (
                                                    <Activity size={16} />
                                                )}
                                            </div>
                                            <div>
                                                <div className="font-medium flex items-center gap-2">
                                                    <span>Pending Update</span>
                                                    <span className="text-sm text-muted-foreground">
                                                        v{update.targetVersion}
                                                    </span>
                                                </div>
                                                <div className="text-sm text-muted-foreground flex items-center gap-2">
                                                    <Clock size={14} />
                                                    {new Date(update.timestamp).toLocaleString()}
                                                </div>
                                            </div>
                                        </div>
                                    </div>
                                    <div className="mt-3 pl-4 md:pl-12">
                                        <div className="text-sm space-y-1">
                                            <div className="flex items-center gap-2">
                                                <Timer size={14} className="text-muted-foreground" />
                                                <span className="break-all">
                                                    Scheduled: {new Date(update.timestamp).toLocaleString()}
                                                </span>
                                            </div>
                                            {update.type === "pendingUpdate" && (
                                                <div className="flex items-center gap-2 text-primary">
                                                    <AlertCircle size={14} />
                                                    <span>Update pending deployment</span>
                                                </div>
                                            )}
                                        </div>
                                    </div>
                                </div>
                            ))}
                            {worker.updates.length === 0 && (
                                <div className="flex flex-col items-center justify-center py-8 text-muted-foreground">
                                    <Activity size={24} className="mb-2 opacity-50" />
                                    <p>No system events recorded</p>
                                    <p className="text-sm mt-1">
                                        Worker events will appear here when they occur
                                    </p>
                                </div>
                            )}
                        </div>
                    </div>
                )}

                {activeTab === "files" && (
                    <FilesTab worker={worker} />
                )}

                {activeTab === "config" && (
                    <ConfigTab worker={worker} />
                )}

                {activeTab === "advanced" && (
                    <AdvancedTab worker={worker} onAction={handleAction} />
                )}
            </div>
        </div>
    );
}