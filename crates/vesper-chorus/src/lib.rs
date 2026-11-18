#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "vesper-port"), forbid(unsafe_code))]
#![cfg_attr(feature = "vesper-port", deny(unsafe_code))]
// Under `vesper-port` the Rust bodies of the ported entry points are not
// called (the Rust build stays the default and is never deleted).
#![cfg_attr(feature = "vesper-port", allow(dead_code))]

pub mod check;
pub mod chor;
pub mod durable;
pub mod parse;
#[cfg(feature = "vesper-port")]
mod port;
pub mod project;
pub mod raft;
pub mod rdma;
pub mod replog;
pub mod session;
pub mod simnet;
pub mod warpnet;

pub use chor::{Choreography, GBranch, GlobalChor, RoleId, WellFormedError};
pub use parse::{ParseError, ParseErrorKind, parse_choreography};
pub use project::{LBranch, LocalType, ProjectionError, are_dual, project, project_all};

/// Which build of the ported entry points (parsing, well-formedness,
/// projection, duality, exploration) this is: `"vesper"` under the
/// `vesper-port` feature (port wave P3b lane 6, #1068), else `"rust"`.
pub const IMPLEMENTATION: &str = if cfg!(feature = "vesper-port") {
    "vesper"
} else {
    "rust"
};

/// Live sequence allocations of the Vesper library (0 in the Rust build),
/// for leak tests.
pub fn port_live_allocations() -> i64 {
    #[cfg(feature = "vesper-port")]
    {
        port::live_allocations()
    }
    #[cfg(not(feature = "vesper-port"))]
    {
        0
    }
}
