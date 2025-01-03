import {useNavigate, useParams} from "react-router-dom";
import {Plugin} from "@/types";
import {useEffect, useState} from "react";
import {API} from "@/service";
import {ArrowLeft, Component, Globe, Trash2} from "lucide-react";
import {Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle} from "@/components/ui/card";
import {Select, SelectContent, SelectItem, SelectTrigger, SelectValue} from "@/components/ui/select.tsx";
import {Button} from "@/components/ui/button.tsx";
import {
    AlertDialog,
    AlertDialogAction,
    AlertDialogCancel,
    AlertDialogContent,
    AlertDialogDescription,
    AlertDialogFooter,
    AlertDialogHeader,
    AlertDialogTitle,
    AlertDialogTrigger
} from "@/components/ui/alert-dialog.tsx";
import {Badge} from "@/components/ui/badge.tsx";
import {Separator} from "@/components/ui/separator.tsx";


export function PluginView() {
    const {pluginId, version} = useParams();
    const navigate = useNavigate();
    const [plugin, setPlugin] = useState<Plugin[]>();
    const [ver, setVer] = useState(version!);
    const [currentVersion, setCurrentVersion] = useState<Plugin>(null);

    useEffect(() => {
        API.getPluginByName(pluginId!).then((res) => {
            setPlugin(res);
            if (version) {
                res.forEach(p => {
                    if (p.version == version) {
                        setCurrentVersion(p)
                        setVer(p.version)
                    }
                })
            } else {
                setVer(res[0].version)
                setCurrentVersion(res[0])
            }
        });
    }, [pluginId, version]);
    const handleVersionChange = (version: string) => {
        setVer(version)
        navigate(`/plugins/${currentVersion.name}/${version}`)
    }

    const handleDelete = () => {
        // In a real application, you would call an API to delete the version
        console.log(`Deleting version ${version} of plugin ${currentVersion.name}`)
        navigate(`/plugins/${currentVersion.name}`)
    }
    return (
        <div className="container mx-auto py-10">
            {currentVersion &&
                <Card className="w-full max-w-4xl mx-auto">
                    <CardHeader className={"p-4"}>
                        <div className="flex justify-between items-start">
                            <div className="flex">
                                <Button variant="link" onClick={() => navigate(`/plugins`)}>
                                    <ArrowLeft className="w-4 h-4 mr-2"/>
                                </Button>
                                <CardTitle className="text-3xl font-bold">{currentVersion.name}</CardTitle>
                            </div>
                            <div className="flex items-center space-x-2">
                                <Select onValueChange={handleVersionChange} value={ver}>
                                    <SelectTrigger className="w-[180px]">
                                        <SelectValue placeholder="Select version"/>
                                    </SelectTrigger>
                                    <SelectContent>
                                        {plugin && plugin.map((v) => (
                                            <SelectItem key={v.version} value={v.version}>
                                                {v.version}
                                            </SelectItem>
                                        ))}
                                    </SelectContent>
                                </Select>
                                <AlertDialog>
                                    <AlertDialogTrigger asChild>
                                        <Button variant="destructive" size="icon">
                                            <Trash2 className="h-4 w-4"/>
                                        </Button>
                                    </AlertDialogTrigger>
                                    <AlertDialogContent>
                                        <AlertDialogHeader>
                                            <AlertDialogTitle>Are you absolutely sure?</AlertDialogTitle>
                                            <AlertDialogDescription>
                                                This action cannot be undone. This will permanently delete the
                                                {version} version of {currentVersion.name}.
                                            </AlertDialogDescription>
                                        </AlertDialogHeader>
                                        <AlertDialogFooter>
                                            <AlertDialogCancel>Cancel</AlertDialogCancel>
                                            <AlertDialogAction onClick={handleDelete}>Delete</AlertDialogAction>
                                        </AlertDialogFooter>
                                    </AlertDialogContent>
                                </AlertDialog>
                            </div>
                        </div>
                        <CardDescription className="text-lg mt-2">{currentVersion.description}</CardDescription>
                    </CardHeader>
                    <Separator className="my-4"/>
                    <CardContent className="space-y-6">
                        <div>
                            <h3 className="font-semibold mb-2">Details</h3>
                            <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                                <div className={"flex"}>
                                    <h4>Homepage:</h4>
                                    <a href={currentVersion.homepage} className="text-blue-500 hover:underline"
                                       target="_blank" rel="noopener noreferrer">
                                        {currentVersion.homepage}
                                    </a>
                                </div>
                            </div>
                        </div>
                        <div>
                            <h3 className="font-semibold mb-2">Specs</h3>
                            <div className="space-y-2">
                                <Badge variant="outline" className="mr-2">
                                    {currentVersion.specs.type}
                                </Badge>
                                {currentVersion.specs.type === "OplogProcessor" && (
                                    <>
                                        <Badge variant="outline">Component
                                            ID: {currentVersion.specs.componentId}</Badge>
                                        <Badge variant="outline">Component
                                            Version: {currentVersion.specs.componentVersion}</Badge>
                                    </>
                                )}
                                {currentVersion.specs.type === "ComponentTransformer" && (
                                    <>
                                        {currentVersion.specs.jsonSchema &&
                                            <div>
                                                <h4 className="font-semibold mt-2">JSON Schema:</h4>
                                                <pre className="bg-gray-100 p-2 rounded-md overflow-x-auto">
                      {currentVersion.specs.jsonSchema}
                    </pre>
                                            </div>
                                        }
                                    </>
                                )}
                            </div>
                        </div>
                        <div>
                            <h3 className="text-xl font-semibold mb-2">Scope</h3>
                            <Badge variant="outline" className="text-lg">
                                {currentVersion.scope.type === "Global" ? <Globe className="w-5 h-5 mr-2"/> :
                                    <Component className="w-5 h-5 mr-2"/>}
                                {currentVersion.scope.type}
                            </Badge>
                            {currentVersion.scope.type === "Component" && (
                                <div className="mt-2">
                                    <h4 className="font-semibold">Component ID:</h4>
                                    <span>{currentVersion.scope.componentID}</span>
                                </div>
                            )}
                        </div>
                    </CardContent>
                    <CardFooter className="flex justify-end space-x-4">
                        <Button variant="outline"
                                onClick={() => window.open(currentVersion.specs.validateUrl, "_blank")}>
                            Validate
                        </Button>
                        <Button variant="default"
                                onClick={() => window.open(currentVersion.specs.transformUrl, "_blank")}>
                            Transform
                        </Button>
                    </CardFooter>
                </Card>
            }
        </div>
    )
}