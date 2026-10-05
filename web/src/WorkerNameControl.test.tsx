// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
} from "@testing-library/react";
import { YardApiError } from "./api";
import type { Worker } from "./types";
import { WorkerNameControl } from "./WorkerNameControl";

afterEach(cleanup);

function worker(display_name: string | null = null): Worker {
  return {
    created_at_unix_ms: 1,
    desired_state: "running",
    display_name,
    id: "worker-1",
    profile_id: "profile-1",
    profile_version: "1",
    runtime: null,
    updated_at_unix_ms: 1,
    version: "7",
  } as Worker;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, reject, resolve };
}

function openEditor(label = "Generalist") {
  fireEvent.click(screen.getByRole("button", { name: `Rename ${label}` }));
  return screen.getByRole("textbox", {
    name: "Worker name",
  }) as HTMLInputElement;
}

describe("WorkerNameControl", () => {
  it("shows no rename control for Yard's own summary workers", () => {
    render(
      <WorkerNameControl
        defaultLabel="Summary"
        onRename={vi.fn()}
        worker={{ ...worker(), ownership_kind: "system_ephemeral" }}
      />,
    );
    expect(screen.getByRole("heading", { name: "Summary" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Rename Summary" })).toBeNull();
  });

  it("offers renaming for Yard-owned and external workers", () => {
    for (const ownership_kind of ["yard_owned", "external"] as const) {
      render(
        <WorkerNameControl
          defaultLabel="Generalist"
          onRename={vi.fn()}
          worker={{ ...worker(), ownership_kind }}
        />,
      );
      expect(
        screen.getByRole("button", { name: "Rename Generalist" }),
      ).toBeTruthy();
      cleanup();
    }
  });

  it("saves on Enter with the name the user saw as the expectation", async () => {
    const pending = deferred<Worker>();
    const onRename = vi.fn(() => pending.promise);
    const { rerender } = render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={worker()}
      />,
    );
    const input = openEditor();
    expect(document.activeElement).toBe(input);
    fireEvent.change(input, { target: { value: "  BAR CDK " } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onRename).toHaveBeenCalledWith(worker(), "BAR CDK", null);
    // Optimistic: the new name shows before the server answers.
    expect(screen.getByRole("heading", { name: "BAR CDK" })).toBeTruthy();
    expect(screen.getByText("Generalist")).toBeTruthy();

    const renamed = worker("BAR CDK");
    await act(async () => {
      pending.resolve(renamed);
      await pending.promise;
    });
    rerender(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={renamed}
      />,
    );
    expect(screen.getByRole("heading", { name: "BAR CDK" })).toBeTruthy();
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Rename BAR CDK" }),
    );
  });

  it("sends the raw stored name, untrimmed, as the expectation", () => {
    // An API client may have stored a name the browser's trim() changes.
    const stored = worker("BAR\uFEFF");
    const onRename = vi.fn(() => new Promise<Worker>(() => undefined));
    render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={stored}
      />,
    );
    const input = openEditor("BAR");
    expect(input.value).toBe("BAR");
    fireEvent.blur(input);
    expect(onRename).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Rename BAR" }));
    fireEvent.click(screen.getByRole("button", { name: "Reset to default" }));
    expect(onRename).toHaveBeenCalledWith(stored, null, "BAR\uFEFF");
  });

  it("cancels on Escape without saving", () => {
    const onRename = vi.fn();
    render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={worker("Docs")}
      />,
    );
    const input = openEditor("Docs");
    expect(input.value).toBe("Docs");
    fireEvent.change(input, { target: { value: "Other" } });
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Docs" })).toBeTruthy();
  });

  it("does not save a blur without a change", () => {
    const onRename = vi.fn();
    render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={worker("Docs")}
      />,
    );
    fireEvent.blur(openEditor("Docs"));
    expect(onRename).not.toHaveBeenCalled();
  });

  it("clears the name with Reset to default", () => {
    const onRename = vi.fn(() => new Promise<Worker>(() => undefined));
    render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={worker("Docs")}
      />,
    );
    openEditor("Docs");
    fireEvent.click(screen.getByRole("button", { name: "Reset to default" }));
    expect(onRename).toHaveBeenCalledWith(worker("Docs"), null, "Docs");
    expect(screen.getByRole("heading", { name: "Generalist" })).toBeTruthy();
  });

  it("refuses an invalid name inline without calling the server", () => {
    const onRename = vi.fn();
    render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={worker()}
      />,
    );
    const input = openEditor();
    fireEvent.change(input, { target: { value: "x".repeat(65) } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onRename).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toContain("64");
    expect(input.getAttribute("aria-invalid")).toBe("true");
  });

  it("reverts and shows the error when the rename fails", async () => {
    const pending = deferred<Worker>();
    render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={() => pending.promise}
        worker={worker()}
      />,
    );
    const input = openEditor();
    fireEvent.change(input, { target: { value: "BAR CDK" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await act(async () => {
      pending.reject(
        new YardApiError(
          "worker_name_conflict",
          "The worker was renamed by someone else; review the current name and try again",
          null,
          null,
          null,
          null,
          "Docs",
        ),
      );
      await pending.promise.catch(() => undefined);
    });
    expect(screen.getByRole("heading", { name: "Generalist" })).toBeTruthy();
    expect(screen.getByRole("alert").textContent).toBe(
      'Someone else renamed this worker to "Docs"; try again.',
    );
  });
  it("drops the editor and error when another worker is shown", async () => {
    const onRename = vi.fn(() => new Promise<Worker>(() => undefined));
    const other = { ...worker(), id: "worker-2" } as Worker;
    const { rerender } = render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={worker()}
      />,
    );
    const input = openEditor();
    fireEvent.change(input, { target: { value: "x".repeat(65) } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByRole("alert")).toBeTruthy();

    rerender(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={other}
      />,
    );
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    const next = openEditor();
    expect(next.value).toBe("");
    fireEvent.change(next, { target: { value: "Docs" } });
    fireEvent.keyDown(next, { key: "Enter" });
    expect(onRename).toHaveBeenCalledWith(other, "Docs", null);
  });

  it("does not show a late failure under another worker", async () => {
    const pending = deferred<Worker>();
    const onRename = vi.fn(() => pending.promise);
    const other = { ...worker(), id: "worker-2" } as Worker;
    const { rerender } = render(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={worker()}
      />,
    );
    const input = openEditor();
    fireEvent.change(input, { target: { value: "BAR CDK" } });
    fireEvent.keyDown(input, { key: "Enter" });
    rerender(
      <WorkerNameControl
        defaultLabel="Generalist"
        onRename={onRename}
        worker={other}
      />,
    );
    expect(screen.getByRole("heading", { name: "Generalist" })).toBeTruthy();
    await act(async () => {
      pending.reject(new YardApiError("worker_name_conflict", "renamed"));
      await pending.promise.catch(() => undefined);
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
