use std::str::FromStr;

fn needs_prefix(_: xerj_corpus_publication::CorpusPrefix) {}

fn main() {
    needs_prefix(xerj_corpus_publication::RootIdentity::from_str("root").unwrap());
}
