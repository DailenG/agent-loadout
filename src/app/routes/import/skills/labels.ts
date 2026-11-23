// Plain reasons for the import sections: why a folder was kept or cannot be
// adopted, why a discovered folder cannot be added.

import { errorMessage } from "@/i18n/en";
import type { IpcError } from "@/ipc/bindings";

/**
 * The plain reason of an item's error: the backend's own words (scrubbed,
 * placeholder paths only) as a sentence, else the code's sentence.
 */
export function reasonText(error: IpcError): string {
  const trimmed = error.message.trim();
  if (!trimmed) {
    return errorMessage(error.code);
  }
  const first = trimmed.charAt(0).toLocaleUpperCase() + trimmed.slice(1);
  return /[.!?]$/.test(first) ? first : `${first}.`;
}
