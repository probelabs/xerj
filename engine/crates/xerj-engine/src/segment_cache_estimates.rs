//! Deterministic retained-capacity estimates for immutable segment caches.
//!
//! These estimates are intentionally conservative and saturating. They are
//! neither allocator identities nor RSS measurements.

use serde_json::Value;

fn usize_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn capacity_bytes<T>(capacity: usize) -> u64 {
    usize_u64(capacity).saturating_mul(usize_u64(std::mem::size_of::<T>()))
}

/// Key, resident wrapper, Arc counters, and a conservative DashMap
/// slot/control allowance. `resident_inline_size` must be
/// `size_of::<CacheResident<T>>()` for the map's exact resident type.
pub fn common_entry_bytes(key: &String, resident_inline_size: usize) -> u64 {
    usize_u64(resident_inline_size)
        .saturating_add(usize_u64(2 * std::mem::size_of::<usize>()))
        .saturating_add(usize_u64(std::mem::size_of::<String>()))
        .saturating_add(usize_u64(key.capacity()))
        .saturating_add(usize_u64(6 * std::mem::size_of::<usize>()))
}

pub fn stored_slices_bytes(
    key: &String,
    resident_inline_size: usize,
    bytes_capacity: usize,
    offsets_capacity: usize,
) -> u64 {
    common_entry_bytes(key, resident_inline_size)
        .saturating_add(usize_u64(bytes_capacity))
        .saturating_add(capacity_bytes::<(u32, u32)>(offsets_capacity))
}

pub fn decoded_stored_bytes(
    key: &String,
    resident_inline_size: usize,
    bytes_capacity: usize,
) -> u64 {
    common_entry_bytes(key, resident_inline_size).saturating_add(usize_u64(bytes_capacity))
}

pub fn row_sequences_bytes(key: &String, resident_inline_size: usize, capacity: usize) -> u64 {
    common_entry_bytes(key, resident_inline_size).saturating_add(capacity_bytes::<u64>(capacity))
}

pub fn sort_shadow_bytes(
    key: &String,
    resident_inline_size: usize,
    capacity: Option<usize>,
) -> u64 {
    common_entry_bytes(key, resident_inline_size).saturating_add(
        capacity
            .map(|capacity| {
                usize_u64(std::mem::size_of::<Vec<(i64, u32)>>())
                    .saturating_add(usize_u64(2 * std::mem::size_of::<usize>()))
                    .saturating_add(capacity_bytes::<(i64, u32)>(capacity))
            })
            .unwrap_or_default(),
    )
}

pub fn id_positions_bytes(
    key: &String,
    resident_inline_size: usize,
    map: &std::collections::HashMap<String, u32>,
) -> u64 {
    let slots = usize_u64(map.capacity()).saturating_mul(
        usize_u64(std::mem::size_of::<(String, u32)>())
            .saturating_add(usize_u64(std::mem::size_of::<usize>())),
    );
    common_entry_bytes(key, resident_inline_size)
        .saturating_add(slots)
        .saturating_add(map.keys().fold(0_u64, |total, value| {
            total.saturating_add(usize_u64(value.capacity()))
        }))
}

pub fn doc_values_bytes(
    key: &String,
    resident_inline_size: usize,
    columns: &std::collections::BTreeMap<String, xerj_storage::doc_values::Column>,
) -> u64 {
    let node_allowance = usize_u64(std::mem::size_of::<(
        String,
        xerj_storage::doc_values::Column,
        [usize; 3],
    )>());
    common_entry_bytes(key, resident_inline_size).saturating_add(columns.iter().fold(
        0_u64,
        |total, (field, column)| {
            total
                .saturating_add(usize_u64(field.capacity()))
                .saturating_add(node_allowance)
                .saturating_add(column.estimated_retained_bytes())
        },
    ))
}

/// Conservative retained estimate for values produced by serde_json
/// `from_slice` and then left insert-only. All stored-value cache callers meet
/// that precondition; arbitrary remove/reserve/shrink mutation would require a
/// different capacity proof.
pub fn stored_values_bytes(key: &String, resident_inline_size: usize, values: &Vec<Value>) -> u64 {
    common_entry_bytes(key, resident_inline_size)
        .saturating_add(capacity_bytes::<Value>(values.capacity()))
        .saturating_add(values.iter().fold(0_u64, |total, value| {
            total.saturating_add(value_owned_bytes(value))
        }))
}

