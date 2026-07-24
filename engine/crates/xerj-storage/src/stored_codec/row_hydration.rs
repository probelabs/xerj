use super::*;
use serde::de::Deserializer as _;
use std::collections::{HashMap, HashSet};

/// Logical work performed by row-selective hydration.
///
/// These fields are not allocator or RSS measurements. In particular,
/// decompressed buffers are counted by byte length while JSON values are
/// counted as objects, so the fields must never be summed into "memory used".
#[derive(Debug, Default, PartialEq, Eq)]
pub struct StoredV2RowHydrationStats {
    pub encoded_rows_visited: usize,
    pub selected_json_values: usize,
    pub selected_dictionary_entries: usize,
    pub decompressed_buffer_bytes: usize,
    pub output_values_cloned: usize,
}

#[derive(Debug, PartialEq)]
pub struct StoredV2HydratedRow {
    pub ordinal: usize,
    pub id: serde_json::Value,
    pub seq_no: serde_json::Value,
    pub source: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, PartialEq)]
pub enum StoredV2RowHydrationResult {
    NotV2,
    Hydrated {
        rows: Vec<StoredV2HydratedRow>,
        stats: StoredV2RowHydrationStats,
    },
    /// Valid storage whose dependency shape cannot be hydrated selectively.
    /// The caller must use the compatibility path for the whole request.
    UnsupportedDependencyShape {
        column: String,
    },
}

struct SelectedRowsVisitor<'a> {
    selected: &'a [usize],
    expected_rows: usize,
}

impl<'de> serde::de::Visitor<'de> for SelectedRowsVisitor<'_> {
    type Value = Vec<serde_json::Value>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON array matching the declared stored row count")
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut selected_index = 0usize;
        let mut values = Vec::with_capacity(self.selected.len());
        for row in 0..self.expected_rows {
            if self.selected.get(selected_index).copied() == Some(row) {
                values.push(
                    sequence
                        .next_element::<serde_json::Value>()?
                        .ok_or_else(|| serde::de::Error::custom("stored column ended early"))?,
                );
                selected_index += 1;
            } else {
                sequence
                    .next_element::<serde::de::IgnoredAny>()?
                    .ok_or_else(|| serde::de::Error::custom("stored column ended early"))?;
            }
        }
        if sequence.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::custom(
                "stored column exceeds declared row count",
            ));
        }
        Ok(values)
    }
}

fn select_json_rows<R: std::io::Read>(
    reader: R,
    selected: &[usize],
    expected_rows: usize,
    context: &str,
) -> Result<Vec<serde_json::Value>> {
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let values = deserializer
        .deserialize_seq(SelectedRowsVisitor {
            selected,
            expected_rows,
        })
        .map_err(|error| StorageError::Other(anyhow::anyhow!("{context}: {error}")))?;
    deserializer
        .end()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("{context}: {error}")))?;
    Ok(values)
}

fn unpack_id(packed: &[u8], bit_width: u8, ordinal: usize) -> Option<u32> {
    let width = bit_width as usize;
    if !(1..=32).contains(&width) {
        return None;
    }
    let bit_position = ordinal.checked_mul(width)?;
    let byte_position = bit_position / 8;
    let shift = bit_position % 8;
    let needed = (width + shift).div_ceil(8);
    let bytes = packed.get(byte_position..byte_position.checked_add(needed)?)?;
    let mut word = 0u64;
    for (offset, byte) in bytes.iter().enumerate() {
        word |= (*byte as u64) << (offset * 8);
    }
    Some(((word >> shift) & ((1u64 << width) - 1)) as u32)
}

struct SelectedDict {
    values: Vec<serde_json::Value>,
    ids: Vec<u32>,
    selected_entries: usize,
    decompressed_bytes: usize,
}

