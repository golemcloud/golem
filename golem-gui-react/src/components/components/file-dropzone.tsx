import { useDropzone } from "react-dropzone";

export function FileDropzone({ onDrop }: { onDrop: (acceptedFiles: File[]) => void }) {
  const { getRootProps, getInputProps, isDragActive } = useDropzone({ onDrop });

  return (
    <div
      {...getRootProps()}
      className={`border-2 border-dashed cursor-pointer hover:border-[#888] rounded-md p-8 text-center ${
        isDragActive ? "border-[#888]" : "border"
      }`}
    >
      <input {...getInputProps()} />
      <p>Select or Drop files</p>
    </div>
  );
}
