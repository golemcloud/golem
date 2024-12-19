// "use client";
// import { useCallback, useEffect, useState } from "react";
// import { fetcher } from "../../lib/utils";

// // Custom hook to fetch deployments
// export default function useCustomSwr(path: string) {
//   const [data, setData] = useState(null);
//   const [isLoading, setLoading] = useState(true);

//   const fetchData = useCallback(async () => {
//     setLoading(true);
//     try {
//       const result = await fetcher(path);
//       setData(result);
//     } catch (error) {
//       console.error("Error fetching data:", error);
//     } finally {
//       setLoading(false);
//     }
//   }, [path]);

//   useEffect(() => {
//     fetchData();
//   }, [fetchData, path]);

//   return {
//     data,
//     isLoading,
//   };
// };
