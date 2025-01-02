import {useParams} from 'react-router-dom';
import {MetricCard} from "./widgets/metrixCard";
import {ExportsList} from "./widgets/exportsList";
import {WorkerStatus} from "./widgets/workerStatus";
import ComponentLeftNav from './componentsLeftNav';
import {useEffect, useState} from "react";
import {API} from "@/service";
import {Component} from "@/types/component.ts";
import {Worker, WorkerStatus as IWorkerStatus} from "@/types/worker.ts";

export const ComponentDetails = () => {
    const {componentId} = useParams();
    const [component, setComponent] = useState({} as Component)
    const [_, setWorkers] = useState({} as Worker[])
    const [workerStatus, setWorkerStatus] = useState({} as IWorkerStatus)

    useEffect(() => {
        API.getComponentById(componentId!).then((res) => {
            setComponent(res);
        });

        API.findWorker(componentId!).then((res) => {
            setWorkers(res.workers);
            const status: IWorkerStatus = {};
            res.workers.forEach((worker: Worker) => {
                status[worker.status] = (status[worker.status] || 0) + 1;
            })
            setWorkerStatus(status);

        });
    }, [componentId]);
    return (
        <div className="flex">
            <ComponentLeftNav/>
            <div className="p-6 max-w-7xl mx-auto space-y-6">
                <div className="flex justify-between items-center">
                    <h1 className="text-2xl font-bold">{componentId}</h1>
                </div>

                <div className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
                    <MetricCard
                        title="Latest Component Version"
                        value={"v" + (component?.versionedComponentId?.version || "0")}
                        type="version"
                    />
                    <MetricCard
                        title="Active Workers"
                        value={(workerStatus.Running || 0) + (workerStatus.Idle || 0) + (workerStatus.Failed || 0)}
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
                    <ExportsList exports={component?.metadata?.exports[0]}/>
                    <WorkerStatus workerStatus={workerStatus}/>
                </div>
            </div>
        </div>
    );
};





