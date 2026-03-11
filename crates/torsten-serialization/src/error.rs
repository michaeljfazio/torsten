use thiserror::Error;

#[derive(Error, Debug)]
pub enum SerializationError {
    #[error("CBOR encoding error: {0}")]
    CborEncode(String),
    #[error("CBOR decoding error: {0}")]
    CborDecode(String),
    #[error("Invalid data: {0}")]
    InvalidData(String),
    #[error("Unexpected CBOR tag: {0}")]
    UnexpectedTag(u64),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Invalid length: expected {expected}, got {got}")]
    InvalidLength { expected: usize, got: usize },
}

impl From<minicbor::decode::Error> for SerializationError {
    fn from(e: minicbor::decode::Error) -> Self {
        SerializationError::CborDecode(e.to_string())
    }
}
