import React, {useEffect, useState} from "react"
import {format} from "date-fns"
import {Card, CardContent, CardDescription, CardHeader, CardTitle} from "@/components/ui/card"
import {Badge} from "@/components/ui/badge"
import {ScrollArea} from "@/components/ui/scroll-area"
import {Table, TableBody, TableCell, TableHead, TableHeader, TableRow} from "@/components/ui/table"
import {Plug} from "lucide-react"
import {Update, Worker} from "@/types/worker.ts"
import {useParams} from "react-router-dom";
import {API} from "@/service";


interface PluginStatusProps {
    activePlugins: string[]
    componentVersion: number
    status: string
    updates: Update[]
}

const UpdateLog: React.FC<{ update: Update }> = ({update}) => {
    const getStatusColor = (type: string) => {
        switch (type) {
            case "failedUpdate":
                return "bg-red-500"
            case "successfulUpdate":
                return "bg-green-500"
            default:
                return "bg-gray-500"
        }
    }

    return (
        <div className="mb-2 flex items-center space-x-2">
            <Badge variant="outline" className={`${getStatusColor(update.type)} text-white`}>
                {update.type === "failedUpdate" ? "Failed" : "Success"}
            </Badge>
            <span className="text-sm text-gray-600">{format(new Date(update.timestamp), "yyyy-MM-dd HH:mm:ss")}</span>
            <span className="text-sm">Target Version: {update.targetVersion}</span>
            {update.details && <span className="text-sm text-gray-500 truncate">{update.details}</span>}
        </div>
    )
}

export const PluginStatus: React.FC<PluginStatusProps> = ({activePlugins, componentVersion, status, updates}) => {
    return (
        <Card className="w-full max-w-4xl mx-auto">
            <CardHeader>
                <CardTitle>Worker Information</CardTitle>
                <CardDescription>
                    Current Version: {componentVersion} | Status: {status}
                </CardDescription>
            </CardHeader>
            <CardContent>
                <div className="mb-4">
                    <h3 className="text-lg font-semibold mb-2">Active Plugins</h3>
                    <Table>
                        <TableHeader>
                            <TableRow>
                                <TableHead className="w-[50px]">Icon</TableHead>
                                <TableHead>Plugin ID</TableHead>
                                <TableHead className="text-right">Status</TableHead>
                            </TableRow>
                        </TableHeader>
                        <TableBody>
                            {activePlugins.length > 0 ?
                                activePlugins.map((plugin) => (
                                    <TableRow key={plugin}>
                                        <TableCell>
                                            <Plug className="h-4 w-4"/>
                                        </TableCell>
                                        <TableCell className="font-medium">{plugin}</TableCell>
                                        <TableCell className="text-right">
                                            <Badge variant="outline" className="bg-green-500 text-white">
                                                Active
                                            </Badge>
                                        </TableCell>
                                    </TableRow>
                                ))
                                : <TableRow>
                                    <TableCell colSpan={3} className="text-center text-muted-foreground h-52">
                                        No plugins found.
                                    </TableCell>
                                </TableRow>}
                        </TableBody>
                    </Table>
                </div>
                <div>
                    <h3 className="text-lg font-semibold mb-2">Update Logs</h3>
                    <ScrollArea className="h-[300px] w-full rounded-md border p-4">
                        {updates.map((update, index) => (
                            <UpdateLog key={index} update={update}/>
                        ))}
                    </ScrollArea>
                </div>
            </CardContent>
        </Card>
    )
}
export default function WorkerInfo() {
    const {componentId = "", workerName = ""} = useParams();
    const [workerDetails, setWorkerDetails] = useState({} as Worker);

    useEffect(() => {
        if (componentId && workerName) {
            API.getParticularWorker(componentId, workerName).then((response) => {
                setWorkerDetails(response);
                console.log("response", response);
            });
        }
    }, [componentId, workerName]);
    return (
        <div className="container mx-auto py-8">
            <PluginStatus
                activePlugins={workerDetails.activePlugins || []}
                componentVersion={workerDetails.componentVersion || 0}
                status={workerDetails.status || ""}
                updates={workerDetails.updates.reverse() || []}
            />
        </div>
    )
}



