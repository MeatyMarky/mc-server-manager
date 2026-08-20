import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/dialog";
import { Input, Textarea } from "@/components/ui/input";
import { Switch } from "@/components/ui/misc";
import { JavaSettings } from "@/features/setup/JavaSettings";
import { JavaPlanNotice } from "@/features/setup/ManagedRuntimes";
import { useUpdateInstance } from "@/features/instances/queries";
import type { InstanceView } from "@/lib/types";

/**
 * Per-instance settings that already exist in phase 1. Java pinning, JVM
 * presets and the EULA control arrive with phase 2 and 3.
 */
export function SettingsTab({ instance }: { instance: InstanceView }) {
  const update = useUpdateInstance();
  const [form, setForm] = useState(() => toForm(instance));

  useEffect(() => setForm(toForm(instance)), [instance]);

  const disabled = instance.status !== "stopped" && instance.status !== "crashed";

  return (
    <form
      className="grid max-w-2xl gap-6"
      onSubmit={(event) => {
        event.preventDefault();
        update.mutate({
          id: instance.id,
          input: {
            name: null,
            mcVersion: form.mcVersion.trim(),
            loaderVersion: null,
            javaPath: null,
            jvmArgs: form.jvmArgs
              .split("\n")
              .map((line) => line.trim())
              .filter(Boolean),
            serverArgs: null,
            minRamMb: form.minRamMb,
            maxRamMb: form.maxRamMb,
            autoStart: form.autoStart,
            autoRestart: form.autoRestart,
            restartMax: null,
            restartWindowS: null,
            stopTimeoutS: form.stopTimeoutS,
            notes: form.notes.trim() ? form.notes : null,
            color: null,
          },
        });
      }}
    >
      <section className="grid gap-4">
        <h3 className="text-sm font-semibold">Server</h3>
        <div className="grid grid-cols-2 gap-4">
          <div className="grid gap-2">
            <Label htmlFor="settings-version">Minecraft version</Label>
            <Input
              id="settings-version"
              value={form.mcVersion}
              onChange={(event) => setForm({ ...form, mcVersion: event.target.value })}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="settings-stop-timeout">Stop timeout (seconds)</Label>
            <Input
              id="settings-stop-timeout"
              type="number"
              min={5}
              value={form.stopTimeoutS}
              onChange={(event) =>
                setForm({ ...form, stopTimeoutS: Number(event.target.value) })
              }
            />
          </div>
        </div>
      </section>

      <JavaSettings instance={instance} />
      <JavaPlanNotice
        mcVersion={instance.mcVersion}
        serverType={instance.serverType}
        recordedMajor={instance.javaMajor}
        pinned={instance.javaPath}
      />

      <section className="grid gap-4">
        <h3 className="text-sm font-semibold">Memory and JVM</h3>
        <div className="grid grid-cols-2 gap-4">
          <div className="grid gap-2">
            <Label htmlFor="settings-min-ram">Minimum RAM (MB)</Label>
            <Input
              id="settings-min-ram"
              type="number"
              min={512}
              step={512}
              value={form.minRamMb}
              onChange={(event) => setForm({ ...form, minRamMb: Number(event.target.value) })}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="settings-max-ram">Maximum RAM (MB)</Label>
            <Input
              id="settings-max-ram"
              type="number"
              min={512}
              step={512}
              value={form.maxRamMb}
              onChange={(event) => setForm({ ...form, maxRamMb: Number(event.target.value) })}
            />
          </div>
        </div>
        <div className="grid gap-2">
          <Label htmlFor="settings-jvm">JVM arguments (one per line)</Label>
          <Textarea
            id="settings-jvm"
            className="font-mono text-xs"
            rows={6}
            value={form.jvmArgs}
            onChange={(event) => setForm({ ...form, jvmArgs: event.target.value })}
          />
        </div>
      </section>

      <section className="grid gap-3">
        <h3 className="text-sm font-semibold">Behaviour</h3>
        <ToggleRow
          id="settings-auto-start"
          label="Start with the app"
          hint="Launch this server when the manager starts."
          checked={form.autoStart}
          onCheckedChange={(autoStart) => setForm({ ...form, autoStart })}
        />
        <ToggleRow
          id="settings-auto-restart"
          label="Restart after a crash"
          hint="Backoff-limited so a broken server cannot loop forever. Takes effect in phase 3."
          checked={form.autoRestart}
          onCheckedChange={(autoRestart) => setForm({ ...form, autoRestart })}
        />
      </section>

      <section className="grid gap-2">
        <Label htmlFor="settings-notes">Notes</Label>
        <Textarea
          id="settings-notes"
          rows={3}
          value={form.notes}
          onChange={(event) => setForm({ ...form, notes: event.target.value })}
        />
      </section>

      <div className="flex items-center gap-3">
        <Button type="submit" disabled={update.isPending || disabled}>
          {update.isPending ? "Saving…" : "Save changes"}
        </Button>
        {disabled ? (
          <p className="text-xs text-muted-foreground">Stop the server to edit its settings.</p>
        ) : null}
      </div>
    </form>
  );
}

function ToggleRow({
  id,
  label,
  hint,
  checked,
  onCheckedChange,
}: {
  id: string;
  label: string;
  hint: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <div className="flex items-center justify-between rounded-md border border-border p-3">
      <div>
        <Label htmlFor={id}>{label}</Label>
        <p className="text-xs text-muted-foreground">{hint}</p>
      </div>
      <Switch id={id} checked={checked} onCheckedChange={onCheckedChange} />
    </div>
  );
}

function toForm(instance: InstanceView) {
  return {
    mcVersion: instance.mcVersion,
    minRamMb: instance.minRamMb,
    maxRamMb: instance.maxRamMb,
    stopTimeoutS: instance.stopTimeoutS,
    jvmArgs: instance.jvmArgs.join("\n"),
    autoStart: instance.autoStart,
    autoRestart: instance.autoRestart,
    notes: instance.notes ?? "",
  };
}
