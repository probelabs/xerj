use xerj_corpus_publication::{CorpusPublicationV1, ExpectedPublicationV1};
fn main() {
    let publication = CorpusPublicationV1::parse_closed_json(include_bytes!("../../../testdata/review11-v1/publication.json")).unwrap();
    let expected = ExpectedPublicationV1::present(publication).unwrap();
    let parsed = ExpectedPublicationV1::parse_closed_json(expected.canonical_json().canonical_json()).unwrap();
    assert_eq!(parsed.digest(), expected.digest());
}
