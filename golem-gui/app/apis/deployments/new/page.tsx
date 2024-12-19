"use client";
import {
  Box,
  Divider,
  FormControl,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  TextField,
  Typography,
} from "@mui/material";
// import AddIcon from "@mui/icons-material/Add";
import { Button } from "@/components/ui/button";
// import { Loader } from "lucide-react";
import React from "react";
import AddCircleOutlineIcon from "@mui/icons-material/AddCircleOutline";
import DeleteIcon from "@mui/icons-material/Delete";

export default function DeploymentPage() {
  //  const [isLoading, setLoading] = useState(true);

  const handleDelete = async () => {};

  return (
    <Box className={"mx-auto md:max-w-[40%] lg:max-w-[40%]"}>
      <Typography variant="h5" gutterBottom>
        Deploy API
      </Typography>
      <Typography gutterBottom className="">
        Deploy your API on Golem Cloud
      </Typography>

      <form>
        <TextField
          fullWidth
          variant="outlined"
          label="Domain"
          name="domain"
          placeholder="Domain"
          InputLabelProps={{ style: { color: "#AAA" } }}
          InputProps={{
            style: { color: "#FFF", borderColor: "#555" },
          }}
          sx={{ marginTop: 2 }}
        />
        <TextField
          fullWidth
          variant="outlined"
          label="Subdomain"
          name="subdomain"
          placeholder="Subdomain"
          InputLabelProps={{ style: { color: "#AAA" } }}
          InputProps={{
            style: { color: "#FFF", borderColor: "#555" },
          }}
          sx={{ marginTop: 2 }}
        />
        <Typography gutterBottom  variant="subtitle1" className="font-bold" marginTop={2}>
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
         <FormControl fullWidth>
            <InputLabel sx={{ color: "#AAA" }}>Definition</InputLabel>
            <Select
              defaultValue=""
              variant="outlined"
              label="Definition"
              name="definition"
              sx={{
                color: "#FFF",
                "& .MuiOutlinedInput-notchedOutline": { borderColor: "#555" },
                "&:hover .MuiOutlinedInput-notchedOutline": {
                  borderColor: "#888",
                },
              }}
            >
              <MenuItem value="component1">Component 1</MenuItem>
              <MenuItem value="component2">Component 2</MenuItem>
            </Select>
          </FormControl>
          <FormControl fullWidth>
            <InputLabel sx={{ color: "#AAA" }}>Vesrion</InputLabel>
            <Select
              defaultValue=""
              variant="outlined"
              label="Vesrion"
              name="version"
              sx={{
                color: "#FFF",
                "& .MuiOutlinedInput-notchedOutline": { borderColor: "#555" },
                "&:hover .MuiOutlinedInput-notchedOutline": {
                  borderColor: "#888",
                },
              }}
            >
              <MenuItem value="component1">Component 1</MenuItem>
              <MenuItem value="component2">Component 2</MenuItem>
            </Select>
          </FormControl>
          <Stack>
            <Button
              variant={"destructive"}
              type="button"
              size={"icon"}
              onClick={handleDelete}
            >
              <DeleteIcon />
            </Button>
          </Stack>
        </Stack>
      </form>
    </Box>
  );
}
