use std::str::FromStr;

fn needs_logical_index(_: xerj_corpus_publication::LogicalIndexName) {}

fn main() {
    needs_logical_index(xerj_corpus_publication::DataSlug::from_str("docs").unwrap());
}
