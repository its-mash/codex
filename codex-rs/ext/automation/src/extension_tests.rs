use pretty_assertions::assert_eq;

use super::sanitized_component;

#[test]
fn external_identity_scope_is_path_safe_and_stable() {
    assert_eq!(sanitized_component("Mock Team/One"), "mock_team_one");
    assert_eq!(sanitized_component("codex-worker"), "codex-worker");
}
