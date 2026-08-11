fn needs_root(_: xerj_corpus_publication::RootIdentity) {}
fn main() { let tx: xerj_corpus_publication::TransactionId = "xertx1-sha256-0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(); needs_root(tx); }
