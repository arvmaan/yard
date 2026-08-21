use yard_domain::{
    OrchestratorWorkflowProfile, OrchestratorWorkflowProfileValidationError,
    validate_orchestrator_workflow_commands,
};

pub(crate) fn validate_executable_orchestrator_workflow(
    workflow: &OrchestratorWorkflowProfile,
) -> Result<(), OrchestratorWorkflowProfileValidationError> {
    validate_orchestrator_workflow_commands(&workflow.commands)
}

pub(crate) fn with_orchestrator_workflow(
    prompt: &str,
    workflow: &OrchestratorWorkflowProfile,
) -> Result<String, OrchestratorWorkflowProfileValidationError> {
    validate_executable_orchestrator_workflow(workflow)?;
    let commands = serde_json::to_string(&workflow.commands)
        .expect("serializing validated workflow commands cannot fail");
    Ok(format!(
        "Yard orchestrator workflow profile {} revision {} \
         (monitor interval: {} ms).\n\
         Structured workflow commands (ordered JSON):\n{}\n\n{}\n\n\
         ## Current Yard command\n\n{}",
        workflow.id,
        workflow.version,
        workflow.monitor_interval_ms,
        commands,
        workflow.instructions_markdown,
        prompt
    ))
}

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

    use yard_domain::{
        OrchestratorWorkflowProfile, OrchestratorWorkflowProfileSource,
        YARD_STANDARD_ORCHESTRATOR_PROFILE_ID, yard_standard_orchestrator_commands,
    };

    use super::{
        validate_executable_orchestrator_workflow, with_orchestrator_status_contract,
        with_orchestrator_workflow,
    };

    #[test]
    fn composes_the_pinned_workflow_before_the_current_command_and_status_contract() {
        let workflow = OrchestratorWorkflowProfile {
            id: YARD_STANDARD_ORCHESTRATOR_PROFILE_ID.to_owned(),
            name: "Yard Standard Orchestrator".to_owned(),
            description: "Standard workflow".to_owned(),
            version: 7,
            instructions_markdown: "# Fleet workflow\n\nUse workers.".to_owned(),
            monitor_interval_ms: 600_000,
            commands: yard_standard_orchestrator_commands(),
            adapter_context_files: Vec::new(),
            source: OrchestratorWorkflowProfileSource::User,
            updated_by: "local-user".to_owned(),
            created_at_unix_ms: 1,
        };

        let prompt = with_orchestrator_workflow("Ship the change.", &workflow).unwrap();
        let prompt = with_orchestrator_status_contract(&prompt, "command-7");

        assert!(prompt.starts_with(
            "Yard orchestrator workflow profile yard:standard-orchestrator revision 7"
        ));
        assert!(prompt.contains("monitor interval: 600000 ms"));
        assert!(prompt.contains("\"id\":\"result.integrate\""));
        assert!(prompt.find("Use workers.").unwrap() < prompt.find("Ship the change.").unwrap());
        assert!(
            prompt.find("Ship the change.").unwrap()
                < prompt.find("status protocol for command").unwrap()
        );
    }

    #[test]
    fn compiled_runtime_request_contains_the_exact_canonical_command_matrix() {
        let workflow = OrchestratorWorkflowProfile {
            id: YARD_STANDARD_ORCHESTRATOR_PROFILE_ID.to_owned(),
            name: "Yard Standard Orchestrator".to_owned(),
            description: "Standard workflow".to_owned(),
            version: 7,
            instructions_markdown: "# Fleet workflow".to_owned(),
            monitor_interval_ms: 600_000,
            commands: yard_standard_orchestrator_commands(),
            adapter_context_files: Vec::new(),
            source: OrchestratorWorkflowProfileSource::User,
            updated_by: "local-user".to_owned(),
            created_at_unix_ms: 1,
        };
        let compiled = with_orchestrator_workflow("Ship.", &workflow).unwrap();
        let commands = serde_json::to_string(&yard_standard_orchestrator_commands()).unwrap();

        assert!(compiled.contains(&format!(
            "Structured workflow commands (ordered JSON):\n{commands}"
        )));
        assert_eq!(workflow.commands.len(), 9);
        assert_eq!(workflow.commands[7].id, "result.integrate");
        assert_eq!(
            workflow.commands[7].capability,
            "orchestration.result.integrate"
        );
    }

    #[test]
    fn incomplete_historical_workflow_cannot_compile_for_runtime() {
        let mut workflow = OrchestratorWorkflowProfile {
            id: YARD_STANDARD_ORCHESTRATOR_PROFILE_ID.to_owned(),
            name: "Yard Standard Orchestrator".to_owned(),
            description: "Historical workflow".to_owned(),
            version: 6,
            instructions_markdown: "# Historical workflow".to_owned(),
            monitor_interval_ms: 600_000,
            commands: yard_standard_orchestrator_commands(),
            adapter_context_files: Vec::new(),
            source: OrchestratorWorkflowProfileSource::Factory,
            updated_by: "yard:v25-migration".to_owned(),
            created_at_unix_ms: 1,
        };
        workflow.commands.remove(7);

        assert!(validate_executable_orchestrator_workflow(&workflow).is_err());
        assert!(with_orchestrator_workflow("Ship.", &workflow).is_err());
    }

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