fn decode_dict_rows(payload: &[u8], selected: &[usize], num_docs: usize) -> Result<SelectedDict> {
    let mut cursor = Cursor::new(payload);
    let dict_count = cursor
        .read_u32::<LittleEndian>()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("dict_count: {error}")))?
        as usize;
    let bit_width = cursor
        .read_u8()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("bit_width: {error}")))?;
    if !(1..=32).contains(&bit_width) {
        return Err(StorageError::Other(anyhow::anyhow!(
            "dict invalid bit_width {bit_width}"
        )));
    }
    let dict_len = cursor
        .read_u32::<LittleEndian>()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("dict_zstd_len: {error}")))?
        as usize;
    let dict_start = cursor.position() as usize;
    let dict_end = dict_start
        .checked_add(dict_len)
        .filter(|end| *end <= payload.len())
        .ok_or_else(|| StorageError::Other(anyhow::anyhow!("dict payload truncated")))?;
    cursor.set_position(dict_end as u64);
    let ids_len = cursor
        .read_u32::<LittleEndian>()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("ids_len: {error}")))?
        as usize;
    if ids_len != num_docs {
        return Err(StorageError::Other(anyhow::anyhow!(
            "dict ids_len {ids_len} != num_docs {num_docs}"
        )));
    }
    let packed_len = cursor
        .read_u32::<LittleEndian>()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("packed_zstd_len: {error}")))?
        as usize;
    let packed_start = cursor.position() as usize;
    let packed_end = packed_start
        .checked_add(packed_len)
        .filter(|end| *end == payload.len())
        .ok_or_else(|| StorageError::Other(anyhow::anyhow!("dict packed truncated/trailing")))?;
    let packed = zstd::decode_all(&payload[packed_start..packed_end])
        .map_err(|error| StorageError::Other(anyhow::anyhow!("packed zstd: {error}")))?;
    let expected_packed_len = num_docs
        .checked_mul(bit_width as usize)
        .ok_or_else(|| StorageError::Other(anyhow::anyhow!("dict packed length overflow")))?
        .div_ceil(8);
    if packed.len() != expected_packed_len {
        return Err(StorageError::Other(anyhow::anyhow!(
            "dict packed length {} != expected {expected_packed_len}",
            packed.len()
        )));
    }
    let used_bits = num_docs * bit_width as usize;
    if !used_bits.is_multiple_of(8) {
        let used_in_last = used_bits % 8;
        let padding_mask = !((1u8 << used_in_last) - 1);
        if packed.last().is_some_and(|byte| byte & padding_mask != 0) {
            return Err(StorageError::Other(anyhow::anyhow!(
                "dict non-zero packed padding"
            )));
        }
    }
    let null_id = dict_count as u32;
    // Validate every encoded id without constructing per-row Values.
    for ordinal in 0..num_docs {
        let id = unpack_id(&packed, bit_width, ordinal)
            .ok_or_else(|| StorageError::Other(anyhow::anyhow!("dict id truncated")))?;
        if id > null_id {
            return Err(StorageError::Other(anyhow::anyhow!(
                "dict id {id} outside dictionary"
            )));
        }
    }
    let ids: Vec<u32> = selected
        .iter()
        .map(|ordinal| {
            unpack_id(&packed, bit_width, *ordinal)
                .ok_or_else(|| StorageError::Other(anyhow::anyhow!("dict selected id truncated")))
        })
        .collect::<Result<_>>()?;
    let mut needed: Vec<usize> = ids
        .iter()
        .copied()
        .filter(|id| *id != null_id)
        .map(|id| id as usize)
        .collect();
    needed.sort_unstable();
    needed.dedup();
    let decoder = zstd::stream::read::Decoder::new(&payload[dict_start..dict_end])
        .map_err(|error| StorageError::Other(anyhow::anyhow!("dict zstd: {error}")))?;
    let entries = select_json_rows(decoder, &needed, dict_count, "dict json")?;
    let entries: HashMap<usize, serde_json::Value> = needed.iter().copied().zip(entries).collect();
    let values = ids
        .iter()
        .map(|id| {
            if *id == null_id {
                Ok(serde_json::Value::Null)
            } else {
                entries.get(&(*id as usize)).cloned().ok_or_else(|| {
                    StorageError::Other(anyhow::anyhow!("selected dict entry missing"))
                })
            }
        })
        .collect::<Result<_>>()?;
    Ok(SelectedDict {
        values,
        ids,
        selected_entries: needed.len(),
        decompressed_bytes: packed.len(),
    })
}

