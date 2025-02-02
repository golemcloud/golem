import {useParams} from "react-router-dom";
import {MetricCard} from "./widgets/metrixCard";
import {ExportsList} from "./widgets/exportsList";
import {WorkerStatus} from "./widgets/workerStatus";
import {useEffect, useState} from "react";
import {API} from "@/service";
import {ComponentList} from "@/types/component.ts";
import {Worker, WorkerStatus as IWorkerStatus} from "@/types/worker.ts";

export const ComponentDetails = () => {
    const {componentId = ""} = useParams();
    const [component, setComponent] = useState({} as ComponentList);
    const [workerStatus, setWorkerStatus] = useState({} as IWorkerStatus);

    useEffect(() => {
        API.getComponentByIdAsKey().then((response) => {
            setComponent(response[componentId]);
        });

        API.findWorker(componentId!).then((res) => {
            const status: Record<string, number> = {};
            res.workers.forEach((worker: Worker) => {
                status[worker.status] = (status[worker.status] || 0) + 1;
            });
            setWorkerStatus(status);
        });
    }, [componentId]);

    return (
        <div className="flex">
            <div className="flex-1 p-8">
                <div className="p-6 max-w-7xl mx-auto space-y-6">
                    <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
                        <MetricCard
                            title="Latest Component Version"
                            value={
                                "v" +
                                (component?.versionList?.[
                                component?.versionList?.length - 1
                                    ] || "0")
                            }
                            type="version"
                        />
                        <MetricCard
                            title="Active Workers"
                            value={
                                (workerStatus.Running || 0) +
                                (workerStatus.Idle || 0) +
                                (workerStatus.Failed || 0)
                            }
                            type="active"
                        />
                        <MetricCard
                            title="Running Workers"
                            value={workerStatus.Running || 0}
                            type="running"
                        />
                        <MetricCard
                            title="Failed Workers"
                            value={workerStatus.Failed || 0}
                            type="failed"
                        />
                    </div>

                    <div className="grid gap-4 md:grid-cols-2">
                        <ExportsList
                            exports={
                                component?.versions?.[component.versions?.length - 1]
                                    ?.metadata?.exports || []
                            }
                        />
                        <WorkerStatus workerStatus={workerStatus}/>
                    </div>
                </div>
            </div>
        </div>
    );
};
