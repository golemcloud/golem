import React, { useState } from "react";
import { Button, Menu, MenuItem, Box } from "@mui/material";
import AddIcon from "@mui/icons-material/Add";
import ArrowDropDownIcon from "@mui/icons-material/ArrowDropDown";

const DropdownButton = () => {
  const [anchorEl, setAnchorEl] = useState(null);

  const handleClick = (event:any) => {
    setAnchorEl(event.currentTarget);
  };

  const handleClose = () => {
    setAnchorEl(null);
  };

  return (
    <Box sx={{ display: "inline-block", background: "#000", p: 1, borderRadius: "4px", border: "1px solid #444" }}>
      <Button
        variant="outlined"
        sx={{
          color: "#FFF",
          border: "none",
          "&:hover": { backgroundColor: "transparent" },
          fontSize: "20px",
        }}
        onClick={handleClick}
        startIcon={<AddIcon />}
        endIcon={<ArrowDropDownIcon />}
      />
      <Menu anchorEl={anchorEl} open={Boolean(anchorEl)} onClose={handleClose}>
        <MenuItem onClick={handleClose}>Option 1</MenuItem>
        <MenuItem onClick={handleClose}>Option 2</MenuItem>
        <MenuItem onClick={handleClose}>Option 3</MenuItem>
      </Menu>
    </Box>
  );
};

export default DropdownButton;
