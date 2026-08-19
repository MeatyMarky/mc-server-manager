import { useQuery } from "@tanstack/react-query";
import { CheckCircle2, Download, FolderSearch, Loader2, Plus, ShieldCheck } from "lucide-react";

import { Button } from "@/components/ui/button";
import { ipc } from "@/lib/ipc";
import { openExternal } from "@/lib/external";

const JDK_URL = "https://adoptium.net/temurin/releases/";

/**
 * What the app shows before there is anything to show: the two ways in, and an
 * honest answer about whether this machine can run a server at all.
 *
 * Java detection runs in the background at launch, so this screen has three
 * states rather than two — still looking, found nothing, found something —
 * and says which one it is instead of guessing.
 */
export function FirstRun({
  onCreate,
  onImport,
}: {
  onCreate: () => void;
  onImport: () => void;
}) {
  const readiness = useQuery({
    queryKey: ["readiness"],
    queryFn: () => ipc.startupReadiness(),
    // Detection is a background task at launch; this settles within seconds.
    refetchInterval: (query) => (query.state.data?.javaScanPending ? 2000 : false),
  });

  const ready = readiness.data;

  return (
    <div className="flex min-h-0 flex-1 items-center justify-center overflow-y-auto overscroll-contain p-8">
      <div className="w-full max-w-xl space-y-6">
        <header className="text-center">
          <h2 className="text-xl font-semibold">Set up your first server</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            Everything stays on this computer: the app downloads server files, starts them, and
            keeps backups in folders you choose.
          </p>
        </header>

        <section
          aria-live="polite"
          className="rounded-lg border border-border p-4 text-sm"
        >
          <h3 className="flex items-center gap-2 font-medium">
            <ShieldCheck className="size-4" aria-hidden />
            Java
          </h3>

          {!ready || ready.javaScanPending ? (
            <p className="mt-2 flex items-center gap-2 text-muted-foreground">
              <Loader2 className="size-4 animate-spin" aria-hidden />
              Looking for Java on this computer…
            </p>
          ) : ready.warning ? (
            <>
              <p className="mt-2 text-muted-foreground">{ready.warning}</p>
              <div className="mt-3 flex flex-wrap gap-2">
                <Button size="sm" variant="outline" onClick={() => void openExternal(JDK_URL)}>
                  <Download /> Get a JDK
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => void ipc.javaRescan().then(() => readiness.refetch())}
                >
                  Rescan
                </Button>
              </div>
            </>
          ) : (
            <p className="mt-2 flex items-center gap-2 text-muted-foreground">
              <CheckCircle2 className="size-4 text-emerald-500" aria-hidden />
              Java {ready.newestJava} found — that runs current Minecraft versions.
            </p>
          )}
        </section>

        <div className="grid gap-3 sm:grid-cols-2">
          <button
            type="button"
            onClick={onCreate}
            className="rounded-lg border border-border p-4 text-left transition-colors hover:border-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
            <span className="flex items-center gap-2 font-medium">
              <Plus className="size-4" aria-hidden />
              Create a server
            </span>
            <span className="mt-1 block text-sm text-muted-foreground">
              Pick a version and a type — Vanilla, Paper, Purpur, Fabric, Forge or NeoForge —
              and the app downloads it.
            </span>
          </button>

          <button
            type="button"
            onClick={onImport}
            className="rounded-lg border border-border p-4 text-left transition-colors hover:border-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
          >
            <span className="flex items-center gap-2 font-medium">
              <FolderSearch className="size-4" aria-hidden />
              Import an existing one
            </span>
            <span className="mt-1 block text-sm text-muted-foreground">
              Point at a folder you already run a server from. Nothing in it is moved or
              rewritten.
            </span>
          </button>
        </div>

        <p className="text-center text-xs text-muted-foreground">
          A server only starts once you accept the Minecraft EULA for it. The app never accepts
          it for you.
        </p>
      </div>
    </div>
  );
}
