import { API } from "@/service";
import { Worker } from "@/types/worker.ts";
import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Check, Copy, Eye, EyeOff } from "lucide-react";

export default function WorkerEnvironments() {
  const { componentId, workerName, appId } = useParams();
  const [workerDetails, setWorkerDetails] = useState({} as Worker);
  const [visible, setVisible] = useState(false);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  useEffect(() => {
    if (componentId && workerName) {
      API.workerService
        .getParticularWorker(appId!, componentId, workerName)
        .then(response => {
          setWorkerDetails(response as Worker);
        });
    }
  }, [componentId, workerName]);

  const handleCopy = async (key: string, value: string) => {
    await navigator.clipboard.writeText(value);
    setCopiedKey(key);
    setTimeout(() => setCopiedKey(null), 2000);
  };

  return (
    <div className="flex justify-center p-6">
      <Card className="w-full max-w-2xl border rounded-lg shadow-lg p-6">
        <CardHeader>
          <CardTitle className="text-xl font-semibold">
            Environment Variables
          </CardTitle>
        </CardHeader>
        <CardContent>
          {workerDetails.env && Object.entries(workerDetails.env).length > 0 ? (
            <div className="space-y-4">
              {Object.entries(workerDetails.env).map(([key, value]) => (
                <div
                  className="flex items-center justify-between border-b pb-2"
                  key={key}
                >
                  <span className="font-medium">{key}</span>
                  <div className="flex items-center space-x-2">
                    <Input
                      type={visible ? "text" : "password"}
                      value={value}
                      readOnly
                      className="w-64 bg-transparent border-none hover:border-none hover:border-transparent focus:border-none shadow-none"
                    />
                    <TooltipProvider>
                      <Tooltip>
                        <TooltipTrigger asChild>
                          <Button
                            variant="ghost"
                            size="icon"
                            onClick={() => handleCopy(key, value)}
                          >
                            {copiedKey === key ? (
                              <Check className="h-5 w-5 text-green-500" />
                            ) : (
                              <Copy className="h-5 w-5 text-gray-500" />
                            )}
                          </Button>
                        </TooltipTrigger>
                        <TooltipContent>Copy to Clipboard</TooltipContent>
                      </Tooltip>
                    </TooltipProvider>
                    <Button
                      variant="ghost"
                      size="icon"
                      onClick={() => setVisible(prev => !prev)}
                    >
                      {visible ? (
                        <EyeOff className="h-5 w-5 text-gray-500" />
                      ) : (
                        <Eye className="h-5 w-5 text-gray-500" />
                      )}
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <div className="text-gray-500 text-sm text-center">
              No Environment Variables
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
