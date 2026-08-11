use std::str::FromStr;
fn needs_root(_: xerj_corpus_publication::RootIdentity) {}
fn main() { needs_root(xerj_corpus_publication::CorpusPrefix::from_str("life").unwrap()); }
