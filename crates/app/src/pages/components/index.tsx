import {useEffect, useState} from "react";
import {LayoutGrid, PlusCircle, Search} from "lucide-react";
import {Card, CardContent, CardHeader, CardTitle} from "@/components/ui/card";
import {Badge} from "@/components/ui/badge";

import {useNavigate} from "react-router-dom";
import {API} from "@/service";
import {ComponentList} from "@/types/component";
import {Worker, WorkerStatus} from "@/types/worker";
import ErrorBoundary from "@/components/errorBoundary";
import {Input} from "@/components/ui/input.tsx";
import {Button} from "@/components/ui/button.tsx";
import {calculateExportFunctions, formatRelativeTime} from "@/lib/utils.ts";

const Metrix = ["Idle", "Running", "Suspended", "Failed"];

const Components = () => {
    const navigate = useNavigate();
    const [componentList, setComponentList] = useState<{
        [key: string]: ComponentList;
    }>({});
    const [componentApiList, setComponentApiList] = useState<{
        [key: string]: ComponentList;
    }>({});
    const [workerList, setWorkerList] = useState(
        {} as {
            [key: string]: WorkerStatus;
        }
    );

    useEffect(() => {
        const fetchComponentsAndMetrics = async () => {
            try {
                const response = await API.getComponentByIdAsKey();
                setComponentApiList(response);
                setComponentList(response);

                const componentStatus: { [key: string]: WorkerStatus } = {};
                const workerPromises = Object.values(response).map(async (comp) => {
                    if (comp.componentId) {
                        const worker = await API.findWorker(comp.componentId, {
                            count: 100,
                            precise: true,
                        });
                        const status: Record<string, number> = {};
                        worker.workers.forEach((worker: Worker) => {
                            status[worker.status] = (status[worker.status] || 0) + 1;
                        });
                        componentStatus[comp.componentId] = status;
                    }
                });

                await Promise.all(workerPromises);
                setWorkerList(componentStatus);
            } catch (error) {
                console.error("Error fetching components or metrics:", error);
            }
        };

        fetchComponentsAndMetrics();
    }, []);

    const handleSearch = (e: React.ChangeEvent<HTMLInputElement>) => {
        const value = e.target.value;
        const filteredList = Object.fromEntries(
            Object.entries(componentApiList).filter(
                // eslint-disable-next-line @typescript-eslint/no-unused-vars
                ([_, data]: [string, ComponentList]) =>
                    data.componentName?.toLowerCase().includes(value) ?? false
            )
        );

        setComponentList(filteredList);
    };

    return (
        <ErrorBoundary>
            <div className="container mx-auto px-4 py-8">
                <div className="flex flex-wrap items-center justify-between gap-4 mb-8">
                    <div className="relative flex-1">
                        <Search className="absolute left-3 top-1/2 transform -translate-y-1/2 text-gray-400 h-5 w-5"/>
                        <Input
                            type="text"
                            placeholder="Search Components..."
                            className="w-full pl-10 pr-4 py-2"
                            onChange={(e) => handleSearch(e)}
                        />
                    </div>
                    <div className="flex items-center gap-2">
                        <Button onClick={() => navigate("/components/create")}>
                            <PlusCircle className="mr-2 size-4"/>
                            New
                        </Button>
                    </div>
                </div>

                {Object.keys(componentList).length === 0 ? (
                    <div
                        className="border-2 border-dashed border-gray-200 rounded-lg p-12 flex flex-col items-center justify-center">
                        <div className="h-16 w-16 bg-gray-100 rounded-lg flex items-center justify-center mb-4">
                            <LayoutGrid className="h-8 w-8 text-gray-400"/>
                        </div>
                        <h2 className="text-xl font-semibold mb-2 text-center">
                            No Project Components
                        </h2>
                        <p className="text-gray-500 mb-6 text-center">
                            Create a new component to get started.
                        </p>
                    </div>
                ) : (
                    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6 overflow-scroll max-h-[78vh]">
                        {Object.values(componentList).map((data: ComponentList) => (
                            <Card
                                key={data.componentId}
                                className="border shadow-sm cursor-pointer"
                                onClick={() => navigate(`/components/${data.componentId}`)}
                            >
                                <CardHeader className="pb-4">
                                    <CardTitle className="text-lg font-medium">
                                        {data.componentName}
                                    </CardTitle>
                                </CardHeader>
                                <CardContent className="space-y-4">
                                    <div className="grid grid-cols-2 sm:grid-cols-4 :grid-cols-4  gap-2">
                                        {Metrix.map((metric) => (
                                            <div
                                                key={metric}
                                                className="flex flex-col items-start space-y-1"
                                            >
                              <span className="text-sm text-muted-foreground">
                                {metric}
                              </span>
                                                <span className="text-lg font-medium">
                                {data.componentId !== undefined
                                    ? (
                                    workerList[
                                        data.componentId
                                        ] as unknown as Record<string, number>
                                )?.[metric] || 0
                                    : 0}
                              </span>
                                            </div>
                                        ))}
                                    </div>
                                    <div className="flex flex-wrap items-center gap-2">
                                        <Badge variant="secondary" className="rounded-md">
                                            V{data.versionList?.[data.versionList?.length - 1] || "0"}
                                        </Badge>
                                        <Badge variant="secondary" className="rounded-md">
                                            {calculateExportFunctions(
                                                data.versions?.[data.versions?.length - 1]?.metadata
                                                    ?.exports || []
                                            ).length || 0}{" "}
                                            Exports
                                        </Badge>
                                        <Badge variant="secondary" className="rounded-md">
                                            {Math.round(
                                                (data.versions?.[data.versions?.length - 1]
                                                    ?.componentSize || 0) / 1024
                                            )}{" "}
                                            KB
                                        </Badge>
                                        <Badge variant="secondary" className="rounded-md">
                                            {data.versions?.[data.versions?.length - 1].componentType}
                                        </Badge>
                                        <span className="ml-auto text-sm text-muted-foreground">
                            {formatRelativeTime(
                                data.versions?.[data.versions?.length - 1].createdAt ||
                                new Date()
                            )}
                          </span>
                                    </div>
                                </CardContent>
                            </Card>
                        ))}
                    </div>
                )}
            </div>

        </ErrorBoundary>
    );
};

export default Components;
