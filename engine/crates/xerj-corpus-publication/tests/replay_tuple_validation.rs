mod support;

use sha2::{Digest, Sha256};
use support::absent_bundle;
use xerj_corpus_publication::{DesiredPublicationPlanV1, ProtocolErrorKind};

const PLAN_DOMAIN: &[u8] = b"xerj-desired-publication-plan-v1\0";
const REPLAY_SET_DOMAIN: &[u8] = b"xerj-replay-set-v1\0";

#[derive(Clone)]
struct ReplayTuple {
    kind: String,
    projection: String,
    resource: String,
    length: u64,
    count: u64,
    digest: String,
}

#[derive(Clone)]
struct MappingRecord {
    projection: String,
    resource: String,
    digest: String,
    canonical_json: Vec<u8>,
}

impl ReplayTuple {
    fn sort_key(&self) -> (&str, &str, &str, &str) {
        (&self.projection, &self.resource, &self.kind, &self.digest)
    }
}

struct PlanLayout {
    prefix_before_mappings: Vec<u8>,
    replay_set_value: std::ops::Range<usize>,
    quota: u64,
    mappings: Vec<MappingRecord>,
    resources: Vec<String>,
    tuples: Vec<ReplayTuple>,
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
        let value = self
            .input
            .get(self.offset..end)
            .expect("test plan truncated");
        self.offset = end;
        value
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

    fn string_range(&mut self) -> std::ops::Range<usize> {
        let len = self.len();
        let start = self.offset;
        self.take(len);
        start..self.offset
    }

    fn string(&mut self) -> String {
        let range = self.string_range();
        std::str::from_utf8(&self.input[range])
            .expect("test plan string is UTF-8")
            .to_owned()
    }

    fn bytes(&mut self) -> Vec<u8> {
        let range = self.string_range();
        self.input[range].to_vec()
    }

    fn skip_strings(&mut self, count: usize) {
        for _ in 0..count {
            self.string_range();
        }
    }
}

fn parse_layout(input: &[u8]) -> PlanLayout {
    let mut reader = Reader::new(input);
    assert_eq!(reader.take(PLAN_DOMAIN.len()), PLAN_DOMAIN);
    assert_eq!(reader.u32(), 1);
    reader.skip_strings(4); // owner, prefix, root, incarnation
    reader.u64(); // expected sequence
    reader.u64(); // desired sequence
    reader.skip_strings(3); // transaction, manifest, prepared input
    let replay_set_value = reader.string_range();
    reader.u64(); // generation
    reader.string_range(); // data projection
    let data_len = reader.len();
    for _ in 0..data_len {
        reader.skip_strings(4); // slug, logical route, physical target, mapping
        reader.u64(); // document count
        reader.skip_strings(3); // id, content, artifact digests
    }
    reader.skip_strings(4); // catalog storage, generation, incarnation, mapping
    reader.u64(); // catalog wrapper count
    reader.skip_strings(4); // catalog id, content, projection, artifact digests
    reader.skip_strings(2); // graph brain and owner
    reader.u64(); // graph generation
    reader.skip_strings(7); // producer through node mapping
    reader.u64(); // graph edge count
    reader.skip_strings(2); // logical and physical edge digests
    reader.u64(); // graph node count
    reader.skip_strings(5); // logical/physical node, projection and artifact digests
    let mappings_offset = reader.offset;
    let mapping_len = reader.len();
    let mappings = (0..mapping_len)
        .map(|_| MappingRecord {
            projection: reader.string(),
            resource: reader.string(),
            digest: reader.string(),
            canonical_json: reader.bytes(),
        })
        .collect();
    let quota = reader.u64();

    let resources_len = reader.len();
    let resources = (0..resources_len).map(|_| reader.string()).collect();

    let tuple_len = reader.len();
    let tuples = (0..tuple_len)
        .map(|_| ReplayTuple {
            kind: reader.string(),
            projection: reader.string(),
            resource: reader.string(),
            length: reader.u64(),
            count: reader.u64(),
            digest: reader.string(),
        })
        .collect();
    assert_eq!(reader.offset, input.len());

    PlanLayout {
        prefix_before_mappings: input[..mappings_offset].to_vec(),
        replay_set_value,
        quota,
        mappings,
        resources,
        tuples,
    }
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    push_u64(out, value.len() as u64);
    out.extend_from_slice(value.as_bytes());
}

