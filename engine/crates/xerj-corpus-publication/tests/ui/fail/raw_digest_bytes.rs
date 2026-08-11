use std::str::FromStr;
fn main() { let id = xerj_corpus_publication::CorpusOwnerId::from_str("xercpo1-sha256-0000000000000000000000000000000000000000000000000000000000000000").unwrap(); let _: &[u8] = id.as_ref(); }
