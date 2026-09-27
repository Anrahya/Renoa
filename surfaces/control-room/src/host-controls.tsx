import { useState } from "react";
import { pendingChange, saveChange } from "./host-mutations";
import type { ReviewTrigger } from "./host-contract";

export interface Controls { hostId: string; refresh: () => void; available: boolean; preview: boolean }

export function useChange(controls: Controls, path: string) {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  let pending = false;
  let storageError: string | null = null;
  try { pending = !controls.preview && pendingChange(controls.hostId, path) !== null; }
  catch { storageError = "Browser storage is unavailable. Restore it before editing Host settings."; }
  async function save(fields: Record<string, unknown>) {
    if (busy || controls.preview || storageError) return;
    setBusy(true); setNotice(null);
    try {
      const result = await saveChange(controls.hostId, path, fields);
      setNotice(result.message);
      if (result.kind !== "uncertain") controls.refresh();
      return result.kind;
    } catch (error) { setNotice(error instanceof Error ? error.message : "Could not preserve this change in browser storage."); }
    finally { setBusy(false); }
    return undefined;
  }
  return { busy, pending, notice: storageError ?? notice,
    disabled: busy || !controls.available || controls.preview || !!storageError, save };
}

export const triggerLabels: Record<ReviewTrigger, string> = {
  opened: "New pull request", reopened: "Reopened pull request", ready_for_review: "Marked ready for review", synchronize: "New commits pushed",
};