fn push_bytes(out: &mut Vec<u8>, value: &[u8]) {
    push_u64(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn encode_mapping(out: &mut Vec<u8>, mapping: &MappingRecord) {
    push_string(out, &mapping.projection);
    push_string(out, &mapping.resource);
    push_string(out, &mapping.digest);
    push_bytes(out, &mapping.canonical_json);
}

fn encode_tuple(out: &mut Vec<u8>, tuple: &ReplayTuple) {
    push_string(out, &tuple.kind);
    push_string(out, &tuple.projection);
    push_string(out, &tuple.resource);
    push_u64(out, tuple.length);
    push_u64(out, tuple.count);
    push_string(out, &tuple.digest);
}

fn mutated_plan(mut layout: PlanLayout) -> Vec<u8> {
    layout.resources.sort_unstable();
    layout
        .tuples
        .sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

    let mut replay_preimage = REPLAY_SET_DOMAIN.to_vec();
    push_u64(&mut replay_preimage, layout.tuples.len() as u64);
    for tuple in &layout.tuples {
        encode_tuple(&mut replay_preimage, tuple);
    }
    let rendered_replay_set = format!("xerrs1-sha256-{:x}", Sha256::digest(&replay_preimage));
    assert_eq!(
        rendered_replay_set.len(),
        layout.replay_set_value.len(),
        "rendered replay-set digest keeps the canonical prefix length"
    );
    layout.prefix_before_mappings[layout.replay_set_value.clone()]
        .copy_from_slice(rendered_replay_set.as_bytes());

    let mut out = layout.prefix_before_mappings;
    push_u64(&mut out, layout.mappings.len() as u64);
    for mapping in &layout.mappings {
        encode_mapping(&mut out, mapping);
    }
    push_u64(&mut out, layout.quota);
    push_u64(&mut out, layout.resources.len() as u64);
    for resource in &layout.resources {
        push_string(&mut out, resource);
    }
    push_u64(&mut out, layout.tuples.len() as u64);
    for tuple in &layout.tuples {
        encode_tuple(&mut out, tuple);
    }
    out
}

#[test]
fn complete_replay_tuple_set_is_accepted() {
    let bundle = absent_bundle(1);
    let bytes = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    let parsed = DesiredPublicationPlanV1::parse_canonical_preimage(bytes).unwrap();
    assert_eq!(parsed.digest(), bundle.desired_plan().digest());
    let layout = parse_layout(bytes);
    assert_eq!(layout.tuples.len(), 4);
    assert_eq!(layout.mappings.len(), layout.tuples.len());
    assert!(layout.mappings.windows(2).all(|pair| {
        (&pair[0].projection, &pair[0].resource) < (&pair[1].projection, &pair[1].resource)
    }));
    assert_eq!(
        layout
            .mappings
            .iter()
            .map(|mapping| (&mapping.projection, &mapping.resource))
            .collect::<Vec<_>>(),
        layout
            .tuples
            .iter()
            .map(|tuple| (&tuple.projection, &tuple.resource))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        mutated_plan(parse_layout(bytes)),
        bytes,
        "the independent layout reader must preserve mapping records, quota, resources, and tuples"
    );
}

#[test]
fn valid_two_data_route_plan_has_one_tuple_per_route_plus_fixed_projections() {
    let bundle = support::absent_bundle_with_data_route_count(2);
    let bytes = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    let layout = parse_layout(bytes);

    assert_eq!(layout.tuples.len(), 5);
    assert_eq!(
        layout
            .tuples
            .iter()
            .filter(|tuple| tuple.projection == "data")
            .count(),
        2
    );
    let parsed = DesiredPublicationPlanV1::parse_canonical_preimage(bytes).unwrap();
    assert_eq!(parsed.digest(), bundle.desired_plan().digest());
}

#[test]
fn valid_zero_data_route_plan_has_only_fixed_projection_tuples() {
    let bundle = support::absent_bundle_with_data_route_count(0);
    let bytes = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();
    let layout = parse_layout(bytes);

    assert_eq!(layout.tuples.len(), 3);
    assert!(layout.tuples.iter().all(|tuple| tuple.projection != "data"));
    let parsed = DesiredPublicationPlanV1::parse_canonical_preimage(bytes).unwrap();
    assert_eq!(parsed.digest(), bundle.desired_plan().digest());
}

#[test]
fn removed_replay_tuple_with_recomputed_digest_is_rejected_by_cardinality() {
    let bundle = absent_bundle(1);
    let original = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();

    for projection in ["data", "catalog", "graph-edge", "graph-node"] {
        let mut layout = parse_layout(original);
        let position = layout
            .tuples
            .iter()
            .position(|tuple| tuple.projection == projection)
            .expect("projection tuple");
        let removed = layout.tuples.remove(position);
        layout
            .resources
            .retain(|resource| resource != &removed.resource);

        let error = DesiredPublicationPlanV1::parse_canonical_preimage(&mutated_plan(layout))
            .expect_err("a removed replay tuple must fail declared cardinality");
        assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
        assert_eq!(
            error.to_string(),
            "replay tuple cardinality does not match declared projections",
            "projection {projection}"
        );
    }
}

#[test]
fn same_cardinality_digest_substitution_is_rejected_by_exact_join() {
    let bundle = absent_bundle(1);
    let original = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();

    for projection in ["data", "catalog", "graph-edge", "graph-node"] {
        let mut layout = parse_layout(original);
        let replacement = layout
            .tuples
            .iter()
            .find(|tuple| tuple.projection != projection)
            .expect("another projection tuple")
            .digest
            .clone();
        let tuple = layout
            .tuples
            .iter_mut()
            .find(|tuple| tuple.projection == projection)
            .expect("projection tuple");
        assert_ne!(tuple.digest, replacement);
        tuple.digest = replacement;

        let error = DesiredPublicationPlanV1::parse_canonical_preimage(&mutated_plan(layout))
            .expect_err("a substituted replay tuple must fail the projection join");
        assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
        assert_eq!(
            error.to_string(),
            "plan projection does not join exactly one replay tuple",
            "projection {projection}"
        );
    }
}

#[test]
fn extra_internally_valid_replay_tuple_is_rejected() {
    let bundle = absent_bundle(1);
    let mut layout = parse_layout(
        bundle
            .desired_plan()
            .canonical_preimage()
            .canonical_preimage(),
    );
    let mut extra = layout
        .tuples
        .iter()
        .find(|tuple| tuple.projection == "graph-edge")
        .expect("graph-edge tuple")
        .clone();
    let (_, token) = extra
        .resource
        .rsplit_once('/')
        .expect("graph resource has token");
    extra.resource = format!("graph-edge/.xerj-memory-life-extra-edges/{token}");
    layout.resources.push(extra.resource.clone());
    layout.tuples.push(extra);

    let error = DesiredPublicationPlanV1::parse_canonical_preimage(&mutated_plan(layout))
        .expect_err("an internally valid but undeclared replay tuple must be rejected");
    assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
    assert_eq!(
        error.to_string(),
        "replay tuple cardinality does not match declared projections"
    );
}

#[test]
fn replay_tuple_operation_counts_must_match_declared_projection_counts() {
    let bundle = absent_bundle(1);
    let original = bundle
        .desired_plan()
        .canonical_preimage()
        .canonical_preimage();

    for projection in ["data", "catalog", "graph-edge", "graph-node"] {
        let mut layout = parse_layout(original);
        let tuple = layout
            .tuples
            .iter_mut()
            .find(|tuple| tuple.projection == projection)
            .expect("projection tuple");
        tuple.count = tuple.count.checked_add(1).expect("fixture count increment");

        let error = DesiredPublicationPlanV1::parse_canonical_preimage(&mutated_plan(layout))
            .expect_err("tuple operation count must join its declared projection count");
        assert_eq!(error.kind(), ProtocolErrorKind::CrossFieldMismatch);
        assert_eq!(
            error.to_string(),
            "replay tuple operation count does not match declared projection count",
            "projection {projection}"
        );
    }
}
