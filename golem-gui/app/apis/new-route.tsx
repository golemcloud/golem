"use client";

import React, { useState } from "react";
import { useForm, Controller } from "react-hook-form";
import {
  Box,
  Button,
  TextField,
  Typography,
  FormControl,
  Divider,
} from "@mui/material";
import useSWR from "swr";
import { fetcher } from "@/lib/utils";
import { ApiRoute, Component } from "@/types/api";
import { Info, Loader } from "lucide-react";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { ComponentSelect } from "./new-route-select";
import { Button2 } from "@/components/ui/button";
import { PopoverDemo } from "./interpolate-tooltip";
import {
  InterpolationExpressions,
  AvailableFunctions,
} from "@/app/apis/interpolate-tooltip";
import { useRouter } from "next/navigation";

type FormData = {
  path: string;
  workerName: string;
  response: string;
  method: string;
  component: string;
  version: string;
};

const requestPathRegex = /request\.path\.([a-zA-Z0-9_-]+)/g;  // Matches request.path.<key> with optional trailing ;, , space or $
const pathPlaceholdersRegex = /\{([a-zA-Z0-9\-]+)\}/g; 

const NewRouteForm = ({
  apiId,
  version,
  isModal,
  isExperimental,
  defaultRoute,
  onSuccess,
  noRedirect,
}: {
  apiId: string;
  version?: string | null;
  onSuccess?: () => void;
  isModal?: boolean;
  isExperimental?: boolean;
  defaultRoute?: ApiRoute | null;
  noRedirect?: boolean;
}) => {
  const {
    control,
    handleSubmit,
    reset,
    watch,
    formState: { errors },
  } = useForm<FormData>({
    defaultValues: {
      path: defaultRoute?.path || "",
      workerName: defaultRoute?.binding?.workerName || "",
      response: defaultRoute?.binding?.response || "",
      method: defaultRoute?.method || "Get",
      component: defaultRoute?.binding?.componentId.componentId || "",
      version: defaultRoute?.binding?.componentId.version?.toString() || "",
    },
  });

  const component = watch("component");
  const path = watch("path");
  const router=useRouter();
  const [error, setError] = useState<string | null>(null);
  const { data, isLoading } = useSWR("v1/components", fetcher);
  const {
    getApiDefintion,
    isLoading: apiDefinitonLoading,
    upsertRoute,
  } = useApiDefinitions(apiId);
  const { error: apiDefintionError } =
    (!apiDefinitonLoading && getApiDefintion(apiId, version)) || {};
  const components = (data?.data || null) as Component[];
  const selectedVersion = watch("version");

  if (apiDefinitonLoading || isLoading) {
    return <Loader />;
  }

  const versionNotFound = !apiDefinitonLoading && apiDefintionError;
  const onSubmit = async (formData: FormData) => {
    if (isExperimental) {
      return;
    }
    try {
      const newRoute: ApiRoute = {
        path: formData.path,
        method: formData.method,
        security: null,
        binding: {
          bindingType: "default",
          componentId: {
            componentId: formData.component,
            version: Number(formData.version),
          },
          corsPreflight: null,
          idempotencyKey: null,
          idempotencyKeyInput: null,
          response: formData.response,
          responseMappingInput: null,
          responseMappingOutput: null,
          workerName: formData.workerName,
          workerNameInput: null,
        },
      };
      const { success, error } = await upsertRoute(
        apiId,
        newRoute,
        version,
        defaultRoute?.path
      );

      if (!success) {
        return setError(error!);
      }
      const routeId = encodeURIComponent(`${newRoute.path}|${newRoute.method}`);
      if(!noRedirect){
        router.replace(`/apis/${apiId}/${routeId}${version? `?version=${version}`: ''}`);
      }
      setError(null);
      onSuccess?.();
      
    } catch (error) {
      console.error("Error creating route:", error);
    }
  };

  // const handleDelete = async (e: React.MouseEvent<HTMLButtonElement>) => {
  //   e.preventDefault();
  //   const response = await deleteRoute(defaultRoute!, version);
  //   console.log("response======>", response, defaultRoute);
  //   onSuccess?.();
  // };


  const validatePath = (value:string) => {
    if(!path || !value) {
      return true;
    }

    const requestKeys = (value?.match(requestPathRegex) || []).map((match=>match.split('.')[2]));
    const placeholders = (path?.match(pathPlaceholdersRegex) || []).map(match => match.slice(1, -1));

    if(requestKeys?.length === 0 && placeholders?.length === 0){
      return true;
    }  
    const missingKeys = requestKeys.filter((key) => !placeholders.includes(key));

    if (missingKeys.length > 0) {
      return ` Missing request path key(s): ${missingKeys.join(", ")}.`;
    }
    return true;
  };

  return (
    <Box
      sx={{
        width: "100%",
        margin: "auto",
      }}
    >
      {isExperimental && (
        <Typography
          variant="h5"
          fontWeight="bold"
          mb={2}
          className="text-red-600 text-center border border-red-300 rounded-lg shadow-lg font-thin"
        >
          Experimental. Coming soon!
        </Typography>
      )}

      {/* Title */}
      {/* <Box className="flex justify-between">
        {!isModal &&<Typography variant="h5" gutterBottom>
          {defaultRoute ? "Update" : "New"} Route
        </Typography>}
        {defaultRoute && (
          <Button onClick={handleDelete} className="justify-end">
            <DeleteForever />
          </Button>
        )}
      </Box> */}
      <Box className="flex justify-between items-center">
        {!isModal && (
          <Typography variant="h5" gutterBottom>
            {defaultRoute ? "Update" : "New"} Route
          </Typography>
        )}
        {/* {defaultRoute && (
          <Button onClick={handleDelete} className="ml-auto">
            <DeleteForever />
          </Button>
        )} */}
      </Box>

      {!isModal && <Divider className="bg-border" />}

      {/* HTTP Endpoint */}
      <Box sx={{ marginTop: isModal ? 1 : 4 }}>
        <Typography variant="subtitle1" gutterBottom>
          HTTP Endpoint
        </Typography>
        <Typography variant="body2" sx={{ color: "#AAA" }}>
          Each API Route must have a unique Method + Path combination
        </Typography>
        <Box className="flex flex-wrap gap-2 my-5">
          {[
            "Get",
            "Post",
            "Put",
            "Patch",
            "Delete",
            "Head",
            "Options",
            "Trace",
            "Connect",
          ].map((method) => (
            <Controller
              key={method}
              name="method"
              control={control}
              rules={{ required: "Method is mandatory!" }}
              render={({ field }) => (
                <Button
                  {...field}
                  onClick={() => field.onChange(method)}
                  variant="outlined"
                  sx={{
                    textTransform: "none",
                    padding: "4px 10px",
                    borderRadius: "8px",
                    backgroundColor:
                      watch("method") === method ? "#696969" : "transparent",
                    color: watch("method") === method ? "#FFF" : "#000000",
                    borderColor: "#555",
                    "&:hover": {
                      backgroundColor: "#555",
                      color: "#FFF",
                    },
                  }}
                  className="dark:text-white"
                >
                  {method}
                </Button>
              )}
            />
          ))}
        </Box>
        {errors && errors.method && (
          <Typography variant="inherit" color="error">
            {errors.method.message?.toString()}
          </Typography>
        )}
        <Typography variant="subtitle1">Path</Typography>
        <Controller
          name="path"
          control={control}
          //we can add regex for path
          rules={{
            required: "path is mandatory!",
            validate: (value: string) => {
              return value?.[0] != "/"
                ? "Api path should always start with /"
                : true;
            },
          }}
          render={({ field }) => (
            <TextField
              size="small"
              {...field}
              fullWidth
              variant="outlined"
              placeholder="/api/endpoint/{VARIABLE_NAME}"
              className="border border-input"
            />
          )}
        />
        {errors && errors.path && (
          <Typography variant="inherit" color="error">
            {errors.path.message?.toString()}
          </Typography>
        )}
      </Box>

      {/* Worker Binding */}
      <Box sx={{ marginTop: 4 }}>
        <Typography variant="subtitle1" gutterBottom>
          Worker Binding
        </Typography>
        <Box className="flex gap-2 mt-2 md:flex-row flex-col">
          <FormControl fullWidth>
            <ComponentSelect
              name={"component"}
              label={"Select component"}
              control={control}
              options={components}
              isLoading={isLoading}
            />
            <span className="text-muted-foreground text-xs mt-1">
              Select component
            </span>
            {errors && errors.component && (
              <Typography variant="inherit" color="error">
                {errors.component.message?.toString()}
              </Typography>
            )}
          </FormControl>

          <FormControl fullWidth>
            <ComponentSelect
              name={"version"}
              label={"Select version"}
              component={component}
              control={control}
              options={components}
              isLoading={isLoading}
            />
            <span className="text-muted-foreground text-xs mt-1">
              Select version
            </span>
            {errors && errors.version && (
              <Typography variant="inherit" color="error">
                {errors.version.message?.toString()}
              </Typography>
            )}
          </FormControl>
        </Box>
        <Box sx={{ marginTop: 4 }}>
          <Typography variant="subtitle1" gutterBottom>
            Worker Name
          </Typography>
          <Controller
            name="workerName"
            control={control}
            // TODO: need to add the rib expression sysntax regex.
            rules={{ required: "workername is mandatory!" ,
              validate: validatePath
            }}
            render={({ field }) => (
              <TextField
                {...field}
                fullWidth
                size="small"
                variant="outlined"
                placeholder="Worker name in Rib expression"
                multiline
                rows={2}
              />
            )}
          />
          {errors && errors.workerName && (
            <Typography variant="inherit" color="error">
              {errors.workerName.message?.toString()}
            </Typography>
          )}
        </Box>
        <div className="mt-5 flex items-center">
          <PopoverDemo Icon={<Info />}>
            <InterpolationExpressions />
          </PopoverDemo>
          <p className="text-muted-foreground text-xs">
            Interpolate variables into your Worker ID
          </p>
        </div>
      </Box>

      {/* Response */}
      <Box sx={{ marginTop: 4 }}>
        <Typography variant="subtitle1" gutterBottom>
          Response
        </Typography>
        <Controller
          name="response"
          // TODO: need to add the rib expression sysntax regex.
          rules={{ required: "response is mandatory!",
            validate: validatePath
           }}
          control={control}
          render={({ field }) => (
            <TextField
              {...field}
              fullWidth
              variant="outlined"
              label="Response"
              placeholder="Enter response in Rib expression"
              multiline
              rows={3}
            />
          )}
        />
        <div className="mt-5 flex items-center">
          <PopoverDemo Icon={<Info />}>
            <Box>
              <Typography variant="body2" gutterBottom>
                Available Functions
              </Typography>
              <Divider className="bg-border my-1" />
              {component &&
              (selectedVersion || typeof selectedVersion == "number") ? (
                <AvailableFunctions
                  compId={component}
                  version={selectedVersion}
                />
              ) : (
                <Typography variant="body2" padding={1}>
                  No Functions Available
                </Typography>
              )}
            </Box>
          </PopoverDemo>
          <p className="text-muted-foreground text-xs">Available functions</p>
        </div>
        {errors && errors.response && (
          <Typography variant="inherit" color="error">
            {errors.response.message?.toString()}
          </Typography>
        )}
      </Box>
      {(versionNotFound || error) && (
        // TODO we need error type veraint as we are using it many places
        <Typography color="error">{versionNotFound || error}</Typography>
      )}
      {/* Buttons */}
      <Box
        sx={{ marginTop: 4, display: "flex", justifyContent: "space-between" }}
      >
        <Button2 variant="dropdown" size="lg" onClick={() => reset()}>
          Clear
        </Button2>
        <Button2
          variant="primary"
          size="lg"
          onClick={handleSubmit(onSubmit)}
          disabled={!!versionNotFound}
        >
          {defaultRoute ? "Update" : "Create"} Route
        </Button2>
      </Box>
    </Box>
  );
};

export default NewRouteForm;