fn decode_cross_dep_rows(
    column: &V2ColumnRef<'_>,
    selected: &[usize],
    source_ids: &[u32],
    num_docs: usize,
    stats: &mut StoredV2RowHydrationStats,
) -> Result<Vec<serde_json::Value>> {
    let body = decode_cross_dep_body(column.payload)?;
    stats.decompressed_buffer_bytes += body.len();
    let mut cursor = Cursor::new(body.as_slice());
    let _source_index = cursor
        .read_u32::<LittleEndian>()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("cross_dep src_ix: {error}")))?;
    let dict_count = cursor
        .read_u32::<LittleEndian>()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("cross_dep dict_count: {error}")))?
        as usize;
    let mut modes = Vec::with_capacity(dict_count);
    for _ in 0..dict_count {
        modes.push(
            cursor
                .read_i64::<LittleEndian>()
                .map_err(|error| StorageError::Other(anyhow::anyhow!("cross_dep mode: {error}")))?,
        );
    }
    let exception_count = cursor
        .read_u32::<LittleEndian>()
        .map_err(|error| StorageError::Other(anyhow::anyhow!("cross_dep exc_count: {error}")))?
        as usize;
    let wanted: HashSet<u32> = selected.iter().map(|ordinal| *ordinal as u32).collect();
    let mut exceptions = HashMap::with_capacity(wanted.len().min(exception_count));
    let mut position = cursor.position() as usize;
    let mut previous = 0u32;
    for exception_index in 0..exception_count {
        let delta = u32::try_from(read_varint(&body, &mut position)?)
            .map_err(|_| StorageError::Other(anyhow::anyhow!("cross_dep ordinal overflow")))?;
        let value = read_zigzag_i64(&body, &mut position)?;
        let ordinal = previous
            .checked_add(delta)
            .ok_or_else(|| StorageError::Other(anyhow::anyhow!("cross_dep ordinal overflow")))?;
        if ordinal as usize >= num_docs || (exception_index > 0 && ordinal <= previous) {
            return Err(StorageError::Other(anyhow::anyhow!(
                "cross_dep invalid exception ordinal {ordinal}"
            )));
        }
        if wanted.contains(&ordinal) {
            exceptions.insert(ordinal, value);
        }
        previous = ordinal;
    }
    if position != body.len() {
        return Err(StorageError::Other(anyhow::anyhow!(
            "cross_dep trailing exception bytes"
        )));
    }
    Ok(selected
        .iter()
        .zip(source_ids)
        .map(|(ordinal, source_id)| {
            if let Some(value) = exceptions.get(&(*ordinal as u32)).copied() {
                if value == i64::MIN + 1 {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::Number(value.into())
                }
            } else {
                let value = modes.get(*source_id as usize).copied().unwrap_or(i64::MIN);
                if value == i64::MIN {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::Number(value.into())
                }
            }
        })
        .collect())
}

fn select_column_rows(
    column_index: usize,
    directory: &V2Directory<'_>,
    selected: &[usize],
    stats: &mut StoredV2RowHydrationStats,
    dict_ids: &mut HashMap<usize, Vec<u32>>,
) -> Result<std::result::Result<Vec<serde_json::Value>, String>> {
    let column = &directory.columns[column_index];
    let values = match column.codec {
        ColCodec::RawJson => {
            stats.encoded_rows_visited += directory.num_docs;
            let decoder = zstd::stream::read::Decoder::new(column.payload)
                .map_err(|error| StorageError::Other(anyhow::anyhow!("raw zstd: {error}")))?;
            select_json_rows(decoder, selected, directory.num_docs, "raw json")?
        }
        ColCodec::Lz4Json => {
            let raw = lz4_flex::decompress_size_prepended(column.payload)
                .map_err(|error| StorageError::Other(anyhow::anyhow!("lz4 decode: {error}")))?;
            stats.decompressed_buffer_bytes += raw.len();
            stats.encoded_rows_visited += directory.num_docs;
            select_json_rows(raw.as_slice(), selected, directory.num_docs, "lz4 json")?
        }
        ColCodec::Constant => {
            let value: serde_json::Value = serde_json::from_slice(column.payload)
                .map_err(|error| StorageError::Other(anyhow::anyhow!("constant: {error}")))?;
            vec![value; selected.len()]
        }
        ColCodec::DictBitpack => {
            let decoded = decode_dict_rows(column.payload, selected, directory.num_docs)?;
            stats.selected_dictionary_entries += decoded.selected_entries;
            stats.decompressed_buffer_bytes += decoded.decompressed_bytes;
            dict_ids.insert(column_index, decoded.ids);
            decoded.values
        }
        ColCodec::CrossDep => {
            let source_index = cross_dep_source_index(column.payload)?;
            let Some(source) = directory.columns.get(source_index) else {
                return Err(StorageError::Other(anyhow::anyhow!(
                    "cross_dep source index {source_index} out of range"
                )));
            };
            // Current encoder guarantees CROSS_DEP sources remain DICT. Valid
            // historical/handcrafted chains use the compatibility decoder.
            if source.codec != ColCodec::DictBitpack {
                // Unsupported is not an escape hatch for malformed storage:
                // validate this column's complete body and exception stream
                // before asking the caller to use the compatibility path.
                let placeholder_source_ids = vec![0; selected.len()];
                decode_cross_dep_rows(
                    column,
                    selected,
                    &placeholder_source_ids,
                    directory.num_docs,
                    stats,
                )?;
                return Ok(Err(column.name.to_string()));
            }
            if let std::collections::hash_map::Entry::Vacant(entry) = dict_ids.entry(source_index) {
                let decoded = decode_dict_rows(source.payload, selected, directory.num_docs)?;
                stats.selected_dictionary_entries += decoded.selected_entries;
                stats.decompressed_buffer_bytes += decoded.decompressed_bytes;
                entry.insert(decoded.ids);
            }
            decode_cross_dep_rows(
                column,
                selected,
                &dict_ids[&source_index],
                directory.num_docs,
                stats,
            )?
        }
    };
    stats.selected_json_values += values.len();
    Ok(Ok(values))
}

