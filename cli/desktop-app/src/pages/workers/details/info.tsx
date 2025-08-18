import React, { useEffect, useState } from "react";
import { format } from "date-fns";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Plug } from "lucide-react";
import { Update, Worker } from "@/types/worker.ts";
import { useParams } from "react-router-dom";
import { API } from "@/service";

interface PluginStatusProps {
  activePlugins: string[];
  componentVersion: number;
  status: string;
  updates: Update[];
}

const UpdateLog: React.FC<{ update: Update }> = ({ update }) => {
  const getStatusColor = (type: string) => {
    switch (type) {
      case "failedUpdate":
        return "bg-red-500";
      case "successfulUpdate":
        return "bg-green-500";
      case "pendingUpdate":
      default:
        return "bg-gray-500";
    }
  };
  const getStatusText = (type: string) => {
    switch (type) {
      case "failedUpdate":
        return "Failed";
      case "successfulUpdate":
        return "Success";
      case "pendingUpdate":
        return "Pending";
      default:
        return "Pending";
    }
  };

  return (
    <Card className="p-4 border border-gray-200 shadow-sm">
      <CardContent className="space-y-2">
        <div className="flex items-center justify-between">
          <Badge
            variant="outline"
            className={`${getStatusColor(update.type)} text-white px-3 py-1`}
          >
            {getStatusText(update.type)}
          </Badge>
          <span className="text-sm text-gray-600">
            {format(new Date(update.timestamp), "yyyy-MM-dd HH:mm:ss")}
          </span>
        </div>

        <div className="flex items-center justify-between text-sm font-medium text-gray-700">
          <span>Target Version:</span>
          <span className="font-semibold">v{update.targetVersion}</span>
        </div>

        {update.details && (
          <div className="text-sm text-gray-600 border-t pt-2">
            {update.details}
          </div>
        )}
      </CardContent>
    </Card>
  );
};

export const PluginStatus: React.FC<PluginStatusProps> = ({
  activePlugins,
  componentVersion,
  status,
  updates,
}) => {
  return (
    <>
      <Card className="w-full max-w-4xl mx-auto mb-6 shadow-md">
        <CardHeader>
          <CardTitle className="text-lg">Worker Information</CardTitle>
          <CardDescription>
            Current Version:{" "}
            <span className="font-semibold">v{componentVersion}</span> | Status:{" "}
            <Badge className="bg-blue-500 text-white px-2 py-1">{status}</Badge>
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="mb-4">
            <h3 className="text-lg font-semibold mb-3">Active Plugins</h3>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[50px]">Icon</TableHead>
                  <TableHead>Plugin ID</TableHead>
                  <TableHead className="text-right">Status</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {activePlugins.length > 0 ? (
                  activePlugins.map(plugin => (
                    <TableRow key={plugin}>
                      <TableCell>
                        <Plug className="h-4 w-4 text-gray-500" />
                      </TableCell>
                      <TableCell className="font-medium">{plugin}</TableCell>
                      <TableCell className="text-right">
                        <Badge className="bg-green-500 text-white px-2 py-1">
                          Active
                        </Badge>
                      </TableCell>
                    </TableRow>
                  ))
                ) : (
                  <TableRow>
                    <TableCell
                      colSpan={3}
                      className="text-center text-gray-500 py-6"
                    >
                      No plugins found.
                    </TableCell>
                  </TableRow>
                )}
              </TableBody>
            </Table>
          </div>
        </CardContent>
      </Card>
      <Card className="w-full max-w-4xl mx-auto shadow-md">
        <CardHeader>
          <CardTitle className="text-lg">Worker Updates</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4">
          {updates.length > 0 ? (
            updates.map((update, index) => (
              <UpdateLog key={index} update={update} />
            ))
          ) : (
            <div className="text-center text-gray-500 py-6">
              No updates found.
            </div>
          )}
        </CardContent>
      </Card>
    </>
  );
};

export default function WorkerInfo() {
  const {
    appId,
    componentId = "",
    workerName = "",
  } = useParams<{ appId: string; componentId: string; workerName: string }>();
  const [workerDetails, setWorkerDetails] = useState({} as Worker);

  useEffect(() => {
    if (componentId && workerName) {
      API.workerService
        .getParticularWorker(appId!, componentId, workerName)
        .then(response => {
          setWorkerDetails(response as Worker);
        });
    }
  }, [appId, componentId, workerName]);

  return (
    <div className="container mx-auto py-10 px-6">
      <PluginStatus
        activePlugins={workerDetails.activePlugins || []}
        componentVersion={workerDetails.componentVersion || 0}
        status={workerDetails.status || "Unknown"}
        updates={(workerDetails.updates || []).reverse()}
      />
    </div>
  );
}
