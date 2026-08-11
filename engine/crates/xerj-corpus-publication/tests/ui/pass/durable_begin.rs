#[path = "../../support/mod.rs"] mod support;
fn main() {
    let bundle = support::absent_bundle(1);
    let _ = bundle.prepared_input().canonical_preimage().canonical_preimage();
    for artifact in bundle.replay_artifacts() { let _ = artifact.bytes().artifact_bytes(); }
    let _ = bundle.desired_plan().canonical_preimage().canonical_preimage();
    let _ = bundle.sync_begin().canonical_json().canonical_json();
}
