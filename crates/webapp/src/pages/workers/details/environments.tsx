import ErrorBoundary from "@/components/errorBoundary";
import WorkerLeftNav from "./leftNav";
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
import { Eye, EyeOff, Copy, Check } from "lucide-react";

export default function WorkerEnvironments() {
  const { componentId, workerName } = useParams();
  const [workerDetails, setWorkerDetails] = useState({} as Worker);

  useEffect(() => {
    if (componentId && workerName) {
      API.getParticularWorker(componentId, workerName).then((response) => {
        setWorkerDetails(response);
      });
    }
  }, [componentId, workerName]);

  const [visible, setVisible] = useState(false);
  const [copied, setCopied] = useState(false);

  const handleCopy = async (value: string) => {
    await navigator.clipboard.writeText(value);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <ErrorBoundary>
      <div className="flex">
        <WorkerLeftNav />
        <div className="flex-1 flex flex-col">
          <header className="w-full border-b bg-background py-4">
            <div className="mx-auto px-6 lg:px-8">
              <div className="flex items-center gap-4">
                <h1 className="text-xl font-semibold text-foreground truncate">
                  {workerName}
                </h1>
              </div>
            </div>
          </header>
          <div className="p-10 space-y-6 max-w-7xl mx-auto overflow-scroll h-[76vh]">
            <Card className="w-full min-w-[600px] max-w-md mx-auto border rounded-md shadow-sm  p-6">
              <CardHeader className="flex justify-between mb-4 font-large">
                <CardTitle>Environment</CardTitle>
              </CardHeader>
              <CardContent>
                {workerDetails.env &&
                Object.entries(workerDetails.env)?.length > 0 ? (
                  Object.entries(workerDetails.env).map(([key, value]) => (
                    <div
                      className={`flex items-center justify-between border-b`}
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
                                onClick={() => handleCopy(value)}
                              >
                                {copied ? (
                                  <Check className="h-5 w-5" />
                                ) : (
                                  <Copy className="h-5 w-5" />
                                )}
                              </Button>
                            </TooltipTrigger>
                            <TooltipContent>Copy to Clipboard</TooltipContent>
                          </Tooltip>
                        </TooltipProvider>
                        <Button
                          variant="ghost"
                          size="icon"
                          onClick={() => setVisible((prev) => !prev)}
                        >
                          {visible ? (
                            <EyeOff className="h-5 w-5" />
                          ) : (
                            <Eye className="h-5 w-5" />
                          )}
                        </Button>
                      </div>
                    </div>
                  ))
                ) : (
                  <div>No Environment Variables</div>
                )}
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}
