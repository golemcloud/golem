import { Typography } from "@mui/material";
import { Box } from "lucide-react";
import { Button2 } from "../ui/button";
import AddIcon from "@mui/icons-material/Add";


export default function Empty({
  heading,
  subheading,
  onClick,
}: {
  heading: string;
  subheading: string;
  onClick: () => void;
}) {
  return (
    <Box
      className='border-dashed text-center border rounded-md m-auto p-16'
    >
      <Typography variant='h6' className='text-foreground'>
        {heading}
      </Typography>
      <Typography variant='body2' className='text-muted-foreground'>
        {subheading}
      </Typography>
      <br />
      <Button2
        variant='primary'
        size='md'
        startIcon={<AddIcon />}
        onClick={onClick}
      >
        Create New
      </Button2>
    </Box>
  );
}
