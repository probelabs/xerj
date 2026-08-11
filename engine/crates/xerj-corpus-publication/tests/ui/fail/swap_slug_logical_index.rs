use std::str::FromStr;
fn needs_slug(_: xerj_corpus_publication::DataSlug) {}
fn main() { needs_slug(xerj_corpus_publication::LogicalIndexName::from_str("life-docs").unwrap()); }
