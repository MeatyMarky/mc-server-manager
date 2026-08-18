import { TriangleAlert } from "lucide-react";

import { Badge } from "@/components/ui/misc";
import type { InstanceView } from "@/lib/types";
import { useJavaStatus } from "./queries";

/**
 * A mismatch between the Java this version needs and the Java available is a
 * warning, never a block: the user may fix it by installing a JDK or pinning
 * one in Settings.
 */
export function JavaWarning({ instance }: { instance: InstanceView }) {
  const status = useJavaStatus(instance.id);
  if (!status.data) return null;

  const { requiredMajor, selected, mismatch, message } = status.data;

  if (!mismatch) {
    return (
      <p className="text-xs text-muted-foreground">
        Java {requiredMajor} required
        {selected ? ` · using Java ${selected.major} at ${selected.path}` : ""}
      </p>
    );
  }

  return (
    <div className="flex items-start gap-3 rounded-lg border border-border bg-muted/40 p-4">
      <TriangleAlert className="mt-0.5 size-4 text-destructive" />
      <div>
        <p className="flex items-center gap-2 text-sm font-medium">
          Java mismatch <Badge>needs Java {requiredMajor}</Badge>
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          {message ?? `This version needs Java ${requiredMajor}.`} Install a matching JDK, or
          pin one for this instance under Settings.
        </p>
      </div>
    </div>
  );
}
