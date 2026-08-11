use std::str::FromStr;
fn needs_brain(_: xerj_corpus_publication::BrainName) {}
fn main() { needs_brain(xerj_corpus_publication::ExtractorIdentity::from_str("extractor@1").unwrap()); }
