//! The Vesper build: the same API, with every decision forwarded to
//! `vesper/src/lib.ves` through the generated binding (`port/binding.rs`).
//! What stays here is what the export ABI cannot carry yet (names, error
//! strings, generic payloads, the backend trait) and the conversion between
//! the public types and the Vesper records in [`ffi`].

// Generated and committed, kept outside src/ so formatters never reflow it.
#[rustfmt::skip]
#[path = "../../port/binding.rs"]
pub(crate) mod binding;
pub(crate) mod d3d12;
pub(crate) mod ffi;
pub(crate) mod graph;
pub(crate) mod lifecycle;
pub(crate) mod multi_backend;
pub(crate) mod plan;
pub(crate) mod recording;
