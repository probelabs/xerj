use std::str::FromStr;

fn needs_extractor_identity(_: xerj_corpus_publication::ExtractorIdentity) {}

fn main() {
    needs_extractor_identity(xerj_corpus_publication::BrainName::from_str("life").unwrap());
}
