"use client";
import {
  Box,
  Divider,
  InputLabel,
  Select,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
import { Button } from "@/components/ui/button";
import React from "react";
import AddCircleOutlineIcon from "@mui/icons-material/AddCircleOutline";
import DeleteIcon from "@mui/icons-material/Delete";
import { fetcher } from "@/lib/utils";

export default function DeploymentCreationPage({
  onCreation,
}: {
  onCreation: () => void;
}) {
  const { data, isLoading } = useSWR(`?path=api/definitions`, fetcher);

  return (
    <Box className={"mx-auto md:max-w-[40%] lg:max-w-[40%]"}>
      <Typography gutterBottom className="font-bold" variant="h3">
        Deploy API
      </Typography>
      <Typography gutterBottom className="">
        Deploy your API on Golem Cloud
      </Typography>

      <form>
        <Stack className="w-full">
          <InputLabel>Domain</InputLabel>
          <TextField name="domain" />
          <InputLabel>Subdomain</InputLabel>
          <TextField name="subdomain" />
        </Stack>
        <Typography gutterBottom className="font-bold" marginTop={2}>
          Api Definitions
        </Typography>
        <Stack
          direction="row"
          justifyContent={"space-between"}
          alignItems={"center"}
        >
          <Typography gutterBottom>
            include one or more Api defintions to deploy
          </Typography>
          <Button>
            Add <AddCircleOutlineIcon />
          </Button>
        </Stack>
        <Divider className="my-2" />
        <Stack
          direction="row"
          justifyContent={"space-between"}
          alignItems={"center"}
          gap={2}
        >
          <Stack className="w-full">
            <InputLabel>Defintion</InputLabel>
            <Select />
          </Stack>
          <Stack className="w-full">
            <InputLabel>Vesrion</InputLabel>
            <Select />
          </Stack>
          <Stack>
            <InputLabel>{"Delete"}</InputLabel>
            <Button variant={"destructive"} size={"icon"}>
              <DeleteIcon />
            </Button>
          </Stack>
        </Stack>
      </form>
    </Box>
  );
}
function useSWR(arg0: string, fetcher: any): { data: any; isLoading: any } {
  throw new Error("Function not implemented.");
}
