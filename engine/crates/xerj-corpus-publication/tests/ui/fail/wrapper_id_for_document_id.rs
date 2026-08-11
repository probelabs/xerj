use std::str::FromStr;

fn needs_wrapper_id(_: xerj_corpus_publication::WrapperId) {}

fn main() {
    needs_wrapper_id(xerj_corpus_publication::DocumentId::from_str("id").unwrap());
}
