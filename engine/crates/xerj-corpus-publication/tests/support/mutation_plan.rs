use sha2::{Digest, Sha256};

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

struct Layout {
    prefix_before_mappings: Vec<u8>,
    replay_set_value: std::ops::Range<usize>,
    first_mapping_json_len_offset: usize,
    owner: String,
    incarnation: String,
    transaction: String,
    generation: u64,
    catalog_generation: String,
    catalog_incarnation_value: std::ops::Range<usize>,
    catalog_count_offset: usize,
    catalog_content: String,
    catalog_projection_value: std::ops::Range<usize>,
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
        let end = self.offset.checked_add(len).unwrap();
        let value = &self.input[self.offset..end];
        self.offset = end;
        value
    }
    fn u32(&mut self) -> u32 {
        u32::from_be_bytes(self.take(4).try_into().unwrap())
    }
    fn u64(&mut self) -> u64 {
        u64::from_be_bytes(self.take(8).try_into().unwrap())
    }
    fn string_range(&mut self) -> std::ops::Range<usize> {
        let len = usize::try_from(self.u64()).unwrap();
        let start = self.offset;
        self.take(len);
        start..self.offset
    }
    fn string(&mut self) -> String {
        let range = self.string_range();
        std::str::from_utf8(&self.input[range]).unwrap().to_owned()
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

fn parse(input: &[u8]) -> Layout {
    let mut reader = Reader::new(input);
    assert_eq!(reader.take(PLAN_DOMAIN.len()), PLAN_DOMAIN);
    assert_eq!(reader.u32(), 1);
    let owner = reader.string();
    reader.skip_strings(2);
    let incarnation = reader.string();
    reader.u64();
    reader.u64();
    let transaction = reader.string();
    reader.skip_strings(2);
    let replay_set_value = reader.string_range();
    let generation = reader.u64();
    reader.string_range();
    let data_len = reader.u64();
    for _ in 0..data_len {
        reader.skip_strings(4);
        reader.u64();
        reader.skip_strings(3);
    }
    reader.string_range();
    let catalog_generation = reader.string();
    let catalog_incarnation_value = reader.string_range();
    reader.string_range();
    let catalog_count_offset = reader.offset;
    reader.u64();
    reader.skip_strings(1);
    let catalog_content = reader.string();
    let catalog_projection_value = reader.string_range();
    reader.string_range();
    reader.skip_strings(2);
    reader.u64();
    reader.skip_strings(7);
    reader.u64();
    reader.skip_strings(2);
    reader.u64();
    reader.skip_strings(5);
    let mappings_offset = reader.offset;
    let mapping_len = reader.u64();
    let mut first_mapping_json_len_offset = None;
    let mappings = (0..mapping_len)
        .map(|index| MappingRecord {
            projection: reader.string(),
            resource: reader.string(),
            digest: reader.string(),
            canonical_json: {
                if index == 0 {
                    first_mapping_json_len_offset = Some(reader.offset);
                }
                reader.bytes()
            },
        })
        .collect();
    let quota = reader.u64();
    let resources_len = reader.u64();
    let resources = (0..resources_len).map(|_| reader.string()).collect();
    let tuple_len = reader.u64();
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
    Layout {
        prefix_before_mappings: input[..mappings_offset].to_vec(),
        replay_set_value,
        first_mapping_json_len_offset: first_mapping_json_len_offset.unwrap(),
        owner,
        incarnation,
        transaction,
        generation,
        catalog_generation,
        catalog_incarnation_value,
        catalog_count_offset,
        catalog_content,
        catalog_projection_value,
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

fn encode(mut layout: Layout) -> Vec<u8> {
    let mut replay = REPLAY_SET_DOMAIN.to_vec();
    push_u64(&mut replay, layout.tuples.len() as u64);
    for tuple in &layout.tuples {
        encode_tuple(&mut replay, tuple);
    }
    let rendered = format!("xerrs1-sha256-{:x}", Sha256::digest(replay));
    layout.prefix_before_mappings[layout.replay_set_value.clone()]
        .copy_from_slice(rendered.as_bytes());
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

fn flip_last_hex(value: &mut String) {
    let last = value.pop().unwrap();
    value.push(if last == '0' { '1' } else { '0' });
}

fn mapping_digest(json: &[u8]) -> String {
    let mut preimage = b"xerj-mapping-v1\0".to_vec();
    push_bytes(&mut preimage, json);
    format!("xermap1-sha256-{:x}", Sha256::digest(preimage))
}

fn rendered_digest(prefix: &str, preimage: &[u8]) -> String {
    format!("{prefix}{:x}", Sha256::digest(preimage))
}

fn replace_fixed(bytes: &mut [u8], range: std::ops::Range<usize>, replacement: &str) {
    assert_eq!(range.len(), replacement.len());
    bytes[range].copy_from_slice(replacement.as_bytes());
}

fn set_catalog_count(layout: &mut Layout, catalog_count: u64) {
    let catalog_tuple = layout
        .tuples
        .iter_mut()
        .find(|tuple| tuple.projection == "catalog")
        .unwrap();
    catalog_tuple.count = catalog_count;

    layout.prefix_before_mappings[layout.catalog_count_offset..layout.catalog_count_offset + 8]
        .copy_from_slice(&catalog_count.to_be_bytes());

    let mut projection = Vec::from(b"xerj-catalog-projection-v1\0".as_slice());
    push_string(&mut projection, &layout.owner);
    push_string(&mut projection, &layout.incarnation);
    push_u64(&mut projection, layout.generation);
    push_string(&mut projection, &layout.catalog_generation);
    push_u64(&mut projection, catalog_count);
    push_string(&mut projection, &layout.catalog_content);
    let projection = rendered_digest("xercatp1-sha256-", &projection);
    replace_fixed(
        &mut layout.prefix_before_mappings,
        layout.catalog_projection_value.clone(),
        &projection,
    );

    let mut incarnation = Vec::from(b"xerj-catalog-generation-incarnation-v1\0".as_slice());
    push_string(&mut incarnation, &layout.owner);
    push_string(&mut incarnation, &layout.incarnation);
    push_u64(&mut incarnation, layout.generation);
    push_string(&mut incarnation, &layout.transaction);
    push_string(&mut incarnation, &projection);
    let incarnation = rendered_digest("xercati1-sha256-", &incarnation);
    replace_fixed(
        &mut layout.prefix_before_mappings,
        layout.catalog_incarnation_value.clone(),
        &incarnation,
    );
}

fn operation_stage_overflow(mut layout: Layout) -> Layout {
    let other_operations = layout
        .tuples
        .iter()
        .filter(|tuple| tuple.projection != "catalog")
        .map(|tuple| tuple.count)
        .sum::<u64>();
    let catalog_count = u64::MAX / 64 - other_operations;
    set_catalog_count(&mut layout, catalog_count);
    layout.quota = u64::MAX;
    layout
}

pub fn mapping_case(input: &[u8], case: usize) -> Vec<u8> {
    let mut layout = parse(input);
    match case {
        0 | 16 => {
            layout.mappings.remove(0);
        }
        1 | 17 => {
            let mut extra = layout.mappings[0].clone();
            extra.projection = "data".to_owned();
            extra.resource = layout.mappings[1].resource.clone();
            layout.mappings.push(extra);
        }
        2 => layout.mappings.push(layout.mappings[0].clone()),
        3 => layout.mappings.reverse(),
        4 => layout.mappings[0].projection = "data".to_owned(),
        5 => layout.mappings.swap(0, 1),
        6 => flip_last_hex(&mut layout.mappings[0].digest),
        7 => layout.mappings[0].canonical_json[0] = b'[',
        8 => {
            layout.mappings[0].canonical_json = br#"{"changed":true}"#.to_vec();
            layout.mappings[0].digest = mapping_digest(&layout.mappings[0].canonical_json);
        }
        9 => {
            layout.mappings[0].canonical_json = br#"{"z":1,"a":2}"#.to_vec();
            layout.mappings[0].digest = mapping_digest(&layout.mappings[0].canonical_json);
        }
        10 => {
            layout.mappings[0].canonical_json = br#"{"a":1,"a":1}"#.to_vec();
            layout.mappings[0].digest = mapping_digest(&layout.mappings[0].canonical_json);
        }
        11 => {
            layout.mappings[0].canonical_json = b"[]".to_vec();
            layout.mappings[0].digest = mapping_digest(&layout.mappings[0].canonical_json);
        }
        12 => layout.mappings[1].digest = layout.mappings[0].digest.clone(),
        13 => layout.resources.swap(0, 1),
        14 => layout.tuples[0].resource = layout.tuples[1].resource.clone(),
        15 => layout.tuples[0].projection = "data".to_owned(),
        _ => unreachable!(),
    }
    encode(layout)
}

pub fn quota_case(input: &[u8], case: usize) -> Vec<u8> {
    let mut layout = parse(input);
    if matches!(case, 0 | 1) {
        let mut encoded = input.to_vec();
        let offset = layout.first_mapping_json_len_offset;
        let length = u64::from_be_bytes(encoded[offset..offset + 8].try_into().unwrap());
        let changed = if case == 0 { length - 1 } else { length + 1 };
        encoded[offset..offset + 8].copy_from_slice(&changed.to_be_bytes());
        return encoded;
    }
    let _: Option<()> = match case {
        0 | 1 => unreachable!(),
        2 => {
            layout.tuples[0].length = layout.tuples[0].length.saturating_sub(1);
            None
        }
        3 => {
            layout.tuples[0].length += 1;
            None
        }
        4 => {
            layout.tuples[0].count = layout.tuples[0].count.saturating_sub(1);
            None
        }
        5 => {
            layout.tuples[0].count += 1;
            None
        }
        6 => {
            layout.resources.remove(0);
            None
        }
        7 => {
            layout.resources.push(layout.resources[0].clone());
            None
        }
        8 => {
            layout.quota -= 1;
            None
        }
        9 => {
            layout.quota += 1;
            None
        }
        10 => {
            layout.quota = layout.quota.saturating_sub(8);
            None
        }
        11 => {
            layout.quota += 32;
            None
        }
        12 => {
            layout.mappings[0].canonical_json = vec![b' '; 32];
            layout.quota = u64::MAX;
            None
        }
        13 => {
            layout.tuples[0].length = u64::MAX;
            layout.tuples[1].length = 1;
            None
        }
        14 => {
            set_catalog_count(&mut layout, u64::MAX);
            None
        }
        15 => {
            set_catalog_count(&mut layout, u64::MAX / 64 + 1);
            None
        }
        16 => {
            layout.resources.push(layout.resources[0].clone());
            layout.quota = u64::MAX;
            None
        }
        17 => {
            let other_lengths = layout.tuples[1..]
                .iter()
                .map(|tuple| tuple.length)
                .sum::<u64>();
            layout.tuples[0].length = u64::MAX - other_lengths;
            None
        }
        18 => {
            return encode(operation_stage_overflow(layout));
        }
        _ => unreachable!(),
    };
    encode(layout)
}

pub fn tuple_case(input: &[u8], case: usize) -> Vec<u8> {
    let mut layout = parse(input);
    let replay_set_start = layout.replay_set_value.start;
    match case {
        0 => layout.tuples[0].kind = "data-bulk-ndjson".to_owned(),
        1 => layout.tuples[0].projection = "data".to_owned(),
        2 => layout.tuples[0].resource = layout.tuples[1].resource.clone(),
        3 => layout.tuples[0].length += 1,
        4 => layout.tuples[0].count += 1,
        5 => flip_last_hex(&mut layout.tuples[0].digest),
        6 => layout.tuples.push(layout.tuples[0].clone()),
        7 => {
            layout.tuples.remove(0);
        }
        8 => {
            let mut extra = layout.tuples[0].clone();
            extra.resource = layout.tuples[1].resource.clone();
            layout.tuples.push(extra);
        }
        9 => layout.tuples.reverse(),
        10 => layout.tuples.swap(0, 1),
        11 => layout.tuples[1].digest = layout.tuples[0].digest.clone(),
        12 => layout.tuples[1].count += 1,
        13 => {}
        _ => unreachable!(),
    }
    let mut encoded = encode(layout);
    if case == 13 {
        encoded[replay_set_start] ^= 1;
    }
    encoded
}

pub fn ordering_case(input: &[u8], case: usize) -> Vec<u8> {
    let mut layout = parse(input);
    match case {
        0 => layout.mappings.reverse(),
        1 => layout.resources.reverse(),
        2 => layout.tuples.reverse(),
        _ => unreachable!(),
    }
    encode(layout)
}

fn reverse_spans(input: &[u8], spans: &[std::ops::Range<usize>]) -> Vec<u8> {
    assert!(spans.len() >= 2);
    let mut output = Vec::with_capacity(input.len());
    output.extend_from_slice(&input[..spans[0].start]);
    for span in spans.iter().rev() {
        output.extend_from_slice(&input[span.clone()]);
    }
    output.extend_from_slice(&input[spans.last().unwrap().end..]);
    output
}

pub fn reverse_prepared_data_routes(input: &[u8]) -> Vec<u8> {
    const DOMAIN: &[u8] = b"xerj-prepared-input-v1\0";
    let mut reader = Reader::new(input);
    assert_eq!(reader.take(DOMAIN.len()), DOMAIN);
    assert_eq!(reader.u32(), 1);
    reader.skip_strings(3);
    let count = usize::try_from(reader.u64()).unwrap();
    let mut spans = Vec::with_capacity(count);
    for _ in 0..count {
        let start = reader.offset;
        reader.skip_strings(2);
        reader.u64();
        reader.skip_strings(3);
        spans.push(start..reader.offset);
    }
    reverse_spans(input, &spans)
}

pub fn reverse_plan_data_entries(input: &[u8]) -> Vec<u8> {
    let mut reader = Reader::new(input);
    assert_eq!(reader.take(PLAN_DOMAIN.len()), PLAN_DOMAIN);
    assert_eq!(reader.u32(), 1);
    reader.skip_strings(4);
    reader.u64();
    reader.u64();
    reader.skip_strings(4);
    reader.u64();
    reader.string_range();
    let count = usize::try_from(reader.u64()).unwrap();
    let mut spans = Vec::with_capacity(count);
    for _ in 0..count {
        let start = reader.offset;
        reader.skip_strings(4);
        reader.u64();
        reader.skip_strings(3);
        spans.push(start..reader.offset);
    }
    reverse_spans(input, &spans)
}

pub fn cross_file_plan_field_case(input: &[u8], case: usize) -> Vec<u8> {
    let mut reader = Reader::new(input);
    assert_eq!(reader.take(PLAN_DOMAIN.len()), PLAN_DOMAIN);
    assert_eq!(reader.u32(), 1);
    let owner = reader.string_range();
    reader.skip_strings(2);
    let incarnation = reader.string_range();
    reader.u64();
    reader.u64();
    reader.string_range();
    let manifest = reader.string_range();
    reader.skip_strings(2);
    reader.u64();
    reader.string_range();
    let data_len = reader.u64();
    assert!(data_len > 0);
    reader.skip_strings(4);
    let data_count = reader.offset;
    reader.u64();
    reader.skip_strings(3);
    for _ in 1..data_len {
        reader.skip_strings(4);
        reader.u64();
        reader.skip_strings(3);
    }
    reader.skip_strings(4);
    let catalog_count = reader.offset;
    reader.u64();
    reader.skip_strings(4);
    reader.skip_strings(2);
    reader.u64();
    reader.skip_strings(7);
    let edge_count = reader.offset;
    reader.u64();
    reader.skip_strings(2);
    let node_count = reader.offset;
    reader.u64();

    let mut encoded = input.to_vec();
    match case {
        0 => flip_rendered_value(&mut encoded, owner),
        1 => flip_rendered_value(&mut encoded, incarnation),
        2 => flip_rendered_value(&mut encoded, manifest),
        3 => increment_u64(&mut encoded, data_count),
        4 => increment_u64(&mut encoded, catalog_count),
        5 => increment_u64(&mut encoded, edge_count),
        6 => increment_u64(&mut encoded, node_count),
        _ => unreachable!(),
    }
    encoded
}

fn flip_rendered_value(bytes: &mut [u8], range: std::ops::Range<usize>) {
    let offset = range.end - 1;
    bytes[offset] = if bytes[offset] == b'0' { b'1' } else { b'0' };
}

fn increment_u64(bytes: &mut [u8], offset: usize) {
    let value = u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
    bytes[offset..offset + 8].copy_from_slice(&(value + 1).to_be_bytes());
}
