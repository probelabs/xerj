use std::str::FromStr;

use xerj_corpus_publication::{
    DataMappingV1, MappingReservationV1, ProjectionKind, ResourceKey,
};

fn main() {
    let mapping = DataMappingV1::parse_json(b"{}").unwrap();
    let resource = ResourceKey::from_str(
        "catalog/.xerj-autoindex-catalog-generations-v1/xerg1-sha256-0000000000000000000000000000000000000000000000000000000000000000",
    )
    .unwrap();
    let _ = MappingReservationV1::from_canonical_mapping(
        ProjectionKind::Data,
        resource,
        mapping.digest().clone(),
        Box::from(*b"{}"),
    );
}
