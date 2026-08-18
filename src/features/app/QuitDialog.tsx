import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { onQuitRequested } from "@/lib/events";
import { ipc } from "@/lib/ipc";

/**
 * Closing the window only hides it to the tray. Quitting from the tray while
 * servers are alive routes through here first: the backend emits
 * `app://quit-requested` with the live instances instead of exiting.
 */
export function QuitDialog() {
  const [liveInstances, setLiveInstances] = useState<string[] | null>(null);

  useEffect(() => {
    const pending = onQuitRequested((payload) => setLiveInstances(payload.liveInstances));
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  const open = liveInstances !== null;

  return (
    <Dialog open={open} onOpenChange={(next) => (next ? null : setLiveInstances(null))}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Quit with servers running?</DialogTitle>
          <DialogDescription>
            These servers are still running and will keep running unless you stop them first:
          </DialogDescription>
        </DialogHeader>
        <ul className="list-inside list-disc text-sm">
          {(liveInstances ?? []).map((name) => (
            <li key={name}>{name}</li>
          ))}
        </ul>
        <DialogFooter>
          <Button variant="ghost" onClick={() => setLiveInstances(null)}>
            Stay open
          </Button>
          <Button variant="destructive" onClick={() => void ipc.appQuit()}>
            Quit anyway
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
