use xerj_corpus_publication::{
    DurableBeginBundleV1, PersistedDesiredPlanBytesV1, PersistedPreparedInputBytesV1,
    PersistedSyncBeginBytesV1,
};

fn main() {
    let prepared = PersistedPreparedInputBytesV1::from_journal(Box::new([]));
    let plan = PersistedDesiredPlanBytesV1::from_journal(Box::new([]));
    let begin = PersistedSyncBeginBytesV1::from_journal(Box::new([]));
    let _ = DurableBeginBundleV1::rehydrate(plan, Vec::new(), prepared, begin);
}
