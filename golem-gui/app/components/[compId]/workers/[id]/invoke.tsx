import React, { useMemo, useState } from "react";
import { Worker, WorkerFunction } from "@/types/api";
import useComponents from "@/lib/hooks/use-component";
import { useParams } from "next/navigation";
import { Loader } from "lucide-react";
import { Paper, Typography, Divider, Box, Chip, Stack } from "@mui/material";
import DynamicForm from "./form-generator";
import { useWorkerInvocation } from "@/lib/hooks/use-worker";
import { Button2 as Button } from "@/components/ui/button";

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
          {" "}
          <Typography variant="subtitle1" sx={{ fontWeight: "bold", mt: 5 }}>
            Result:
          </Typography>
          <Typography variant="body2" className="text-muted-foreground">
            View the result of your latest worker invocation
          </Typography>
        </>
      )}
      {result && (
        <Box
          mt={2}
          p={3}
          borderRadius={1}
          overflow="auto"
          sx={{
            whiteSpace: "pre-wrap",
            fontFamily: "monospace",
            color: "black",
            fontSize: "0.9rem",
          }}
          className="dark:bg-[#0a0a0a] bg-[#dedede] dark:text-[#dedede]"
        >
          <Box
            component="pre"
            sx={{
              padding: "10px",
              overflowX: "auto",
              marginTop: "8px",
            }}
            className="dark:bg-[#282c34] bg-white"
          >
            {JSON.stringify(result, null, 2)}
          </Box>
        </Box>
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
    <div className="mx-auto max-w-7xl px-6 lg:px-8">
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
            {exports.map((item, index) =>
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
