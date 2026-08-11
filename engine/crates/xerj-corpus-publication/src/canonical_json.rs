use crate::error::{error, ProtocolErrorKind, Result};
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde_json::Number;
use std::{cmp::Ordering, collections::HashSet, fmt};

#[derive(Clone, PartialEq)]
pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

struct Seed;

impl<'de> DeserializeSeed<'de> for Seed {
    type Value = JsonValue;
    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ValueVisitor)
    }
}

struct ValueVisitor;
impl<'de> Visitor<'de> for ValueVisitor {
    type Value = JsonValue;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("one JSON value without duplicate object keys")
    }
    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }
    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }
    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(JsonValue::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(JsonValue::Number(value.into()))
    }
    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(JsonValue::Number(value.into()))
    }
    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(JsonValue::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }
    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(JsonValue::String(value.to_owned()))
    }
    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(JsonValue::String(value))
    }
    fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(Seed)? {
            values.push(value);
        }
        Ok(JsonValue::Array(values))
    }
    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut seen = HashSet::new();
        let mut values = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(A::Error::custom(format!("duplicate JSON object key {key}")));
            }
            let value = map.next_value_seed(Seed)?;
            values.push((key, value));
        }
        Ok(JsonValue::Object(values))
    }
}

pub(crate) fn parse(input: &[u8], field: &str) -> Result<JsonValue> {
    let mut deserializer = serde_json::Deserializer::from_slice(input);
    let value = Seed.deserialize(&mut deserializer).map_err(|source| {
        let message = source.to_string();
        let kind = if message.contains("duplicate JSON object key") {
            ProtocolErrorKind::DuplicateJsonKey
        } else {
            ProtocolErrorKind::InvalidJson
        };
        error(kind, format_args!("{field} is invalid JSON"))
    })?;
    deserializer.end().map_err(|_| {
        error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} has trailing JSON input"),
        )
    })?;
    Ok(value)
}

fn utf16_cmp(left: &str, right: &str) -> Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

pub(crate) fn canonicalize(value: &JsonValue) -> Vec<u8> {
    let mut out = Vec::new();
    write_canonical(value, &mut out);
    out
}

pub(crate) fn serialize_in_order(value: &JsonValue) -> Vec<u8> {
    let mut out = Vec::new();
    write_in_order(value, &mut out);
    out
}

fn write_in_order(value: &JsonValue, out: &mut Vec<u8>) {
    match value {
        JsonValue::Object(values) => {
            out.push(b'{');
            for (index, (key, value)) in values.iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                out.extend_from_slice(
                    serde_json::to_string(key)
                        .expect("key serialization cannot fail")
                        .as_bytes(),
                );
                out.push(b':');
                write_in_order(value, out);
            }
            out.push(b'}');
        }
        JsonValue::Array(values) => {
            out.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                write_in_order(value, out);
            }
            out.push(b']');
        }
        _ => write_canonical(value, out),
    }
}

pub(crate) fn ordered_object(
    value: &JsonValue,
    field: &str,
    names: &[&str],
    nested: impl Fn(&str, &JsonValue) -> Result<JsonValue>,
) -> Result<JsonValue> {
    let values = closed(value, field, names)?;
    let mut out = Vec::with_capacity(names.len());
    for (name, value) in names.iter().zip(values) {
        out.push(((*name).to_owned(), nested(name, value)?));
    }
    Ok(JsonValue::Object(out))
}

fn write_canonical(value: &JsonValue, out: &mut Vec<u8>) {
    match value {
        JsonValue::Null => out.extend_from_slice(b"null"),
        JsonValue::Bool(value) => out.extend_from_slice(if *value { b"true" } else { b"false" }),
        JsonValue::Number(value) => write_number(value, out),
        JsonValue::String(value) => out.extend_from_slice(
            serde_json::to_string(value)
                .expect("string serialization cannot fail")
                .as_bytes(),
        ),
        JsonValue::Array(values) => {
            out.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                write_canonical(value, out);
            }
            out.push(b']');
        }
        JsonValue::Object(values) => {
            let mut sorted: Vec<_> = values.iter().collect();
            sorted.sort_by(|left, right| utf16_cmp(&left.0, &right.0));
            out.push(b'{');
            for (index, (key, value)) in sorted.into_iter().enumerate() {
                if index != 0 {
                    out.push(b',');
                }
                out.extend_from_slice(
                    serde_json::to_string(key)
                        .expect("key serialization cannot fail")
                        .as_bytes(),
                );
                out.push(b':');
                write_canonical(value, out);
            }
            out.push(b'}');
        }
    }
}

