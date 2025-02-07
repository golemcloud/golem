import * as Imports from "@/components/imports";
const { Box, Typography, Button2, AddIcon } = Imports;

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
      textAlign='center'
      className='border-dashed border rounded-md m-auto p-16'
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
