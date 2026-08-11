use std::str::FromStr;

use xerj_corpus_publication::{ProtocolErrorKind, ResourceKey};

fn digest(prefix: &str) -> String {
    format!("{prefix}{}", "0".repeat(64))
}

fn catalog_resource_key() -> String {
    format!(
        "catalog/.xerj-autoindex-catalog-generations-v1/{}",
        digest("xerg1-sha256-")
    )
}

fn valid_resource_keys() -> Vec<String> {
    let physical = format!(
        ".xerj-aidx-d-{}-g1-s{}-t{}",
        "0".repeat(64),
        "1".repeat(64),
        "2".repeat(64)
    );
    vec![
        format!("data/{physical}"),
        catalog_resource_key(),
        format!(
            "graph-edge/.xerj-memory-life-edges/{}",
            digest("xergt1-sha256-")
        ),
        format!(
            "graph-node/.xerj-autoindex-graph-nodes-v1/{}",
            digest("xergt1-sha256-")
        ),
    ]
}

#[test]
fn embedded_nul_is_rejected_at_every_position() {
    let valid = catalog_resource_key();
    assert_eq!(valid.len(), 124);
    ResourceKey::from_str(&valid).expect("pinned catalog resource key must be valid");

    let mut case_count = 0;
    for insertion in 0..=valid.len() {
        let mut mutated = valid.clone();
        mutated.insert(insertion, '\0');
        let error = ResourceKey::from_str(&mutated).unwrap_err();
        assert_eq!(error.kind(), ProtocolErrorKind::InvalidScalar);
        case_count += 1;
    }
    assert_eq!(case_count, 125);
}

#[test]
fn embedded_nul_is_rejected_in_every_resource_grammar_component() {
    let mut accepted = Vec::new();
    for valid in valid_resource_keys() {
        ResourceKey::from_str(&valid).expect("control resource key must be valid");
        for (component_index, component) in valid.split('/').enumerate() {
            for insertion in 0..=component.len() {
                let mut mutated = valid.split('/').map(str::to_owned).collect::<Vec<_>>();
                mutated[component_index].insert(insertion, '\0');
                let mutated = mutated.join("/");
                match ResourceKey::from_str(&mutated) {
                    Ok(_) => accepted.push((valid.clone(), component_index, insertion)),
                    Err(error) => assert_eq!(error.kind(), ProtocolErrorKind::InvalidScalar),
                }
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "resource grammar components accepted embedded NUL: {accepted:?}"
    );
}
