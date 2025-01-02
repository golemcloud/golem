import React from "react";
import { Modal, Box, Typography, IconButton } from "@mui/material";
import CloseIcon from "@mui/icons-material/Close";

interface CustomModalProps {
  open: boolean;
  onClose: () => void;
  children: React.ReactNode;
  heading?: string;
  subheading?: string;
}

const CustomModal: React.FC<CustomModalProps> = ({ open, onClose, children, heading, subheading }) => {
  return (
    <Modal open={open} onClose={onClose}>
      <Box
        sx={{
          position: "absolute",
          top: "50%",
          left: "50%",
          transform: "translate(-50%, -50%)",
          width: "70%",
          minWidth: "fit-content",
          maxWidth: 600,
          color: "white",
          boxShadow: 24,
          p: 0,
          borderRadius: 2,
          minHeight:'300px',
          maxHeight: "calc(100vh - 120px)",
          overflowY: "auto",
        }}
        className="dark:bg-[#1e1e1e] bg-[#fff] dark:text-[#fff] text-[#333] border"
      >
        {heading && (
          <Box
            sx={{
              position: "sticky",
              top: 0,
              zIndex: 1000,
              px: 2,
              py:1,
            }}
            className="dark:bg-[#1e1e1e] bg-[#fff] dark:text-[#fff] text-[#333] border-b border-border dark:border-[#333]"
          >
            <Box
              sx={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
              }}
            >
              <Typography variant="h5" component="h2">
                {heading}
              </Typography>
              <IconButton
                onClick={onClose}
              className="dark:text-[#fff] text-[#333]"
              >
                <CloseIcon />
              </IconButton>
            </Box>
            {subheading && (
              <Typography
                variant="body1"
                sx={{
                  mt: 1,
                  color: "#cccccc",
                }}
              >
                {subheading}
              </Typography>
            )}
          </Box>
        )}
        <Box sx={{ p: 2 }}>{children}</Box>
      </Box>
    </Modal>
  );
};

export default CustomModal;
