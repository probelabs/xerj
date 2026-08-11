fn main() {
    let persisted = xerj_corpus_publication::PersistedPreparedInputBytesV1::from_journal(
        Box::new([]),
    );
    let _ = persisted.0;
}
