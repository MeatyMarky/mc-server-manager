import { openUrl } from "@tauri-apps/plugin-opener";

import { toastError } from "./toast";

/**
 * The one way this app opens an address in the user's browser.
 *
 * Every call site used to be `void openUrl(...)`, which threw away the promise
 * — so when the opener's URL scope was missing, every link in the app failed
 * silently and looked like a dead button. This awaits it and says something
 * when it fails.
 *
 * Only http and https are opened. A URL comes from a mod source's API, and
 * handing `file:` or a custom scheme to the operating system on that authority
 * is not something to do.
 */
export async function openExternal(url: string | null | undefined): Promise<void> {
  const target = url?.trim();
  if (!target) return;

  if (!/^https?:\/\//i.test(target)) {
    toastError(new Error(`${target} is not a web address this app will open.`));
    return;
  }

  try {
    await openUrl(target);
  } catch (error) {
    toastError(error);
  }
}