// RFC 8785 delegates JSON number serialization to ECMAScript's finite
// binary64 NumberToString spelling. Preserve exact JSON integers for the
// protocol's full-width u64 fields and use the ECMAScript Ryū implementation
// for parsed floating-point values.
fn write_number(value: &Number, out: &mut Vec<u8>) {
    if let Some(value) = value.as_i64() {
        out.extend_from_slice(value.to_string().as_bytes());
        return;
    }
    if let Some(value) = value.as_u64() {
        out.extend_from_slice(value.to_string().as_bytes());
        return;
    }
    let value = value
        .as_f64()
        .expect("the parser rejects non-finite JSON numbers");
    let mut buffer = ryu_js::Buffer::new();
    out.extend_from_slice(buffer.format(value).as_bytes());
}

pub(crate) fn object<'a>(value: &'a JsonValue, field: &str) -> Result<&'a [(String, JsonValue)]> {
    match value {
        JsonValue::Object(value) => Ok(value),
        _ => Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} must be an object"),
        )),
    }
}

pub(crate) fn array<'a>(value: &'a JsonValue, field: &str) -> Result<&'a [JsonValue]> {
    match value {
        JsonValue::Array(value) => Ok(value),
        _ => Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} must be an array"),
        )),
    }
}

pub(crate) fn string<'a>(value: &'a JsonValue, field: &str) -> Result<&'a str> {
    match value {
        JsonValue::String(value) => Ok(value),
        _ => Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} must be a string"),
        )),
    }
}

pub(crate) fn nullable_string<'a>(value: &'a JsonValue, field: &str) -> Result<Option<&'a str>> {
    match value {
        JsonValue::Null => Ok(None),
        JsonValue::String(value) => Ok(Some(value)),
        _ => Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} must be a string or null"),
        )),
    }
}

pub(crate) fn u64(value: &JsonValue, field: &str) -> Result<u64> {
    match value {
        JsonValue::Number(value) => value
            .as_u64()
            .or_else(|| {
                value.as_f64().and_then(|value| {
                    (value.is_finite()
                        && value.fract() == 0.0
                        && (0.0..18_446_744_073_709_551_616.0).contains(&value))
                    .then_some(value as u64)
                })
            })
            .ok_or_else(|| {
                error(
                    ProtocolErrorKind::InvalidJson,
                    format_args!("{field} must be a u64 integer"),
                )
            }),
        _ => Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} must be a u64 integer"),
        )),
    }
}

pub(crate) fn i64(value: &JsonValue, field: &str) -> Result<i64> {
    match value {
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| {
                value.as_f64().and_then(|value| {
                    (value.is_finite()
                        && value.fract() == 0.0
                        && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0)
                            .contains(&value))
                    .then_some(value as i64)
                })
            })
            .ok_or_else(|| {
                error(
                    ProtocolErrorKind::InvalidJson,
                    format_args!("{field} must be an i64 integer"),
                )
            }),
        _ => Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} must be an i64 integer"),
        )),
    }
}

pub(crate) fn finite_number(value: &JsonValue, field: &str) -> Result<f64> {
    match value {
        JsonValue::Number(value) => value.as_f64().filter(|v| v.is_finite()).ok_or_else(|| {
            error(
                ProtocolErrorKind::InvalidJson,
                format_args!("{field} must be a finite binary64 number"),
            )
        }),
        _ => Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} must be a number"),
        )),
    }
}

pub(crate) fn closed<'a>(
    value: &'a JsonValue,
    field: &str,
    names: &[&str],
) -> Result<Vec<&'a JsonValue>> {
    let entries = object(value, field)?;
    if entries.len() != names.len() {
        return Err(error(
            ProtocolErrorKind::InvalidJson,
            format_args!("{field} has missing or unknown members"),
        ));
    }
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let value = entries
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value))
            .ok_or_else(|| {
                error(
                    ProtocolErrorKind::InvalidJson,
                    format_args!("{field}.{name} is missing"),
                )
            })?;
        out.push(value);
    }
    Ok(out)
}

pub(crate) fn member<'a>(value: &'a JsonValue, field: &str, name: &str) -> Result<&'a JsonValue> {
    object(value, field)?
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
        .ok_or_else(|| {
            error(
                ProtocolErrorKind::InvalidJson,
                format_args!("{field}.{name} is missing"),
            )
        })
}

pub(crate) fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

#[cfg(test)]
mod tests {
    use super::{canonicalize, parse};

    #[test]
    fn rfc_8785_number_and_utf16_member_order_oracles() {
        let value = parse(
            br#"{"numbers":[1e20,1e21,1e-6,1e-7,-0.0,1.0,333333333.33333329],"\u20ac":1,"\ud83d\ude00":2,"\ufb33":3}"#,
            "rfc8785",
        )
        .unwrap();
        assert_eq!(
            canonicalize(&value),
            r#"{"numbers":[100000000000000000000,1e+21,0.000001,1e-7,0,1,333333333.3333333],"€":1,"😀":2,"דּ":3}"#.as_bytes()
        );
    }
}
