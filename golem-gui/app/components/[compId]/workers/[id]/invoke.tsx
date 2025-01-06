import React, { useMemo, useState } from "react";
import { Worker, WorkerFunction } from "@/types/api";
import useComponents from "@/lib/hooks/use-component";
import { useParams } from "next/navigation";
import { Loader } from "lucide-react";
import {
  Paper,
  Typography,
  Divider,
  Box,
  Chip,
  Stack,
  CardContent,
  Card,
} from "@mui/material";
import DynamicForm from "./form-generator";
import { useWorkerInvocation } from "@/lib/hooks/use-worker";
import JsonEditor from "@/components/json-editor";

export function InvokeForm({
  invoke,
}: {
  invoke: { fun?: WorkerFunction; instanceName?: string | null };
}) {
  const { result, error, invokeFunction } = useWorkerInvocation(invoke);
  const paramsConfig = useMemo(() => {
    return invoke?.fun?.parameters || [];
  }, [invoke]);

  const onSubmit = async (data: unknown) => {
    invokeFunction(data);
  };

  return (
    <Box>
      <Typography variant="h5" fontWeight="bold" gutterBottom>
        {invoke?.fun?.name || "Invoke Function"}
      </Typography>
      {error && (
        <Typography variant="body2" color="error" sx={{ marginBottom: 2 }}>
          {error}
        </Typography>
      )}
      <DynamicForm config={paramsConfig} onSubmit={onSubmit} />
      {result && (
        <>
          <Divider className="my-2 bg-border" />
          <Box>
            <Typography variant="h6">Result</Typography>
            <Typography variant="body2" className="text-muted-foreground mb-1">
              View the result of your latest worker invocation
            </Typography>
            <Box
              component="pre"
              sx={{
                padding: 2,
                borderRadius: 1,
                color: "#9cdcfe",
                overflow: "auto",
                whiteSpace: "pre-wrap",
                wordBreak: "break-word",
              }}
              className="dark:bg-[#121212] bg-[#dedede]"
            >
              <JsonEditor json={result} />
            </Box>
          </Box>
        </>
      )}
    </Box>
  );
}

export default function InvokePage({ worker }: { worker: Worker }) {
  const { compId } = useParams<{ compId: string }>();
  const { components, isLoading } = useComponents(
    compId,
    worker?.componentVersion ?? "latest"
  );
  const [latestComponent] = components;
  const [invoke, setInvoke] = useState<{
    fun: WorkerFunction;
    instanceName?: string | null;
  } | null>(null);
  const exports = useMemo(() => {
    const exports = latestComponent?.metadata?.exports || [];
    const firstExport = exports[0] || null;
    const isInstance = firstExport?.type === "Instance";
    const firstFunction = firstExport
      ? isInstance
        ? firstExport?.functions?.[0]
        : firstExport
      : firstExport;
    setInvoke(
      firstFunction
        ? {
            fun: firstFunction,
            instanceName: isInstance ? firstExport.name : null,
          }
        : null
    );
    return exports;
  }, [latestComponent?.metadata?.exports]);

  if (isLoading) {
    return <Loader />;
  }

  return (
    <div className="mx-auto max-w-7xl px-2 md:px-6 lg:px-8">
      <div className="mx-auto max-w-2xl lg:max-w-none py-4">
        <Paper
          sx={{
            padding: 3,
            borderRadius: 2,
          }}
          className="border"
        >
          <Typography variant="h6" fontWeight="bold" gutterBottom>
            Select a Function
          </Typography>
          <Divider sx={{ marginY: 2, bgcolor: "#555" }} />
          <Stack direction="row" flexWrap="wrap" gap={1}>
            {exports.map((item) =>
              item.type === "Instance" ? (
                item?.functions?.map((fun) => {
                  const active =
                    invoke?.fun?.name === fun.name &&
                    invoke?.instanceName === item.name;
                  return (
                    <Chip
                      key={fun.name}
                      label={`${item.name} - ${fun.name}`}
                      onClick={() =>
                        setInvoke({
                          fun: fun,
                          instanceName: item.name,
                        })
                      }
                      className={`text-foreground ${
                        active
                          ? "bg-green-800 hover:bg-green-800"
                          : "bg-border hover:bg-border"
                      }`}
                    />
                  );
                })
              ) : (
                <Chip
                  key={item.name}
                  label={item.name}
                  onClick={() => setInvoke({ fun: item, instanceName: null })}
                  className={`${
                    invoke?.fun?.name === item.name &&
                    invoke?.instanceName === null
                      ? "bg-green-800 hover:bg-green-800 text-white"
                      : "bg-border hover:bg-border text-foreground"
                  }`}
                />
              )
            )}
          </Stack>
          <Divider className="my-2 bg-border" />
          <Box mt={4}>
            {invoke ? (
              <InvokeForm invoke={invoke} />
            ) : (
              <Typography variant="body1" color="textSecondary">
                Select a function to invoke.
              </Typography>
            )}
          </Box>
          {/* need to be replace by form preview */}
        </Paper>
      </div>
    </div>
  );
}
