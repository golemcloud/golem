import { Button2 as Button } from "@/components/ui/button";
import { Info } from "lucide-react";
import { Box, Typography, Divider } from "@mui/material";
import React, { useMemo } from "react";
import { toast } from "react-toastify";

import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { ComponentExport, WorkerFunction } from "@/types/api";
import useComponents from "@/lib/hooks/use-component";

export function PopoverDemo({ children }: { children: React.ReactNode }) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button variant="ghost" className="w-fit py-1" size="info">
          <Info></Info>
        </Button>
      </PopoverTrigger>
      <PopoverContent
        className="w-fit bg-[#0a0a0a] z-[1500]"
        align="start"
        side="bottom"
      >
        {children}
      </PopoverContent>
    </Popover>
  );
}

export function InterpolationExpressions() {
  const expressions = [
    { label: "Path Parameters", value: "${request.path.<PATH_PARAM_NAME>}" },
    { label: "Query Parameters", value: "${request.path.<QUERY_PARAM_NAME>}" },
    { label: "Request Body", value: "${request.body}" },
    { label: "Request Body Field", value: "${request.body.<FIELD_NAME>}" },
    { label: "Request Headers", value: "${request.header.<HEADER_NAME>}" },
  ];

  return (
    <Box>
      <Typography variant="body2" gutterBottom>
        Common Interpolation Expressions
      </Typography>
      <Divider className="bg-border my-1" />
      <Box>
        {expressions.map((item, index) => (
          <Box
            key={index}
            sx={{
              display: "flex",
              justifyContent: "space-between",
              alignItems: "center",
              gap: "8px",
              padding: "4px",
            }}
          >
            <Button variant="outline" size="icon_sm">
              {item.label}
            </Button>
            <Typography variant="body2" className="w-fit text-muted-foreground">
              {item.value}
            </Typography>
          </Box>
        ))}
      </Box>
    </Box>
  );
}

export function AvailableFunctions({ compId }: { compId: string }) {
  const { components } = useComponents(compId, "latest");
  const [latestComponent] = components;

  const exports = useMemo(() => {
    const metaExports = (latestComponent?.metadata?.exports ||
      []) as ComponentExport[];
    return metaExports.flatMap((expo: ComponentExport) =>
      "functions" in expo
        ? expo.functions?.map(
            (fun: WorkerFunction) => `${expo.name}.${fun.name}`
          )
        : expo.name
    );
  }, [latestComponent?.versionedComponentId?.version]);

  const handleCopy = (func: string) => {
    navigator.clipboard.writeText(func);
    toast.success("Copied to clipboard");
  };

  return (
    <Box>
      <Typography variant="body2" gutterBottom>
        Available Functions
      </Typography>
      <Divider className="bg-border my-1" />
      <Box>
        {exports.map((func, index) => (
          <Box key={index} gap={2}>
            <Button
              variant="ghost"
              size="icon_sm"
              className="w-fit text-muted-foreground"
              onClick={() => handleCopy(func)}
            >
              {func}
            </Button>
          </Box>
        ))}
      </Box>
    </Box>
  );
}
