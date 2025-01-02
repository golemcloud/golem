import React, {useRef, useState} from "react";
import {CircleX, FileUp} from "lucide-react";
import {Button} from "@/components/ui/button";

export interface UploaderProps
    extends React.HTMLAttributes<HTMLInputElement> {
}

const WasmUploader: React.FC<UploaderProps> = ({className, ...props}) => {
    const [file, setFile] = useState<File | null>(null);
    const fileInputRef = useRef<HTMLInputElement>(null);

    const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
        const selectedFile = e.target.files?.[0];
        if (selectedFile && selectedFile.size <= 50 * 1024 * 1024) {
            setFile(selectedFile);
        }
    };

    return (
        <div className={className}>
            <div
                className="border-2 border-dashed border-gray-200 rounded-lg p-8 cursor-pointer hover:border-gray-400"
                onClick={() => fileInputRef.current?.click()}
            >
                <div className="flex flex-col items-center justify-center text-center">
                    <FileUp className="h-8 w-8 text-gray-400 mb-3"/>
                    <p className="font-medium mb-1 flex items-center space-x-2">
                        {file ? (
                            <>
                                <span>{file.name}</span>
                                <Button
                                    variant="link"
                                    className="h-9 w-9 p-0"
                                    onClick={() => setFile(null)}
                                >
                                    <CircleX className="h-5 w-5"/>
                                </Button>
                            </>
                        ) : (
                            "Upload Component WASM"
                        )}
                    </p>
                    <p className="text-sm text-gray-500 mb-4">File up to 50MB</p>
                    <input
                        type="file"
                        accept="application/wasm,.wasm"
                        onChange={handleFileSelect}
                        className="hidden"
                        ref={fileInputRef}
                        {...props}
                    />
                </div>
            </div>
        </div>
    );
};

WasmUploader.displayName = "WasmUploader";

export {WasmUploader};


