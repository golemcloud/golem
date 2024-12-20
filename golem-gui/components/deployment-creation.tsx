"use client";
import {
  Box,
  Container,
  Divider,
  InputLabel,
  MenuItem,
  Paper,
  Select,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import { Button } from "@/components/ui/button";
import React, { useState } from "react";
import AddCircleOutlineIcon from "@mui/icons-material/AddCircleOutline";
import DeleteIcon from "@mui/icons-material/Delete";
import { fetcher, getErrorMessage } from "@/lib/utils";
import useSWR from "swr";
import { ApiDefinition } from "@/types/api";
import { useForm, Controller } from "react-hook-form";

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
  onCreation,
}: {
  onCreation: () => void;
}) {
  const [error, setError] = useState("");
  const { data, isLoading } = useSWR(`?path=api/definitions`, fetcher);
  let apiDefintions = (data?.data || []) as ApiDefinition[];
  apiDefintions = apiDefintions.filter((api) => api.draft);

  const { control, handleSubmit, register, watch, setValue } =
    useForm<FormValues>({
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
    const newDeploy = {
      apiDefinitions: formData.definitions,
      site: {
        host: formData.domain,
        subdomain: formData.subdomain,
      },
    };

    const response = await fetcher("?path=api/deployments/deploy", {
      method: "POST",
      body: JSON.stringify(newDeploy),
      headers: {
        "content-type": "application/json",
      },
    });

    if (response.status !== 200) {
      return setError(getErrorMessage(response.data));
    }
    console.log("response", response);
    onCreation();
  };

  return (
    <>
      <Typography gutterBottom className="font-bold" variant="h3">
        Deploy API
      </Typography>
      <Typography gutterBottom>Deploy your API on Golem Cloud</Typography>

      <form onSubmit={handleSubmit(onSubmit)}>
        <Stack className="w-full">
          <InputLabel>Domain</InputLabel>
          <TextField {...register("domain")} name="domain" required />

          <InputLabel>Subdomain</InputLabel>
          <TextField {...register("subdomain")} name="subdomain" required />
        </Stack>

        <Typography gutterBottom className="font-bold" marginTop={2}>
          API Definitions
        </Typography>
        <Stack
          direction="row"
          justifyContent={"space-between"}
          alignItems={"center"}
        >
          <Typography>Include one or more API definitions to deploy</Typography>
          <Button
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
        <Divider className="my-2" />

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
            render={({ field }) => {
              return (
                <>
                  {definitions.map((definition, index) => {
                    return (
                      <Stack
                        direction="row"
                        justifyContent={"space-between"}
                        alignItems={"center"}
                        gap={2}
                      >
                        <Stack className="w-full">
                          {index === 0 && <InputLabel>Definition</InputLabel>}
                          <Select
                            name={`definition[${index}].id`}
                            variant="outlined"
                            disabled={isLoading || apiDefintions?.length === 0}
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
                              apiDefintions?.map(
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
                            value={definitions[index].version}
                            variant="outlined"
                            disabled={isLoading || apiDefintions?.length === 0}
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
                              apiDefintions
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
                            variant={"destructive"}
                            size={"icon"}
                            onClick={(e) => {
                              e.preventDefault();
                              setValue("definitions", [
                                ...definitions.slice(0, index),
                                ...definitions.slice(index + 1),
                              ]);
                            }}
                          >
                            <DeleteIcon />
                          </Button>
                        </Stack>
                      </Stack>
                    );
                  })}
                </>
              );
            }}
          />
        </Stack>
        {error && <Typography className="text-red-500">{error}</Typography>}
        <Stack>
          <Button type="submit" className="mt-4 self-end">
            Deploy
          </Button>
        </Stack>
      </form>
    </>
  );
}
