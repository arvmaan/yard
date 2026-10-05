// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { CompletedRuntimeCleanupPreviewDialog } from "./CompletedRuntimeCleanupPreviewDialog";
import type {
  CompletedRuntimeCleanupCandidate,
  CompletedRuntimeCleanupPreview,
} from "./types";

afterEach(cleanup);

function candidate(
  display_name: string | null,
): CompletedRuntimeCleanupCandidate {
  return {
    assignment_id: `assignment-${display_name ?? "unnamed"}`,
    close_eligible: false,
    completed_at_unix_ms: 1,
    completion_receipt_id: "receipt-1",
    display_name,
    linked_artifact_count: 0,
    profile_name: "Generalist",
    project_id: "project-1",
    project_name: "Project",
    retained_reasons: ["no_linked_artifacts"],
    role: "implementer",
    worker_id: "worker-1",
  };
}

function preview(
  candidates: CompletedRuntimeCleanupCandidate[],
): CompletedRuntimeCleanupPreview {
  return {
    candidate_count: candidates.length,
    candidates,
    close_ready_count: 0,
    limit: 50,
    truncated: false,
  };
}

describe("CompletedRuntimeCleanupPreviewDialog", () => {
  it("labels a named worker by its name, with the profile as context", () => {
    render(
      <CompletedRuntimeCleanupPreviewDialog
        error={null}
        loading={false}
        onClose={() => {}}
        preview={preview([candidate("BAR CDK"), candidate(null)])}
        returnFocus={null}
      />,
    );
    const labels = screen
      .getAllByRole("listitem")
      .map((item) => item.querySelector("strong")?.textContent);
    expect(labels).toEqual(["BAR CDK · Generalist", "Generalist"]);
  });
});