fn value_owned_bytes(value: &Value) -> u64 {
    match value {
        Value::String(value) => usize_u64(value.capacity()),
        Value::Array(values) => capacity_bytes::<Value>(values.capacity()).saturating_add(
            values.iter().fold(0_u64, |total, value| {
                total.saturating_add(value_owned_bytes(value))
            }),
        ),
        Value::Object(values) => {
            // Pinned serde_json 1.0.149 preserve_order → IndexMap 2.14 →
            // hashbrown 0.16. For from_slice/insert-only maps, growth leaves
            // dense entry capacity <= max(3, 2*n). The raw index table is
            // power-of-two and bounded by max(4, 2*entry_capacity). Charge a
            // full usize index plus one control byte per bucket even though
            // IndexMap uses narrower adaptive indices, plus fixed
            // control/alignment slack.
            let len = values.len();
            let entry_capacity = if len == 0 {
                0
            } else {
                len.saturating_mul(2).max(3)
            };
            let bucket_capacity = if entry_capacity == 0 {
                0
            } else {
                entry_capacity.saturating_mul(2).max(4)
            };
            let dense = capacity_bytes::<(u64, String, Value)>(entry_capacity);
            let hash_index = usize_u64(bucket_capacity)
                .saturating_mul(usize_u64(std::mem::size_of::<usize>() + 1))
                .saturating_add(if bucket_capacity == 0 { 0 } else { 64 });
            dense
                .saturating_add(hash_index)
                .saturating_add(values.iter().fold(0_u64, |total, (key, value)| {
                    total
                        .saturating_add(usize_u64(key.capacity()))
                        .saturating_add(value_owned_bytes(value))
                }))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_and_nested_json_are_counted() {
        let key = String::with_capacity(64);
        assert!(
            decoded_stored_bytes(&key, 32, 128) > decoded_stored_bytes(&String::from("x"), 32, 1)
        );

        let mut nested = Vec::with_capacity(8);
        nested.push(Value::String(String::from("owned")));
        let values = vec![Value::Array(nested)];
        let segment = String::from("seg");
        let estimate = stored_values_bytes(&segment, 64, &values);
        assert!(estimate > common_entry_bytes(&segment, 64));

        let shadow_key = String::from("seg\u{1}field");
        let none = sort_shadow_bytes(&shadow_key, 32, None);
        let some_empty = sort_shadow_bytes(&shadow_key, 32, Some(0));
        assert!(none > 0);
        assert!(
            some_empty
                >= none
                    .saturating_add(std::mem::size_of::<Vec<(i64, u32)>>() as u64)
                    .saturating_add((2 * std::mem::size_of::<usize>()) as u64)
        );

        let mut object = serde_json::Map::new();
        object.insert(
            String::with_capacity(48),
            Value::Object(serde_json::Map::with_capacity(16)),
        );
        let object_value = Value::Object(object);
        assert!(value_owned_bytes(&object_value) > 0);
    }

    #[test]
    fn insert_only_indexmap_bound_is_monotonic_at_growth_edges() {
        let mut previous = 0;
        for len in [1_usize, 3, 4, 7, 8, 15] {
            let mut object = serde_json::Map::new();
            for i in 0..len {
                object.insert(format!("key-{i}"), Value::Number(i.into()));
            }
            let estimate = value_owned_bytes(&Value::Object(object));
            let entry_capacity = len.saturating_mul(2).max(3);
            let bucket_capacity = entry_capacity.saturating_mul(2).max(4);
            let fixed = capacity_bytes::<(u64, String, Value)>(entry_capacity)
                .saturating_add(
                    usize_u64(bucket_capacity)
                        .saturating_mul(usize_u64(std::mem::size_of::<usize>() + 1)),
                )
                .saturating_add(64);
            assert!(estimate >= fixed);
            assert!(estimate >= previous);
            previous = estimate;
        }
    }

    #[test]
    fn formulas_saturate_instead_of_wrapping() {
        let key = String::from("seg");
        assert_eq!(
            decoded_stored_bytes(&key, usize::MAX, usize::MAX),
            u64::try_from(usize::MAX)
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(usize::MAX).unwrap_or(u64::MAX))
                .saturating_add(
                    u64::try_from(
                        2 * std::mem::size_of::<usize>()
                            + std::mem::size_of::<String>()
                            + key.capacity()
                            + 6 * std::mem::size_of::<usize>(),
                    )
                    .unwrap_or(u64::MAX),
                )
        );
    }
}
