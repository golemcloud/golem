"use client";

import React, { useState } from "react";
import { useForm, Controller } from "react-hook-form";
import {
  Box,
  Typography,
  RadioGroup,
  FormControlLabel,
  Radio,
  TextField,
} from "@mui/material";
import UploadFileIcon from "@mui/icons-material/UploadFile";
import CloudUploadIcon from "@mui/icons-material/CloudUpload";
import { addNewcomponent } from "../../lib/hooks/use-component";
import JSZip from "jszip";
import { getFormErrorMessage } from "../../lib/utils";
import { Button2 } from "../ui/button";
import { FileOrganizer } from "./file-organizer";

export interface FileEntity {
  id: string;
  name: string;
  size: number;
  type: "file" | "folder";
  parentId: string | null;
  isLocked: boolean;
  fileObject?: File;
}
type FormData = {
  name: string;
  componentType?: "Durable" | "Ephemeral";
  component?: File | null;
  files: File[];
};

type Props = {
  mode: "create" | "update";
  onSubmitSuccess?: () => void;
  componentId?: string;
  initialValues?: Partial<FormData>;
};

export default function ComponentForm({
  mode,
  onSubmitSuccess,
  initialValues,
  componentId,
}: Props) {
  const isCreateMode = mode === "create";

  const {
    handleSubmit,
    control,
    formState: { errors },
  } = useForm<FormData>({
    defaultValues: {
      name: "",
      componentType: "Durable",
      component: null,
      files: [],
      ...initialValues,
    },
  });

  const [error, setError] = React.useState<string | null>(null);
  const [fileSystem, setFileSystem] = useState<FileEntity[] | []>([]);

  function constructFilePath(file: FileEntity, allFiles: FileEntity[]): string {
    if (!file.parentId) {
      return `/${file.name}`;
    }
    const parentFile = allFiles.find((f) => f.id === file.parentId);
    return parentFile ? `${constructFilePath(parentFile, allFiles)}/${file.name}` : file.name;
  }
  
  function gatherFileMetadata(allFiles: FileEntity[]): { values: { path: string; permissions: string }[] } {
    const metadata = allFiles
      .filter((file) => file.type !== "folder")
      .map((file) => ({
        path: constructFilePath(file, allFiles),
        permissions: file.isLocked ? "read-only" : "read-write",
      }));
  
    return { values: metadata };
  }
  
  async function populateZip(zipInstance: JSZip, parentId: string | null, fileSystem: FileEntity[]) {
    const childFiles = fileSystem.filter((file) => file.parentId === parentId);
  
    for (const file of childFiles) {
      if (file.type === "folder") {
        const newFolder = zipInstance.folder(file.name);
        if (newFolder) {
          await populateZip(newFolder, file.id, fileSystem);
        }
      } else if (file.type === "file" && file.fileObject) {
        zipInstance.file(file.name, file.fileObject);
      }
    }
  }


  const onSubmit = async (data: FormData) => {
    console.log("Form submitted:", data);

    const zip = new JSZip();
    await populateZip(zip, null,fileSystem);
    const blob = await zip.generateAsync({ type: "blob" });

    try {
      const formData = new FormData();
      if (isCreateMode) {
        formData.append("name", data.name);
      }
      if (isCreateMode) {
        formData.append("componentType", data.componentType || "Durable");
      }
      if (data.component) {
        formData.append("component", data.component);
      }

      formData.append(
        "filesPermissions",
        JSON.stringify(gatherFileMetadata(fileSystem))
      );

      formData.append("files", blob, "temp.zip");

      const { error } = await addNewcomponent(formData, componentId, mode);
      setError(error || null);
      onSubmitSuccess?.();
    } catch (err) {
      console.error("Error during submission:", err);
      setError("Something went wrong! Please try again.");
    }
  };

  return (
    <form onSubmit={handleSubmit(onSubmit)}>
      {isCreateMode && (
        <>
          <Box>
            <Typography variant="body2" className="text-foreground">Component Name</Typography>
            <Controller
              name="name"
              rules={{ required: "Name is mandatory!" }}
              control={control}
              render={({ field }) => (
                <TextField
                  placeholder="Enter component name"
                  size="small"
                  variant="outlined"
                  fullWidth
                  {...field}
                />
              )}
            />
          </Box>
          <Typography variant="caption" color="error">
            {getFormErrorMessage("name", errors)}
          </Typography>
        </>
      )}

      {/* Type Selection */}
      {isCreateMode && (
        <Box my={3}>
          <Typography variant="body1" mb={1}>
            Type
          </Typography>
          <Controller
            name="componentType"
            control={control}
            render={({ field }) => (
              <RadioGroup row {...field}>
                <FormControlLabel
                  value="Durable"
                  control={<Radio className="text-foreground" />}
                  label={
                    <Box>
                      <Typography>
                        <b>Durable</b>
                      </Typography>
                      <Typography
                        variant="caption"
                        className="text-muted-foreground"
                      >
                        Workers are persistent and executed with transactional
                        guarantees
                      </Typography>
                    </Box>
                  }
                />
                <FormControlLabel
                  value="Ephemeral"
                  control={<Radio className="text-foreground" />}
                  label={
                    <Box>
                      <Typography>
                        <b>Ephemeral</b>
                      </Typography>
                      <Typography
                        variant="caption"
                        className="text-muted-foreground"
                      >
                        Workers are transient and executed normally
                      </Typography>
                    </Box>
                  }
                />
              </RadioGroup>
            )}
          />
        </Box>
      )}
      {/* WASM File Upload */}
      <Box mb={3}>
        <Typography variant="body1">Upload WASM File</Typography>
        <Typography variant="caption" mb={1} color="gray">
          The compiled WASM binary of your component.
        </Typography>
        <Controller
          name="component"
          rules={{
            required: "WASM file is mandatory!",
            validate: (value) =>
              value?.name.endsWith('.wasm') || "Invalid file type!",
          }}
          control={control}
          render={({ field }) => (
            <>
              <input
                type="file"
                accept=".wasm"
                hidden
                id="wasm-upload"
                onChange={(e) =>
                  field.onChange(e.target.files ? e.target.files[0] : null)
                }
              />
              <label htmlFor="wasm-upload">
                <Box
                  textAlign="center"
                  p={2}
                  borderRadius="8px"
                  className="cursor-pointer hover:border-[#888] border-dashed border-2"
                >
                  <UploadFileIcon sx={{ fontSize: 50 }} />
                  <Typography variant="body2" sx={{ mt: 1 }}>
                    {field.value ? field.value.name : "Upload Component WASM"}
                  </Typography>
                  <Typography variant="caption">File up to 50MB</Typography>
                </Box>
              </label>
              <Typography variant="caption" color="error">
                {getFormErrorMessage("component", errors)}
              </Typography>
            </>
          )}
        />
      </Box>
      <FileOrganizer files={fileSystem} setFiles={setFileSystem} />
      {error && <Typography className="text-red-500">{error}</Typography>}

      {/* Submit Button */}
      <Box display="flex" justifyContent="flex-end" mt={3}>
        <Button2
          type="submit"
          variant="primary"
          size="lg"
          startIcon={<CloudUploadIcon />}
        >
          {isCreateMode ? "Create" : "Update"}
        </Button2>
      </Box>
    </form>
  );
}
