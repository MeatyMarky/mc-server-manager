import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { toast } from "sonner";

import { toastError } from "./toast";

/**
 * The one way this app copies anything.
 *
 * `navigator.clipboard` needs a secure context, and the webview's custom scheme
 * is not one everywhere — a copy button that silently does nothing on Linux is
 * the same failure the external links had, so this goes through the plugin and
 * reports a failure rather than swallowing it.
 */
export async function copyToClipboard(value: string, description?: string): Promise<void> {
  try {
    await writeText(value);
    toast.success("Copied", { description: description ?? value });
  } catch (error) {
    toastError(error);
  }
}
