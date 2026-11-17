//! The Rust reference implementation. It is the default build until the
//! Vesper port (`vesper-port`) replaces it (port wave P1 lane 14, #911).

pub(crate) mod connection;
pub(crate) mod decoder;
pub(crate) mod response;
pub(crate) mod router;
pub(crate) mod target;
