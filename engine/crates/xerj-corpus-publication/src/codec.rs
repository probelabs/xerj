use crate::error::{error, ProtocolErrorKind, Result};

#[derive(Default)]
pub(crate) struct Encoder(Vec<u8>);

impl Encoder {
    pub(crate) fn domain(domain: &'static [u8]) -> Self {
        Self(domain.to_vec())
    }
    pub(crate) fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    pub(crate) fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    pub(crate) fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.0.extend_from_slice(value.as_bytes());
    }
    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.u64(value.len() as u64);
        self.0.extend_from_slice(value);
    }
    pub(crate) fn raw(&mut self, value: &[u8]) {
        self.0.extend_from_slice(value);
    }
    pub(crate) fn array_len(&mut self, value: usize) {
        self.u64(value as u64);
    }
    pub(crate) fn finish(self) -> Vec<u8> {
        self.0
    }
}

pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub(crate) fn domain(&mut self, domain: &[u8]) -> Result<()> {
        if self.take(domain.len())? != domain {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "binary domain does not match",
            ));
        }
        Ok(())
    }
    pub(crate) fn u32(&mut self, field: &str) -> Result<u32> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("length checked");
        let value = u32::from_be_bytes(bytes);
        let _ = field;
        Ok(value)
    }
    pub(crate) fn u64(&mut self, field: &str) -> Result<u64> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("length checked");
        let value = u64::from_be_bytes(bytes);
        let _ = field;
        Ok(value)
    }
    pub(crate) fn len(&mut self, field: &str) -> Result<usize> {
        let value = usize::try_from(self.u64(field)?).map_err(|_| {
            error(
                ProtocolErrorKind::BoundsExceeded,
                format_args!("{field} length is not representable"),
            )
        })?;
        if value > self.bytes.len() {
            return Err(error(
                ProtocolErrorKind::BoundsExceeded,
                format_args!("{field} length exceeds its enclosing input"),
            ));
        }
        Ok(value)
    }
    pub(crate) fn string(&mut self, field: &str) -> Result<&'a str> {
        let len = self.len(field)?;
        std::str::from_utf8(self.take(len)?).map_err(|_| {
            error(
                ProtocolErrorKind::NonCanonicalEncoding,
                format_args!("{field} is not UTF-8"),
            )
        })
    }
    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(len).ok_or_else(|| {
            error(
                ProtocolErrorKind::ArithmeticOverflow,
                "binary cursor overflow",
            )
        })?;
        if end > self.bytes.len() {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "truncated binary encoding",
            ));
        }
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }
    pub(crate) fn finish(self) -> Result<()> {
        if self.offset != self.bytes.len() {
            return Err(error(
                ProtocolErrorKind::NonCanonicalEncoding,
                "trailing bytes in binary encoding",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn checked_len_from_u128_for_test(value: u128) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        error(
            ProtocolErrorKind::ArithmeticOverflow,
            "synthetic length exceeds u64",
        )
    })
}

pub(crate) fn checked_add(values: impl IntoIterator<Item = u64>, field: &str) -> Result<u64> {
    values.into_iter().try_fold(0u64, |acc, value| {
        acc.checked_add(value).ok_or_else(|| {
            error(
                ProtocolErrorKind::ArithmeticOverflow,
                format_args!("{field} addition overflow"),
            )
        })
    })
}

pub(crate) fn checked_mul(left: u64, right: u64, field: &str) -> Result<u64> {
    left.checked_mul(right).ok_or_else(|| {
        error(
            ProtocolErrorKind::ArithmeticOverflow,
            format_args!("{field} multiplication overflow"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_arithmetic_matrix() {
        let parse_errors = [
            (
                "synthetic length exceeds u64",
                checked_len_from_u128_for_test(u64::MAX as u128 + 1),
            ),
            (
                "mapping charge addition overflow",
                checked_add([u64::MAX, 1], "mapping charge"),
            ),
            (
                "artifact charge addition overflow",
                checked_add([u64::MAX, 1], "artifact charge"),
            ),
            (
                "operation charge addition overflow",
                checked_add([u64::MAX, 1], "operation charge"),
            ),
            (
                "operation charge multiplication overflow",
                checked_mul(u64::MAX / 64 + 1, 64, "operation charge"),
            ),
            (
                "resource charge multiplication overflow",
                checked_mul(u64::MAX / 4096 + 1, 4096, "resource charge"),
            ),
            (
                "stage charge addition overflow",
                checked_add([u64::MAX, 1, 0, 0], "stage charge"),
            ),
            (
                "stage charge addition overflow",
                checked_add([u64::MAX - 1, 1, 1, 0], "stage charge"),
            ),
            (
                "stage charge addition overflow",
                checked_add([u64::MAX - 2, 1, 1, 1], "stage charge"),
            ),
        ];
        assert_eq!(parse_errors.len(), 9);
        for (expected_message, result) in parse_errors {
            let error = result.unwrap_err();
            assert_eq!(error.kind(), ProtocolErrorKind::ArithmeticOverflow);
            assert_eq!(error.to_string(), expected_message);
        }

        let u64_max_successes = [checked_len_from_u128_for_test(u64::MAX as u128).unwrap()];
        assert_eq!(u64_max_successes, [u64::MAX]);
    }

    #[test]
    fn checked_length_accepts_exact_u64_max_and_rejects_max_plus_one() {
        assert_eq!(
            checked_len_from_u128_for_test(u64::MAX as u128).unwrap(),
            u64::MAX
        );
        let length = checked_len_from_u128_for_test(u64::MAX as u128 + 1).unwrap_err();
        assert_eq!(length.kind(), ProtocolErrorKind::ArithmeticOverflow);
        assert_eq!(length.to_string(), "synthetic length exceeds u64");
    }

    #[test]
    fn checked_arithmetic_reports_the_named_operation() {
        let addition = checked_add([u64::MAX, 1], "test charge").unwrap_err();
        assert_eq!(addition.kind(), ProtocolErrorKind::ArithmeticOverflow);
        assert_eq!(addition.to_string(), "test charge addition overflow");

        let multiplication = checked_mul(u64::MAX, 2, "test charge").unwrap_err();
        assert_eq!(multiplication.kind(), ProtocolErrorKind::ArithmeticOverflow);
        assert_eq!(
            multiplication.to_string(),
            "test charge multiplication overflow"
        );
    }
}
