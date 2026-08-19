import { toast } from "sonner";

import { errorParts } from "./ipc";

/**
 * The one way failures reach a toast: the readable sentence as the title, the
 * fix as the description. The technical text is deliberately left out — a toast
 * disappears, so anything worth copying belongs in an `ErrorNotice`.
 */
export function toastError(error: unknown): void {
  const { message, hint } = errorParts(error);
  toast.error(message, hint ? { description: hint } : undefined);
}
