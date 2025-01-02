import ErrorBoundary from "@/components/errorBoundary";
import WorkerLeftNav from "./leftNav";

export default function WorkerDetails() {
  return (
    <ErrorBoundary>
      <div className="flex">
        <WorkerLeftNav />
        <div>Right Nav</div>
      </div>
    </ErrorBoundary>
  );
}
