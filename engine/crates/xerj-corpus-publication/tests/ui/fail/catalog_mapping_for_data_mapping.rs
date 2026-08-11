fn needs_catalog_mapping(_: xerj_corpus_publication::CatalogMappingV1) {}

fn main() {
    needs_catalog_mapping(xerj_corpus_publication::DataMappingV1::parse_json(b"{}").unwrap());
}
