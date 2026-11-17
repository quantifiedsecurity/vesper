//! The Vesper build (port wave P1 lane 13, #910): the public API forwards
//! to vesper/aura.ves through the committed binding in port/binding.rs.
//! These wrappers keep the Rust-only parts: the thread-confined owners
//! (`UiThread` checks, `PhantomData<Rc<()>>`), the generic `Backend`
//! adaptor that `Session<B>` applies a plan to, and the conversions in
//! [`codec`].

// Generated and committed, outside src/ (see the porting recipe).
#[rustfmt::skip]
#[path = "../../port/binding.rs"]
mod binding;

mod codec;
mod recording;
mod session;
mod sim_aura;

pub use recording::RecordingBackend;
pub use session::{CloseDecision, PreparedUpdate, Session};
pub use sim_aura::{SimAura, audit_view_tree, encode_view_tree};
