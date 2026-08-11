use std::fmt;

const MAX_DETAIL: usize = 4096;

#[non_exhaustive]
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ProtocolErrorKind {
    InvalidScalar,
    InvalidJson,
    DuplicateJsonKey,
    InvalidRenderedIdentity,
    InvalidVersion,
    NonCanonicalEncoding,
    DuplicateTuple,
    CrossFieldMismatch,
    BoundsExceeded,
    ArithmeticOverflow,
}

pub struct ProtocolError {
    kind: ProtocolErrorKind,
    detail: Box<str>,
}

impl ProtocolError {
    pub(crate) fn new(kind: ProtocolErrorKind, detail: impl fmt::Display) -> Self {
        let mut detail = detail.to_string();
        if detail.len() > MAX_DETAIL {
            let mut end = MAX_DETAIL;
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            detail.truncate(end);
        }
        Self {
            kind,
            detail: detail.into_boxed_str(),
        }
    }

    pub fn kind(&self) -> ProtocolErrorKind {
        self.kind
    }
}

impl fmt::Debug for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolError")
            .field("kind", &self.kind)
            .field("detail", &self.detail)
            .finish()
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for ProtocolError {}

pub(crate) type Result<T, E = ProtocolError> = std::result::Result<T, E>;

pub(crate) fn error(kind: ProtocolErrorKind, detail: impl fmt::Display) -> ProtocolError {
    ProtocolError::new(kind, detail)
}
