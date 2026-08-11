fn needs_data(_: xerj_corpus_publication::DataMappingV1) {}
fn main() { needs_data(xerj_corpus_publication::CatalogMappingV1::parse_json(b"{}").unwrap()); }
