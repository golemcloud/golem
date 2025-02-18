import { useParams } from "react-router-dom";

export const useCustomParam = () => {
    const param = useParams<Record<string, string>>();
    return Object.keys(param).reduce<Record<string, string>>((acc, key) => {
        acc[key] = decodeURIComponent(param[key] as string);
        return acc;
    }, {});
};
