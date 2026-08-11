#[path = "../../support/mod.rs"]
mod support;

use xerj_corpus_publication::{
    DurableBeginBundleV1, PersistedDesiredPlanBytesV1, PersistedPreparedInputBytesV1,
    PersistedReplayArtifactBytesV1, PersistedSyncBeginBytesV1,
};

fn main() {
    let fresh = support::absent_bundle(1);
    let recovered = DurableBeginBundleV1::rehydrate(
        PersistedPreparedInputBytesV1::from_journal(
            fresh
                .prepared_input()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        fresh
            .replay_artifacts()
            .iter()
            .map(|artifact| {
                PersistedReplayArtifactBytesV1::from_journal(
                    artifact.bytes().artifact_bytes().into(),
                )
            })
            .collect(),
        PersistedDesiredPlanBytesV1::from_journal(
            fresh
                .desired_plan()
                .canonical_preimage()
                .canonical_preimage()
                .into(),
        ),
        PersistedSyncBeginBytesV1::from_journal(
            fresh.sync_begin().canonical_json().canonical_json().into(),
        ),
    )
    .unwrap();
    assert_eq!(fresh.sync_begin().digest(), recovered.sync_begin().digest());
}
