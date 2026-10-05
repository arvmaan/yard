// A user-chosen worker name. Yard stores it on the durable worker
// (`display_name`); it is cosmetic and never renames the Herdr tab or pane.
// These rules mirror the server's validation so the rename control can
// refuse an invalid name before sending it; the server stays authoritative.
export const WORKER_DISPLAY_NAME_MAX_CHARS = 64;

// Bidi embedding/override (U+202A-U+202E) and isolate (U+2066-U+2069)
// characters can make one worker's label impersonate another's.
const BIDI_CONTROL = /[\u202A-\u202E\u2066-\u2069]/u;
const CONTROL_CHARACTER = /\p{Cc}/u;
// Other invisible format characters (zero-width space, word joiner, BOM,
// LRM/RLM, ...) and the line/paragraph separators, which browsers render as
// a forced line break. The server rejects the same set.
const INVISIBLE_CHARACTER = /[\p{Cf}\p{Zl}\p{Zp}]/u;

export interface NamedWorker {
  display_name?: string | null;
}

export type WorkerDisplayNameInput =
  { ok: true; displayName: string | null } | { ok: false; error: string };

/** Trims a typed name; empty means "reset to the default label". */
export function normalizeWorkerDisplayName(
  value: string,
): WorkerDisplayNameInput {
  const trimmed = value.trim();
  if (!trimmed) return { ok: true, displayName: null };
  if (CONTROL_CHARACTER.test(trimmed)) {
    return {
      ok: false,
      error: "Names cannot contain line breaks, tabs, or control characters.",
    };
  }
  if (BIDI_CONTROL.test(trimmed)) {
    return {
      ok: false,
      error: "Names cannot contain text-direction override characters.",
    };
  }
  if (INVISIBLE_CHARACTER.test(trimmed)) {
    return {
      ok: false,
      error: "Names cannot contain invisible formatting characters.",
    };
  }
  // Count characters (code points), not UTF-16 units or bytes.
  if (Array.from(trimmed).length > WORKER_DISPLAY_NAME_MAX_CHARS) {
    return {
      ok: false,
      error: `Names can be at most ${WORKER_DISPLAY_NAME_MAX_CHARS} characters.`,
    };
  }
  return { ok: true, displayName: trimmed };
}

/** The user-chosen name, or null when the worker uses its default label. */
export function workerDisplayName(
  worker: NamedWorker | null | undefined,
): string | null {
  const name = worker?.display_name?.trim();
  return name ? name : null;
}

/**
 * Applies a rename response to a cached worker. Only the name is taken from
 * the response: a rename is cosmetic, and its (possibly older) runtime,
 * desired state and version must not overwrite a newer poll's projection.
 */
export function withRenamedDisplayName<T extends NamedWorker & { id: string }>(
  worker: T,
  renamed: NamedWorker & { id: string },
): T {
  if (worker.id !== renamed.id) return worker;
  return { ...worker, display_name: renamed.display_name ?? null };
}
