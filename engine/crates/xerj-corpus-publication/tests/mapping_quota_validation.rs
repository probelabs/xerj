mod support;

use support::absent_bundle;
use xerj_corpus_publication::{DesiredPublicationPlanV1, ProtocolErrorKind};

const PLAN_DOMAIN: &[u8] = b"xerj-desired-publication-plan-v1\0";

struct MappingOffset {
    projection: String,
    digest: std::ops::Range<usize>,
    json: std::ops::Range<usize>,
}

struct PlanOffsets {
    mappings: Vec<MappingOffset>,
    quota: usize,
}

struct Reader<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn take(&mut self, len: usize) -> &'a [u8] {
        let end = self.offset.checked_add(len).expect("test cursor overflow");
        let bytes = self
            .input
            .get(self.offset..end)
            .expect("test plan truncated");
        self.offset = end;
        bytes
    }

    fn u32(&mut self) -> u32 {
        u32::from_be_bytes(self.take(4).try_into().expect("four bytes"))
    }

    fn u64(&mut self) -> u64 {
        u64::from_be_bytes(self.take(8).try_into().expect("eight bytes"))
    }

    fn len(&mut self) -> usize {
        usize::try_from(self.u64()).expect("test length fits usize")
    }

    fn bytes_range(&mut self) -> std::ops::Range<usize> {
        let len = self.len();
        let start = self.offset;
        self.take(len);
        start..self.offset
    }

    fn string(&mut self) -> String {
        let range = self.bytes_range();
        std::str::from_utf8(&self.input[range])
            .expect("protocol string is UTF-8")
            .to_owned()
    }

    fn skip_strings(&mut self, count: usize) {
        for _ in 0..count {
            self.bytes_range();
        }
    }
}

fn plan_offsets(input: &[u8]) -> PlanOffsets {
    let mut reader = Reader::new(input);
    assert_eq!(reader.take(PLAN_DOMAIN.len()), PLAN_DOMAIN);
    assert_eq!(reader.u32(), 1);
    reader.skip_strings(4);
    reader.u64();
    reader.u64();
    reader.skip_strings(4);
    reader.u64();
    reader.skip_strings(1);
    let data_count = reader.len();
    for _ in 0..data_count {
        reader.skip_strings(4);
        reader.u64();
        reader.skip_strings(3);
    }
    reader.skip_strings(4);
    reader.u64();
    reader.skip_strings(4);
    reader.skip_strings(2);
    reader.u64();
    reader.skip_strings(7);
    reader.u64();
    reader.skip_strings(2);
    reader.u64();
    reader.skip_strings(5);

    let mapping_count = reader.len();
    let mut mappings = Vec::with_capacity(mapping_count);
    for _ in 0..mapping_count {
        let projection = reader.string();
        reader.bytes_range();
        let digest = reader.bytes_range();
        let json = reader.bytes_range();
        mappings.push(MappingOffset {
            projection,
            digest,
            json,
        });
    }
    let quota = reader.offset;
    reader.u64();
    let resources = reader.len();
    reader.skip_strings(resources);
    let tuples = reader.len();
    for _ in 0..tuples {
        reader.skip_strings(3);
        reader.u64();
        reader.u64();
        reader.skip_strings(1);
    }
    assert_eq!(reader.offset, input.len());
    PlanOffsets { mappings, quota }
}

