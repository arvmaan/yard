import { Pencil } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { YardApiError } from "./api";
import type { Worker } from "./types";
import { labelWithDisplayName, secondaryDefaultLabel } from "./workerDisplay";
import {
  WORKER_DISPLAY_NAME_MAX_CHARS,
  normalizeWorkerDisplayName,
  workerDisplayName,
} from "./workerNames";

export type RenameWorkerHandler = (
  worker: Worker,
  displayName: string | null,
  expectedDisplayName: string | null,
) => Promise<Worker>;

function renameErrorMessage(caught: unknown) {
  if (caught instanceof YardApiError) {
    if (caught.code === "worker_name_conflict") {
      if (caught.currentDisplayName === undefined) return caught.message;
      const current = workerDisplayName({
        display_name: caught.currentDisplayName,
      });
      return current === null
        ? "Someone else reset this worker's name; try again."
        : `Someone else renamed this worker to "${current}"; try again.`;
    }
    return caught.message;
  }
  return caught instanceof Error ? caught.message : "Rename failed.";
}

/**
 * A worker's heading with an inline rename control. Enter or leaving the
 * field with a change saves, Escape cancels, and an empty name resets the
 * worker to its default label. The new name shows immediately and is
 * replaced by the server's answer (or reverted when the rename fails).
 */
export function WorkerNameControl({
  defaultLabel,
  onRename,
  worker,
}: {
  defaultLabel: string;
  onRename?: RenameWorkerHandler;
  worker: Worker;
}) {
  const inputId = useId();
  const errorId = useId();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  // `undefined` = nothing in flight; `null` = resetting to the default.
  const [pendingName, setPendingName] = useState<string | null | undefined>(
    undefined,
  );
  const inputRef = useRef<HTMLInputElement | null>(null);
  const renameButtonRef = useRef<HTMLButtonElement | null>(null);
  const restoreFocus = useRef(false);
  // The name the user saw when editing began: the compare-and-set token.
  const expectedName = useRef<string | null>(null);
  const settled = useRef(false);
  // The inspector reuses this control when the selection moves to another
  // worker: drop the previous worker's editor, error and pending name.
  const workerId = useRef(worker.id);
  const [shownWorkerId, setShownWorkerId] = useState(worker.id);
  if (shownWorkerId !== worker.id) {
    setShownWorkerId(worker.id);
    setEditing(false);
    setDraft("");
    setError(null);
    setPendingName(undefined);
  }
  useEffect(() => {
    workerId.current = worker.id;
    settled.current = false;
    expectedName.current = null;
  }, [worker.id]);

  const shown =
    pendingName === undefined ? worker : { display_name: pendingName };
  const label = labelWithDisplayName(shown.display_name, defaultLabel);
  // Yard's own short-lived workers (summary workers) are not renamed.
  const renameable =
    Boolean(onRename) && worker.ownership_kind !== "system_ephemeral";
  const secondary = secondaryDefaultLabel(shown.display_name, defaultLabel);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    } else if (restoreFocus.current) {
      restoreFocus.current = false;
      renameButtonRef.current?.focus();
    }
  }, [editing]);

  const startEditing = () => {
    const current = workerDisplayName(worker);
    // The raw stored value, untrimmed: the server compares it exactly, and
    // JavaScript's trim() strips characters (U+FEFF) that Rust's does not.
    expectedName.current = worker.display_name ?? null;
    settled.current = false;
    setDraft(current ?? "");
    setError(null);
    setEditing(true);
  };

  const finish = () => {
    settled.current = true;
    restoreFocus.current = true;
    setEditing(false);
  };

  const save = (value: string) => {
    if (settled.current || !onRename) return;
    const normalized = normalizeWorkerDisplayName(value);
    if (!normalized.ok) {
      setError(normalized.error);
      return;
    }
    const expected = expectedName.current;
    if (
      normalized.displayName === workerDisplayName({ display_name: expected })
    ) {
      finish();
      return;
    }
    finish();
    setError(null);
    setPendingName(normalized.displayName);
    const renamedId = worker.id;
    onRename(worker, normalized.displayName, expected)
      .catch((caught: unknown) => {
        if (workerId.current === renamedId) {
          setError(renameErrorMessage(caught));
        }
      })
      .finally(() => {
        if (workerId.current === renamedId) setPendingName(undefined);
      });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setError(null);
      finish();
    } else if (event.key === "Enter") {
      event.preventDefault();
      save(draft);
    }
  };

  return (
    <div className="worker-name">
      {editing ? (
        <div className="worker-name__editor">
          <label className="visually-hidden" htmlFor={inputId}>
            Worker name
          </label>
          <input
            aria-describedby={error ? errorId : undefined}
            aria-invalid={error ? true : undefined}
            className="worker-name__input"
            id={inputId}
            maxLength={WORKER_DISPLAY_NAME_MAX_CHARS * 2}
            onBlur={() => save(draft)}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder={defaultLabel}
            ref={inputRef}
            type="text"
            value={draft}
          />
          {expectedName.current !== null ? (
            <button
              className="secondary-button worker-name__reset"
              // Keep focus in the field so blur does not save the draft first.
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => save("")}
              type="button"
            >
              Reset to default
            </button>
          ) : null}
        </div>
      ) : (
        <div className="worker-name__display">
          <h2>{label}</h2>
          {renameable ? (
            <button
              aria-label={`Rename ${label}`}
              className="icon-button worker-name__rename"
              // aria-disabled (not disabled) so focus can return here
              // while the save is still in flight.
              aria-disabled={pendingName !== undefined ? true : undefined}
              onClick={() => {
                if (pendingName === undefined) startEditing();
              }}
              ref={renameButtonRef}
              title="Rename"
              type="button"
            >
              <Pencil aria-hidden="true" size={14} />
            </button>
          ) : null}
        </div>
      )}
      {secondary && !editing ? (
        <p className="worker-name__default">{secondary}</p>
      ) : null}
      {error ? (
        <p className="worker-name__error" id={errorId} role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
