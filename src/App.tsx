import { Boxes, HelpCircle, Info, Moon, Settings, Sun } from "lucide-react";
import { useEffect } from "react";
import { Toaster } from "sonner";

import { Button } from "@/components/ui/button";
import { AboutDialog } from "@/features/app/AboutDialog";
import { FirstRun } from "@/features/app/FirstRun";
import { QuitDialog } from "@/features/app/QuitDialog";
import { ReportProblemDialog } from "@/features/app/ReportProblemDialog";
import { InstanceDetail } from "@/features/instance/InstanceDetail";
import { CreateInstanceDialog } from "@/features/instances/CreateInstanceDialog";
import { ImportInstanceDialog } from "@/features/instances/ImportInstanceDialog";
import { InstanceSidebar } from "@/features/instances/InstanceSidebar";
import { PackBrowser } from "@/features/packs/PackBrowser";
import { AppSettings } from "@/features/settings/AppSettings";
import { useInstanceEvents, useInstances } from "@/features/instances/queries";
import { ipc } from "@/lib/ipc";
import { applyTheme, useUiStore, type Theme } from "@/stores/ui";
import { useState } from "react";

export default function App() {
  const { data: instances = [], isLoading } = useInstances();
  const { selectedInstanceId, selectInstance, theme, setTheme } = useUiStore();
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);
  const [about, setAbout] = useState(false);
  // What fills the main pane. Packs and app settings are not about one server,
  // so they replace the instance view rather than hiding inside it.
  const [view, setView] = useState<"instance" | "packs" | "settings">("instance");
  const [reporting, setReporting] = useState(false);

  useInstanceEvents();

  // Theme is persisted in the settings table, so it survives restarts.
  useEffect(() => {
    ipc
      .settingsGetAll()
      .then((settings) => {
        const stored = settings.theme === "light" ? "light" : "dark";
        applyTheme(stored as Theme);
        useUiStore.setState({ theme: stored as Theme });
      })
      .catch(() => applyTheme("dark"));
  }, []);

  const selected =
    instances.find((instance) => instance.id === selectedInstanceId) ?? instances[0];

  useEffect(() => {
    if (!selectedInstanceId && selected) selectInstance(selected.id);
  }, [selected, selectedInstanceId, selectInstance]);

  function toggleTheme() {
    const next: Theme = theme === "dark" ? "light" : "dark";
    setTheme(next);
    void ipc.settingsSet("theme", next);
  }

  return (
    <div className="flex h-full">
      <InstanceSidebar
        instances={instances}
        isLoading={isLoading}
        selectedId={selected?.id ?? null}
        onSelect={selectInstance}
        onCreate={() => setCreating(true)}
        onImport={() => setImporting(true)}
      />

      <main className="flex min-h-0 min-w-0 flex-1 flex-col">
        <div className="flex items-center justify-end gap-1 px-6 pt-4">
          <Button
            variant={view === "packs" ? "default" : "ghost"}
            size="sm"
            className="mr-auto"
            onClick={() => setView((current) => (current === "packs" ? "instance" : "packs"))}
          >
            <Boxes /> Browse packs
          </Button>
          <Button
            variant={view === "settings" ? "default" : "ghost"}
            size="icon"
            onClick={() =>
              setView((current) => (current === "settings" ? "instance" : "settings"))
            }
            aria-label="Settings"
            aria-pressed={view === "settings"}
            title="Settings"
          >
            <Settings />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setReporting(true)}
            aria-label="Report a problem"
            title="Report a problem"
          >
            <HelpCircle />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setAbout(true)}
            aria-label="About this app"
            title="About"
          >
            <Info />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={toggleTheme}
            aria-label={theme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
          >
            {theme === "dark" ? <Sun /> : <Moon />}
          </Button>
        </div>

        {view === "settings" ? (
          <AppSettings
            onAbout={() => setAbout(true)}
            onReportProblem={() => setReporting(true)}
          />
        ) : view === "packs" ? (
          <PackBrowser
            onInstalled={(instanceId) => {
              setView("instance");
              selectInstance(instanceId);
            }}
          />
        ) : selected ? (
          <InstanceDetail key={selected.id} instance={selected} />
        ) : isLoading ? (
          <div className="flex flex-1 items-center justify-center p-10 text-sm text-muted-foreground">
            Loading your servers…
          </div>
        ) : (
          // Nothing yet is a starting point, not an empty list.
          <FirstRun onCreate={() => setCreating(true)} onImport={() => setImporting(true)} />
        )}
      </main>

      <CreateInstanceDialog open={creating} onOpenChange={setCreating} />
      <ImportInstanceDialog open={importing} onOpenChange={setImporting} />
      <AboutDialog
        open={about}
        onOpenChange={setAbout}
        onReportProblem={() => {
          setAbout(false);
          setReporting(true);
        }}
      />
      <ReportProblemDialog
        open={reporting}
        onOpenChange={setReporting}
        instance={selected ?? null}
      />
      <QuitDialog />
      <Toaster position="bottom-right" theme={theme} richColors closeButton />
    </div>
  );
}
