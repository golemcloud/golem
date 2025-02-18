

import { Box, Typography } from "@mui/material";
import { ReactElement } from "react";


interface NotFoundCardProps{
    heading:string,
    subheading:string,
    icon:ReactElement
}
export default function NotFoundCard({heading,subheading,icon}:NotFoundCardProps){


    return (
        <Box
        sx={{
          textAlign: "center",
          py: 8,
          borderRadius: 2,
        }}
        className="border-dashed border-border border-2"
      >
        <Box display="flex" justifyContent="center" mb={2}>
          {icon}
        </Box>
        <Typography variant="h6" fontWeight="bold" className="text-foreground" >
         {heading}
        </Typography>
        <Typography variant="body2" className="text-muted-foreground">
          {subheading}
        </Typography>
      </Box>
    )
}