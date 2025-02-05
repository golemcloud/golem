import {FolderStructure} from "@/components/file-manager.tsx";

const data = [
    {
        key: "deb45168f29df0d0d70c5679d9e525881d4bc010e812eeb1d992febab02c35f1",
        path: "/__MACOSX/temp/._name.json",
        permissions: "read-only",
    },
    {
        key: "9ae32b02af5738242717782409d5e64544ca0eaf2575bddf8121446f3b958372",
        path: "/temp/name.json",
        permissions: "read-only",
    },
]

export default function FileManager() {
    return (
        <div className="container mx-auto p-4">
            <FolderStructure data={data}/>
        </div>
    )
}

