"use client";
import {
  Divider,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import { Button2 as Button } from "@/components/ui/button";
import React, { useState } from "react";
import AddCircleOutlineIcon from "@mui/icons-material/AddCircleOutline";
import { ApiDefinition, ApiDeployment } from "@/types/api";
import { useForm, Controller } from "react-hook-form";
import useApiDefinitions from "@/lib/hooks/use-api-definitons";
import { addNewApiDeployment } from "@/lib/hooks/use-api-deployments";
import { getFormErrorMessage } from "@/lib/utils";
import RemoveIcon from "@mui/icons-material/Remove";

interface KeyValue {
  id: string;
  version: string;
}

interface FormValues {
  domain: string;
  subdomain: string;
  definitions: KeyValue[];
}

export default function DeploymentCreationPage({
  addDeployment,
  apiId,
  onSuccess,
}: {
  onSuccess?: () => void;
  apiId?: string;
  addDeployment?: (newDeploy: ApiDeployment) => Promise<{
    success: boolean;
    data?: ApiDeployment | null;
    error?: string | null;
  }>;
}) {
  const [error, setError] = useState<string | null>(null);
  const { apiDefinitions: data, isLoading } = useApiDefinitions(apiId);
  const apiDefinitions = data.filter((api) => api.draft);

  const uniqueApiDefintions = Object.values(apiDefinitions?.reduce<Record<string,ApiDefinition>>((obj, apiDefinition:ApiDefinition)=>{
    obj[apiDefinition.id] = apiDefinition;
    return obj;
  }, {}) || {})

  const {
    control,
    handleSubmit,
    watch,
    setValue,
    formState: { errors },
  } = useForm<FormValues>({
    defaultValues: {
      domain: "",
      subdomain: "",
      definitions: [{ id: "", version: "" }] as KeyValue[],
    },
  });

  const definitions = watch("definitions");
  const onSubmit = async (formData: FormValues) => {
    console.log("Form submitted:", formData);
    // Add your submission logic here
    const newDeploy: ApiDeployment = {
      apiDefinitions: formData.definitions,
      site: {
        host: formData.domain,
        subdomain: formData.subdomain,
      },
    };

    if (addDeployment) {
      const { error } = await addDeployment(newDeploy);
      return setError(error || null);
    }
    const { error } = await addNewApiDeployment(newDeploy);
    setError(error || null);
    if (error) {
      return;
    }
    onSuccess?.();
  };

  return (
    <div>
      <Typography gutterBottom>Deploy your API on Golem Cloud</Typography>
      <form onSubmit={handleSubmit(onSubmit)}>
        <Stack gap={2} className="w-full">
          <Stack>
            <InputLabel>Domain</InputLabel>
            <Controller
              name="domain"
              control={control}
              rules={{ required: "Domain is required.",
                pattern: {
                  value: /^(?!:\/\/)([a-zA-Z0-9-_]+\.)+[a-zA-Z]{2,}$/,
                  message: "Please enter a valid domain.",
                }}}
              render={({ field }) => <TextField size="small" {...field} />}
            />

            <Typography variant="caption" color="error">
              {getFormErrorMessage("domain", errors)}
            </Typography>
          </Stack>
          <Stack>
            <InputLabel>Subdomain</InputLabel>
            <Controller
              name="subdomain"
              control={control}
              rules={{ required: "Subdomain is required." }}
              render={({ field }) => <TextField size="small" {...field} />}
            />
            <Typography variant="caption" color="error">
              {getFormErrorMessage("subdomain", errors)}
            </Typography>
          </Stack>
        </Stack>

        <Typography gutterBottom className="font-bold" marginTop={2}>
          API Definitions
        </Typography>
        <Stack
          direction="row"
          justifyContent={"space-between"}
          alignItems={"end"}
        >
          <Typography>Include one or more API definitions to deploy</Typography>
          <Button
            variant="primary"
            size="md"
            onClick={(e) => {
              e.preventDefault();
              setValue("definitions", [
                ...definitions,
                { id: "", version: "" },
              ]);
            }}
          >
            Add <AddCircleOutlineIcon />
          </Button>
        </Stack>
        <Divider className="my-2 bg-border" />

        <Stack>
          <Controller
            name="definitions"
            control={control}
            rules={{
              validate: (definitions) =>
                definitions?.filter((def) => def.id && def.version).length >
                  0 ||
                "At least one valid API definition with a version is required.",
            }}
            render={() => {
              return (
                <>
                  {definitions.map((definition, index) => {
                    return (
                      <Stack
                        key={`${definition.id}__${index}`}
                        direction="row"
                        justifyContent={"space-between"}
                        alignItems={"center"}
                        gap={2}
                      >
                        <Stack className="w-full">
                          {index === 0 && <InputLabel>Definition</InputLabel>}
                          <Select
                            size="small"
                            name={`definition[${index}].id`}
                            variant="outlined"
                            disabled={isLoading || uniqueApiDefintions?.length === 0}
                            value={definitions[index].id}
                            onChange={(e) => {
                              const newDef = {
                                ...definition,
                                id: e.target.value,
                              } as KeyValue;
                              setValue("definitions", [
                                ...definitions.slice(0, index),
                                newDef,
                                ...definitions.slice(index + 1),
                              ]);
                            }}
                          >
                            {!isLoading &&
                              uniqueApiDefintions?.map(
                                (apiDefintion: ApiDefinition) => (
                                  <MenuItem
                                    key={apiDefintion?.id}
                                    value={apiDefintion?.id}
                                  >
                                    {apiDefintion.id}
                                  </MenuItem>
                                )
                              )}
                          </Select>
                        </Stack>
                        <Stack className="w-full">
                          {index === 0 && <InputLabel>Version</InputLabel>}
                          <Select
                            name={`definition[${index}].version`}
                            size="small"
                            value={definitions[index].version}
                            disabled={isLoading || apiDefinitions?.length === 0}
                            onChange={(e) => {
                              const newDef = {
                                ...definition,
                                version: e.target.value,
                              } as KeyValue;
                              setValue("definitions", [
                                ...definitions.slice(0, index),
                                newDef,
                                ...definitions.slice(index + 1),
                              ]);
                            }}
                          >
                            {!isLoading &&
                              apiDefinitions
                                ?.filter((api) => api.id === definition.id)
                                ?.map((apiDefintion: ApiDefinition) => (
                                  <MenuItem
                                    key={apiDefintion?.version}
                                    value={apiDefintion?.version}
                                  >
                                    {apiDefintion.version}
                                  </MenuItem>
                                ))}
                          </Select>
                        </Stack>
                        <Stack>
                          <InputLabel>Delete</InputLabel>
                          <Button
                            variant="error"
                            size="sm"
                            onClick={(e) => {
                              e.preventDefault();
                              setValue("definitions", [
                                ...definitions.slice(0, index),
                                ...definitions.slice(index + 1),
                              ]);
                            }}
                          >
                            <RemoveIcon />
                          </Button>
                        </Stack>
                      </Stack>
                    );
                  })}
                </>
              );
            }}
          />
          <Typography variant="caption" color="error">
            {getFormErrorMessage("definitions", errors)}
          </Typography>
        </Stack>
        {error && <Typography className="text-red-500">{error}</Typography>}
        <Stack>
          <Button
            type="submit"
            variant="primary"
            size="lg"
            className="mt-4 self-end"
          >
            Deploy
          </Button>
        </Stack>
      </form>
    </div>
  );
}