#[test]
fn fresh_plan_persists_sorted_mapping_records_and_canonical_artifact_order() {
    let bundle = absent_bundle(1);
    let plan = bundle.desired_plan();
    let mappings = plan.mapping_reservations();
    let artifacts = bundle.replay_artifacts();

    assert_eq!(mappings.len(), 4);
    assert_eq!(mappings.len(), plan.reserved_resource_keys().len());
    assert_eq!(mappings.len(), artifacts.len());
    assert!(mappings.windows(2).all(|window| {
        (
            window[0].projection_kind().to_string(),
            window[0].resource_key().as_protocol_str(),
        ) < (
            window[1].projection_kind().to_string(),
            window[1].resource_key().as_protocol_str(),
        )
    }));
    assert!(artifacts.windows(2).all(|window| {
        (
            window[0].projection_kind().to_string(),
            window[0].resource_key().as_protocol_str(),
            window[0].kind().to_string(),
            window[0].digest().as_rendered_str(),
        ) < (
            window[1].projection_kind().to_string(),
            window[1].resource_key().as_protocol_str(),
            window[1].kind().to_string(),
            window[1].digest().as_rendered_str(),
        )
    }));

    let mapping_charge: u64 = mappings
        .iter()
        .map(|mapping| {
            32 + mapping.projection_kind().to_string().len() as u64
                + mapping.resource_key().as_protocol_str().len() as u64
                + mapping.mapping_digest().as_rendered_str().len() as u64
                + mapping.canonical_mapping_json().canonical_json().len() as u64
        })
        .sum();
    let artifact_charge: u64 = artifacts
        .iter()
        .map(|artifact| artifact.byte_length())
        .sum();
    let operation_charge: u64 = artifacts
        .iter()
        .map(|artifact| artifact.operation_count())
        .sum::<u64>()
        * 64;
    let resource_charge = mappings.len() as u64 * 4096;
    assert_eq!(
        plan.quota_charge(),
        mapping_charge + artifact_charge + operation_charge + resource_charge
    );
    assert_eq!(plan.quota_charge(), 21_116);

    let reparsed = DesiredPublicationPlanV1::parse_canonical_preimage(
        plan.canonical_preimage().canonical_preimage(),
    )
    .unwrap();
    assert_eq!(reparsed.quota_charge(), plan.quota_charge());
    assert_eq!(reparsed.mapping_reservations().len(), mappings.len());
}

#[test]
fn persisted_quota_and_mapping_bytes_are_recomputed_not_trusted() {
    let bundle = absent_bundle(1);
    let original = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    let offsets = plan_offsets(original);

    let mut wrong_quota = original.to_vec();
    let quota = u64::from_be_bytes(
        wrong_quota[offsets.quota..offsets.quota + 8]
            .try_into()
            .unwrap(),
    );
    wrong_quota[offsets.quota..offsets.quota + 8]
        .copy_from_slice(&quota.checked_add(1).unwrap().to_be_bytes());
    let error = DesiredPublicationPlanV1::parse_canonical_preimage(&wrong_quota).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
    assert_eq!(
        error.to_string(),
        "quota charge does not recompute from persisted plan bytes"
    );

    let data = offsets
        .mappings
        .iter()
        .find(|mapping| mapping.projection == "data")
        .expect("data mapping reservation");
    let mut wrong_digest = original.to_vec();
    let last = data.digest.end - 1;
    wrong_digest[last] = if wrong_digest[last] == b'0' {
        b'1'
    } else {
        b'0'
    };
    let error = DesiredPublicationPlanV1::parse_canonical_preimage(&wrong_digest).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
    assert_eq!(
        error.to_string(),
        "mapping reservation digest does not recompute"
    );

    const CANONICAL: &[u8] =
        br#"{"properties":{"body":{"type":"text"},"path":{"type":"keyword"}}}"#;
    const NONCANONICAL: &[u8] =
        br#"{"properties":{"path":{"type":"keyword"},"body":{"type":"text"}}}"#;
    assert_eq!(CANONICAL.len(), NONCANONICAL.len());
    assert_eq!(&original[data.json.clone()], CANONICAL);
    let mut noncanonical = original.to_vec();
    noncanonical[data.json.clone()].copy_from_slice(NONCANONICAL);
    let error = DesiredPublicationPlanV1::parse_canonical_preimage(&noncanonical).unwrap_err();
    assert_eq!(error.kind(), ProtocolErrorKind::NonCanonicalEncoding);
    assert_eq!(
        error.to_string(),
        "mapping reservation JSON is not RFC 8785 canonical"
    );
}
