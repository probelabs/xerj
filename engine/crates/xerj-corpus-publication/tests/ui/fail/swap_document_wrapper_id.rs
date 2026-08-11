use std::str::FromStr;
fn needs_document(_: xerj_corpus_publication::DocumentId) {}
fn main() { needs_document(xerj_corpus_publication::WrapperId::from_str("id").unwrap()); }
