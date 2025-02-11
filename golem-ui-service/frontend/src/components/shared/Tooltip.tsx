import { HelpCircle, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";

export const TOOLTIP_CONTENT = {
  path: {
    title: "Path Parameters",
    content: `<pre class="bg-gray-900 p-2 rounded">{&lt;VARIABLE_NAME&gt;}</pre>`,
  },
  worker: {
    title: "Common Interpolation Expressions",
    content: `
      <div class="space-y-3">
        <div>
          <div class="font-medium mb-1">Path Parameters:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.path.&lt;PATH_PARAM_NAME&gt;}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Query Parameters:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.path.&lt;QUERY_PARAM_NAME&gt;}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Request Body:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.body}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Request Body Field:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.body.&lt;FIELD_NAME&gt;}</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Request Headers:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">\${request.header.&lt;HEADER_NAME&gt;}</pre>
        </div>
      </div>
    `,
  },
  response: {
    title: "Response Transform",
    content: `
      <div class="space-y-3">
        <div>
          <div class="font-medium mb-1">Return Payload:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">let response = request.body;</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Transform Response:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">let transformed = response.data;</pre>
        </div>
        <div>
          <div class="font-medium mb-1">Access Request Data:</div>
          <pre class="bg-gray-900 p-2 rounded mb-1">let id = request.path.id;</pre>
        </div>
      </div>
    `,
  },
};

interface TooltipProps {
  content: string;
  title: string;
}

export const Tooltip = ({ content, title }: TooltipProps) => {
  const [isOpen, setIsOpen] = useState(false);
  const tooltipRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        tooltipRef.current &&
        !tooltipRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  return (
    <div className="relative inline-block">
      <button onClick={() => setIsOpen(!isOpen)}>
        <HelpCircle
          className={`w-4 h-4 cursor-pointer transition-colors ${
            isOpen
              ? "text-primary"
              : "text-muted-foreground hover:text-gray-300"
          }`}
        />
      </button>
      {isOpen && (
        <div
          ref={tooltipRef}
          className="absolute left-full ml-2 w-96 p-4 bg-card rounded-lg shadow-xl 
                     text-sm z-50 border border-border"
        >
          <div className="flex justify-between items-start mb-3">
            <h3 className="font-medium text-base text-foreground">{title}</h3>
            <button
              onClick={() => setIsOpen(false)}
              className="text-muted-foreground hover:text-foreground p-1 
                       rounded-md hover:bg-primary/10"
            >
              <X size={14} />
            </button>
          </div>
          <div
            className="text-muted-foreground space-y-2"
            dangerouslySetInnerHTML={{ __html: content }}
          />
        </div>
      )}
    </div>
  );
};
