//! `cdcEncode` from `types/encoding_helper.go`.
//!
//! Empty strings and empty byte slices become an empty Merkle leaf. Other values
//! are `google.protobuf` wrapper messages. Integers are encoded even when zero.

use prost::Message;

#[derive(Clone, PartialEq, ::prost::Message)]
struct StringValue {
    #[prost(string, tag = "1")]
    value: ::prost::alloc::string::String,
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct Int64Value {
    #[prost(int64, tag = "1")]
    value: i64,
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct BytesValue {
    #[prost(bytes = "vec", tag = "1")]
    value: ::prost::alloc::vec::Vec<u8>,
}

#[must_use]
pub fn cdc_encode_string(value: &str) -> Vec<u8> {
    if value.is_empty() {
        return Vec::new();
    }
    StringValue {
        value: value.to_owned(),
    }
    .encode_to_vec()
}

#[must_use]
pub fn cdc_encode_i64(value: i64) -> Vec<u8> {
    Int64Value { value }.encode_to_vec()
}

#[must_use]
pub fn cdc_encode_bytes(value: &[u8]) -> Vec<u8> {
    if value.is_empty() {
        return Vec::new();
    }
    BytesValue {
        value: value.to_vec(),
    }
    .encode_to_vec()
}
