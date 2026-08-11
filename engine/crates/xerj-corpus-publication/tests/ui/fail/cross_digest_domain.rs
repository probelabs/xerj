fn needs_manifest(_: xerj_corpus_publication::ManifestDigest) {}
fn main() { needs_manifest(xerj_corpus_publication::DataMappingV1::parse_json(b"{}").unwrap().digest().clone()); }
