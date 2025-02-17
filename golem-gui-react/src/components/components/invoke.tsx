import React, { useMemo, useState } from "react";
import { Worker, WorkerFunction } from "@lib/types/api";
import useComponents from "@lib/hooks/use-component";
import { Loader } from "lucide-react";
import {
  Paper,
  Typography,
  Divider,
  Box,
  Stack
} from "@mui/material";
import DynamicForm from "./form-generator";
import { useWorkerInvocation } from "@lib/hooks/use-worker";
import JsonEditor from "@ui/json-editor/json-editor";
import { useCustomParam } from "@lib/hooks/use-custom-param";
import { SelectInvoke } from "./select-invoke";

export function InvokeForm({
  invoke,
  isEmpheral,
}: {
  invoke: { fun?: WorkerFunction; instanceName?: string | null };
  isEmpheral?: boolean
}) {
  const { result, error, invokeFunction } = useWorkerInvocation(invoke);
  const paramsConfig = useMemo(() => {
    return invoke?.fun?.parameters || [];
  }, [invoke]);

  const onSubmit = async (data: unknown) => {
    invokeFunction(data, isEmpheral);
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

export default function InvokePage({ worker }: { worker?: Worker }) {
  const { compId } = useCustomParam();
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
          <Stack direction="row" justifyContent="space-between">
            <Typography variant="h6" fontWeight="bold">
              Select a Function
            </Typography>
            <SelectInvoke
              exports={exports}
              invoke={invoke}
              setInvoke={(item) => setInvoke(item as { fun: WorkerFunction; instanceName?: string | null })}
            />
          </Stack>
          <Divider className="my-1 bg-border" />
          <Box mt={4}>
            {invoke ? (
              <InvokeForm invoke={invoke} isEmpheral={latestComponent.componentType === "Ephemeral"}/>
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
