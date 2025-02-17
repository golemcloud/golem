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
          width: "95%",
          minWidth: "fit-content",
          maxWidth: 600,
          color: "white",
          boxShadow: 24,
          p: 0,
          borderRadius: 2,
          minHeight:'200px',
          maxHeight: "calc(100vh - 120px)",
          overflowY: "auto",
        }}
        className="bg-card_secondary text-foreground border border-border"
      >
        {heading && (
          <Box
            sx={{
              position: "sticky",
              top: 0,
              zIndex: 10,
              px: 2,
              py:1,
            }}
            className="bg-card_secondary text-foreground border-b border-border"
          >
            <Box
              sx={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
              }}
            >
              <Typography className="md:text-2xl sm:text-xl text-lg">
                {heading}
              </Typography>
              <IconButton onClick={onClose}>
                <CloseIcon  className="text-muted-foreground"/>
              </IconButton>
            </Box>
            {subheading && (
              <Typography variant="body2" className="text-muted-foreground">
                {subheading}
              </Typography>
            )}
          </Box>
        )}
        <Box className="md:p-5 sm:p-3 p-2">{children}</Box>
      </Box>
    </Modal>
  );
};

export default CustomModal;
