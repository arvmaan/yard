pub(crate) fn with_orchestrator_status_contract(prompt: &str, command_id: &str) -> String {
    let command_id = serde_json::to_string(command_id)
        .expect("serializing a validated command identifier cannot fail");
    format!(
        "{prompt}\n\n\
         Yard orchestrator status protocol for command {command_id}:\n\
         Respond to this command with exactly one JSON object on one line and nothing else. \
         The object must contain exactly these fields: version as the number 1; command_id as \
         the exact command string above; state as one of working, needs_attention, or idle; \
         last as a string; next as a string; and blockers as an array of strings. Do not emit \
         Markdown or prose outside the object. There is no completed state. A status report is \
         observational and never completes an assignment; only a manual Yard completion receipt \
         can do that."
    )
}

#[cfg(test)]
mod tests {
    use yard_domain::OrchestratorStatusReport;

    use super::with_orchestrator_status_contract;

    #[test]
    fn injects_actual_command_without_embedding_a_parseable_report() {
        let prompt = with_orchestrator_status_contract("Coordinate the project.", "command-42");

        assert!(prompt.starts_with("Coordinate the project."));
        assert!(prompt.contains("command \"command-42\""));
        assert!(prompt.contains("exactly one JSON object on one line"));
        assert!(prompt.contains("There is no completed state"));
        assert!(prompt.contains("only a manual Yard completion receipt"));
        assert!(OrchestratorStatusReport::scan_terminal_output(&prompt).is_none());
    }
}
