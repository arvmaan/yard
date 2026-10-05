import { describe, expect, it } from "vitest";
import {
  WORKER_DISPLAY_NAME_MAX_CHARS,
  normalizeWorkerDisplayName,
  workerDisplayName,
  withRenamedDisplayName,
} from "./workerNames";
import type { Worker } from "./types";

function worker(display_name?: string | null): Worker {
  return {
    created_at_unix_ms: 1,
    desired_state: "running",
    display_name,
    id: "worker-1234567890",
    profile_id: "profile-1",
    profile_version: "1",
    runtime: null,
    updated_at_unix_ms: 1,
    version: "1",
  } as Worker;
}

describe("workerDisplayName", () => {
  it("reads a name only from display_name", () => {
    expect(workerDisplayName(worker(" Docs "))).toBe("Docs");
    expect(workerDisplayName(worker("   "))).toBeNull();
    expect(workerDisplayName(undefined)).toBeNull();
  });
});

describe("normalizeWorkerDisplayName", () => {
  it("trims and treats empty as a reset", () => {
    expect(normalizeWorkerDisplayName("  BAR CDK  ")).toEqual({
      displayName: "BAR CDK",
      ok: true,
    });
    expect(normalizeWorkerDisplayName("   ")).toEqual({
      displayName: null,
      ok: true,
    });
  });

  it("counts characters, not UTF-16 units", () => {
    const emoji = "\u{1F680}";
    const atLimit = emoji.repeat(WORKER_DISPLAY_NAME_MAX_CHARS);
    expect(atLimit.length).toBe(WORKER_DISPLAY_NAME_MAX_CHARS * 2);
    expect(normalizeWorkerDisplayName(atLimit).ok).toBe(true);
    expect(normalizeWorkerDisplayName(`${atLimit}x`).ok).toBe(false);
    expect(normalizeWorkerDisplayName("é".repeat(64)).ok).toBe(true);
  });

  it("rejects control and text-direction override characters", () => {
    expect(normalizeWorkerDisplayName("a\nb").ok).toBe(false);
    expect(normalizeWorkerDisplayName("a\tb").ok).toBe(false);
    expect(normalizeWorkerDisplayName("a\u202Eb").ok).toBe(false);
    expect(normalizeWorkerDisplayName("a\u2066b").ok).toBe(false);
  });

  it("rejects invisible format characters and line separators", () => {
    for (const value of [
      "\u200B",
      "\u200F",
      "BAR\u200BCDK",
      "BAR\u200ECDK",
      "BAR\u2060CDK",
      "BAR\u061CCDK",
      "BAR\u180ECDK",
      "BAR\uFEFFCDK",
      "BAR\u2028CDK",
      "BAR\u2029CDK",
    ]) {
      expect(normalizeWorkerDisplayName(value).ok, JSON.stringify(value)).toBe(
        false,
      );
    }
  });
});

describe("withRenamedDisplayName", () => {
  it("takes only the name from a rename response", () => {
    const current = {
      ...worker(null),
      runtime: { observed_status: "working" },
      version: "3",
    } as unknown as Worker;
    const stale = { ...worker("BAR CDK"), version: "2" };
    const patched = withRenamedDisplayName(current, stale);
    expect(patched.display_name).toBe("BAR CDK");
    expect(patched.version).toBe("3");
    expect(patched.runtime).toBe(current.runtime);
  });

  it("leaves other workers unchanged", () => {
    const other = { ...worker(null), id: "worker-other" };
    expect(withRenamedDisplayName(other, worker("BAR CDK"))).toBe(other);
  });
});