/// Reconstruct only selected ZBS2 documents without the canonical full decode.
pub fn decode_stored_v2_rows(
    bytes: &[u8],
    ordinals: &[usize],
) -> Result<StoredV2RowHydrationResult> {
    if bytes.len() < 4 || &bytes[..4] != STORED_V2_MAGIC {
        return Ok(StoredV2RowHydrationResult::NotV2);
    }
    let directory = parse_v2_directory(&bytes[4..])?;
    let id_index = directory
        .columns
        .iter()
        .position(|column| column.name == "__id")
        .ok_or_else(|| StorageError::Other(anyhow::anyhow!("v2 missing __id column")))?;
    let seq_index = directory
        .columns
        .iter()
        .position(|column| column.name == "__seq_no")
        .ok_or_else(|| StorageError::Other(anyhow::anyhow!("v2 missing __seq_no column")))?;
    let mut selected = ordinals.to_vec();
    selected.sort_unstable();
    selected.dedup();
    if selected
        .iter()
        .any(|ordinal| *ordinal >= directory.num_docs)
    {
        return Err(StorageError::Other(anyhow::anyhow!(
            "selected row outside stored section"
        )));
    }
    if selected.is_empty() {
        return Ok(StoredV2RowHydrationResult::Hydrated {
            rows: Vec::new(),
            stats: StoredV2RowHydrationStats::default(),
        });
    }
    let mut stats = StoredV2RowHydrationStats::default();
    let mut columns = Vec::with_capacity(directory.columns.len());
    let mut dict_ids = HashMap::new();
    for index in 0..directory.columns.len() {
        match select_column_rows(index, &directory, &selected, &mut stats, &mut dict_ids)? {
            Ok(values) => columns.push(values),
            Err(column) => {
                return Ok(StoredV2RowHydrationResult::UnsupportedDependencyShape { column });
            }
        }
    }
    let mut rows = Vec::with_capacity(selected.len());
    for (selected_index, ordinal) in selected.into_iter().enumerate() {
        let mut source = serde_json::Map::new();
        for (column_index, column) in directory.columns.iter().enumerate() {
            if column_index == id_index || column_index == seq_index {
                continue;
            }
            let value = columns[column_index][selected_index].clone();
            if !value.is_null() {
                source.insert(column.name.to_string(), value);
                stats.output_values_cloned += 1;
            }
        }
        stats.output_values_cloned += 2;
        rows.push(StoredV2HydratedRow {
            ordinal,
            id: columns[id_index][selected_index].clone(),
            seq_no: columns[seq_index][selected_index].clone(),
            source,
        });
    }
    Ok(StoredV2RowHydrationResult::Hydrated { rows, stats })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assemble(num_docs: usize, columns: &[(&str, ColCodec, Vec<u8>)]) -> Vec<u8> {
        let mut out = STORED_V2_MAGIC.to_vec();
        out.write_u32::<LittleEndian>(num_docs as u32).unwrap();
        out.write_u32::<LittleEndian>(columns.len() as u32).unwrap();
        for (name, codec, payload) in columns {
            out.write_u16::<LittleEndian>(name.len() as u16).unwrap();
            out.extend_from_slice(name.as_bytes());
            out.push(*codec as u8);
            out.write_u32::<LittleEndian>(payload.len() as u32).unwrap();
            out.extend_from_slice(payload);
        }
        out
    }

    fn raw(values: &[serde_json::Value]) -> Vec<u8> {
        zstd::encode_all(
            Cursor::new(serde_json::to_vec(values).unwrap()),
            STORED_ZSTD_LEVEL,
        )
        .unwrap()
    }

    fn fixture(n: usize, body_codec: ColCodec) -> Vec<u8> {
        let ids: Vec<_> = (0..n).map(|i| json!(format!("doc-{i}"))).collect();
        let seq: Vec<_> = (0..n).map(|i| json!(i)).collect();
        let body: Vec<_> = (0..n)
            .map(|i| json!({"row": i, "text": format!("payload-{i}")}))
            .collect();
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let body_payload = match body_codec {
            ColCodec::RawJson => zstd::encode_all(Cursor::new(body_bytes), 3).unwrap(),
            ColCodec::Lz4Json => lz4_flex::compress_prepend_size(&body_bytes),
            _ => unreachable!(),
        };
        assemble(
            n,
            &[
                ("__id", ColCodec::RawJson, raw(&ids)),
                ("__seq_no", ColCodec::RawJson, raw(&seq)),
                ("body", body_codec, body_payload),
            ],
        )
    }

    #[test]
    fn raw_and_lz4_select_rows_and_reject_trailing_json() {
        for codec in [ColCodec::RawJson, ColCodec::Lz4Json] {
            let encoded = fixture(128, codec);
            let StoredV2RowHydrationResult::Hydrated { rows, stats } =
                decode_stored_v2_rows(&encoded, &[127, 0, 7, 7]).unwrap()
            else {
                panic!("expected hydration")
            };
            assert_eq!(
                rows.iter().map(|row| row.ordinal).collect::<Vec<_>>(),
                [0, 7, 127]
            );
            assert_eq!(rows[1].source["body"]["row"], 7);
            assert_eq!(stats.output_values_cloned, 9);
            if codec == ColCodec::Lz4Json {
                assert!(stats.decompressed_buffer_bytes > 0);
            }
        }

        let mut bad_values = serde_json::to_vec(&vec![json!("x"); 4]).unwrap();
        bad_values.extend_from_slice(b"[]");
        let bad = assemble(
            4,
            &[
                ("__id", ColCodec::RawJson, raw(&vec![json!("x"); 4])),
                ("__seq_no", ColCodec::RawJson, raw(&vec![json!(0); 4])),
                (
                    "bad",
                    ColCodec::Lz4Json,
                    lz4_flex::compress_prepend_size(&bad_values),
                ),
            ],
        );
        assert!(decode_stored_v2_rows(&bad, &[0]).is_err());
    }

    #[test]
    fn dictionary_nulls_boundaries_and_corruption_are_validated() {
        let values = vec![json!("a"), json!("b"), json!("c")];
        let ids = vec![0, 1, 3, 2, 0, 3, 1, 2];
        let encoded = assemble(
            ids.len(),
            &[
                (
                    "__id",
                    ColCodec::DictBitpack,
                    encode_dict_bitpack(&values, &ids),
                ),
                (
                    "__seq_no",
                    ColCodec::Constant,
                    serde_json::to_vec(&json!(9)).unwrap(),
                ),
            ],
        );
        let StoredV2RowHydrationResult::Hydrated { rows, stats } =
            decode_stored_v2_rows(&encoded, &[0, 2, 7]).unwrap()
        else {
            panic!("expected hydration")
        };
        assert_eq!(rows[0].id, "a");
        assert!(rows[1].id.is_null());
        assert_eq!(rows[2].id, "c");
        assert_eq!(stats.selected_dictionary_entries, 2);

        for width in 1..=32 {
            let max = if width == 32 {
                u32::MAX
            } else {
                (1u32 << width) - 1
            };
            let packed = bitpack_u32(&[0, max, max / 2], width);
            assert_eq!(unpack_id(&packed, width, 1), Some(max));
            assert_eq!(unpack_id(&packed, width, 2), Some(max / 2));
            assert_eq!(unpack_id(&packed[..packed.len() - 1], width, 2), None);
        }

        let mut corrupt = encode_dict_bitpack(&[json!("only")], &[0, 0, 0]);
        // Rebuild the packed stream with id=3, which is outside {entry,null}.
        let dict_len = u32::from_le_bytes(corrupt[5..9].try_into().unwrap()) as usize;
        let packed_offset = 9 + dict_len + 8;
        let bad_packed = zstd::encode_all(Cursor::new(bitpack_u32(&[0, 3, 0], 2)), 3).unwrap();
        corrupt.truncate(packed_offset);
        corrupt.extend_from_slice(&bad_packed);
        corrupt[4] = 2;
        let length_offset = 9 + dict_len + 4;
        corrupt[length_offset..length_offset + 4]
            .copy_from_slice(&(bad_packed.len() as u32).to_le_bytes());
        let bad = assemble(
            3,
            &[
                ("__id", ColCodec::DictBitpack, corrupt),
                (
                    "__seq_no",
                    ColCodec::Constant,
                    serde_json::to_vec(&json!(0)).unwrap(),
                ),
            ],
        );
        assert!(decode_stored_v2_rows(&bad, &[0]).is_err());

        let mut padding = encode_dict_bitpack(&[json!("only")], &[0, 0, 0]);
        let dict_len = u32::from_le_bytes(padding[5..9].try_into().unwrap()) as usize;
        let packed_length_offset = 9 + dict_len + 4;
        let packed_offset = packed_length_offset + 4;
        let mut unpacked = zstd::decode_all(&padding[packed_offset..]).unwrap();
        unpacked[0] |= 0b1000_0000;
        let repacked = zstd::encode_all(Cursor::new(unpacked), 3).unwrap();
        padding.truncate(packed_offset);
        padding.extend_from_slice(&repacked);
        padding[packed_length_offset..packed_offset]
            .copy_from_slice(&(repacked.len() as u32).to_le_bytes());
        let bad_padding = assemble(
            3,
            &[
                ("__id", ColCodec::DictBitpack, padding),
                (
                    "__seq_no",
                    ColCodec::Constant,
                    serde_json::to_vec(&json!(0)).unwrap(),
                ),
            ],
        );
        assert!(decode_stored_v2_rows(&bad_padding, &[0]).is_err());
    }

    #[test]
    fn selection_validation_empty_fast_path_and_fixed_r_work() {
        assert!(matches!(
            decode_stored_v2_rows(b"plain", &[0]).unwrap(),
            StoredV2RowHydrationResult::NotV2
        ));
        let encoded = fixture(128, ColCodec::RawJson);
        assert!(decode_stored_v2_rows(&encoded, &[128]).is_err());
        let StoredV2RowHydrationResult::Hydrated { rows, stats } =
            decode_stored_v2_rows(&encoded, &[]).unwrap()
        else {
            panic!("expected hydration")
        };
        assert!(rows.is_empty());
        assert_eq!(stats, StoredV2RowHydrationStats::default());

        let small = decode_stored_v2_rows(&fixture(128, ColCodec::RawJson), &[1, 9, 17]).unwrap();
        let large = decode_stored_v2_rows(&fixture(1024, ColCodec::RawJson), &[1, 9, 17]).unwrap();
        let selected = |result| match result {
            StoredV2RowHydrationResult::Hydrated { rows, stats } => (
                rows.len(),
                stats.selected_json_values,
                stats.output_values_cloned,
            ),
            _ => panic!("expected hydration"),
        };
        assert_eq!(selected(small), selected(large));
    }

    #[test]
    fn malformed_framing_and_unsupported_dependency_are_explicit() {
        let encoded = fixture(4, ColCodec::RawJson);
        assert!(decode_stored_v2_rows(&encoded[..encoded.len() - 1], &[0]).is_err());

        let valid_cross_dep = |source_index: u32| {
            let mut bytes = vec![0];
            bytes.extend_from_slice(&source_index.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes
        };
        let unsupported = assemble(
            1,
            &[
                ("__id", ColCodec::RawJson, raw(&[json!("a")])),
                ("__seq_no", ColCodec::RawJson, raw(&[json!(0)])),
                ("dep", ColCodec::CrossDep, valid_cross_dep(0)),
            ],
        );
        assert!(matches!(
            decode_stored_v2_rows(&unsupported, &[0]).unwrap(),
            StoredV2RowHydrationResult::UnsupportedDependencyShape { .. }
        ));

        let malformed = assemble(
            1,
            &[
                ("__id", ColCodec::RawJson, raw(&[json!("a")])),
                ("__seq_no", ColCodec::RawJson, raw(&[json!(0)])),
                ("dep", ColCodec::CrossDep, vec![0, 0, 0, 0, 0]),
            ],
        );
        assert!(decode_stored_v2_rows(&malformed, &[0]).is_err());
    }

    #[test]
    fn selected_cross_dep_exceptions_and_nulls_match_full_semantics() {
        let mut dependent = vec![0];
        dependent.extend_from_slice(&0u32.to_le_bytes()); // source column
        dependent.extend_from_slice(&2u32.to_le_bytes());
        dependent.extend_from_slice(&10i64.to_le_bytes());
        dependent.extend_from_slice(&20i64.to_le_bytes());
        dependent.extend_from_slice(&2u32.to_le_bytes());
        write_varint(&mut dependent, 1);
        write_zigzag_i64(&mut dependent, i64::MIN + 1);
        write_varint(&mut dependent, 2);
        write_zigzag_i64(&mut dependent, 99);
        let encoded = assemble(
            4,
            &[
                (
                    "__id",
                    ColCodec::DictBitpack,
                    encode_dict_bitpack(&[json!("a"), json!("b")], &[0, 1, 0, 1]),
                ),
                ("__seq_no", ColCodec::CrossDep, dependent),
            ],
        );
        let StoredV2RowHydrationResult::Hydrated { rows, .. } =
            decode_stored_v2_rows(&encoded, &[0, 1, 2, 3]).unwrap()
        else {
            panic!("expected hydration")
        };
        let canonical: Vec<serde_json::Value> =
            serde_json::from_slice(&decode_stored(&encoded).unwrap()).unwrap();
        for (row, full) in rows.iter().zip(&canonical) {
            assert_eq!(row.id, full["_id"]);
            assert_eq!(row.seq_no, full["_seq_no"]);
            assert_eq!(
                serde_json::Value::Object(row.source.clone()),
                full["_source"]
            );
        }
        assert_eq!(rows[0].seq_no, 10);
        assert!(rows[1].seq_no.is_null());
        assert_eq!(rows[2].seq_no, 10);
        assert_eq!(rows[3].seq_no, 99);
    }

    #[test]
    fn adaptive_encoder_rows_match_canonical_decode_across_source_shapes() {
        let sources: Vec<_> = (0..256)
            .map(|i| {
                json!({
                    "company": if i % 3 == 0 { "Acme" } else { "Globex" },
                    "quarter": (i % 4) + 1,
                    "constant": true,
                    "nested": {"amount": i as f64 / 7.0, "tags": ["finance", i]},
                    "nullable": if i % 11 == 0 { serde_json::Value::Null } else { json!(i % 5) }
                })
            })
            .collect();
        let ids: Vec<_> = (0..256).map(|i| format!("doc-{i}")).collect();
        let docs: Vec<_> = (0..256)
            .map(|i| (ids[i].as_str(), i as u64, &sources[i]))
            .collect();
        let encoded = encode_stored_v2_from_values_nojson(&docs);
        assert_eq!(&encoded[..4], STORED_V2_MAGIC);
        let full: Vec<serde_json::Value> =
            serde_json::from_slice(&decode_stored(&encoded).unwrap()).unwrap();
        let selected = [0, 7, 42, 128, 255];
        let StoredV2RowHydrationResult::Hydrated { rows, .. } =
            decode_stored_v2_rows(&encoded, &selected).unwrap()
        else {
            panic!("current encoder must have a selectively supported dependency shape")
        };
        for (row, ordinal) in rows.iter().zip(selected) {
            assert_eq!(row.id, full[ordinal]["_id"]);
            assert_eq!(row.seq_no, full[ordinal]["_seq_no"]);
            assert_eq!(
                serde_json::Value::Object(row.source.clone()),
                full[ordinal]["_source"]
            );
        }
    }
}
