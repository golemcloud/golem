import { useParams } from "next/navigation";

export const useCustomParam = () => {
    const param = useParams<Record<string,string>>();
    return Object.keys(param).reduce<Record<string,string>>((acc,key:string) => {
        acc[key] = decodeURIComponent(param[key]) as string;
        return acc;
    },{})
}