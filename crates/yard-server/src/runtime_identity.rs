//! Shared runtime-identity rules for active agent access.

use yard_domain::ProviderSessionRef;

/// Whether the provider session observed in Herdr matches the persisted binding for
/// *active* access (prompting, reading output, opening a terminal, routing).
///
/// The match is exact in both directions: a binding without a provider session does
/// not match a pane that now reports one, because Yard has never verified that
/// agent's identity. Reconciliation adopts a newly reported session into the binding;
/// active access has to wait for that instead of accepting it here.
///
/// Provisioning-time checks are deliberately different (`is_none_or`): there the
/// binding legitimately has no session yet. Do not use this helper for them.
pub(crate) fn active_provider_session_matches(
    expected: Option<&ProviderSessionRef>,
    observed: Option<&ProviderSessionRef>,
) -> bool {
    expected == observed
}

#[cfg(test)]
mod tests {
    use yard_domain::ProviderSessionRef;

    use super::active_provider_session_matches;

    fn session(value: &str) -> ProviderSessionRef {
        ProviderSessionRef {
            source: "herdr:codex".to_owned(),
            provider: "codex".to_owned(),
            kind: "id".to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn provider_session_requires_an_exact_observation() {
        let observed = session("observed");

        assert!(active_provider_session_matches(None, None));
        assert!(!active_provider_session_matches(None, Some(&observed)));
    }

    #[test]
    fn known_provider_session_requires_an_exact_observation() {
        let expected = session("expected");
        let observed = session("observed");

        assert!(active_provider_session_matches(
            Some(&expected),
            Some(&expected)
        ));
        assert!(!active_provider_session_matches(Some(&expected), None));
        assert!(!active_provider_session_matches(
            Some(&expected),
            Some(&observed)
        ));
    }
}
