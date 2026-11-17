//! Forwards [`crate::response::ResponseBuilder`] to the `vesper/response` package.

use std::fmt;

use crate::response::{
    ConnectionPolicy, EncodedResponse, ResponseContext, ResponseError, ResponseLimits,
};

/// The types at the library boundary (`--rust-types`).
pub(crate) mod abi {
    pub use crate::response::{ConnectionPolicy, ResponseContext, ResponseError, ResponseLimits};
}

#[allow(unsafe_code)]
#[rustfmt::skip]
#[path = "../../port/response.rs"]
mod binding;

/// Validated response plan. No arbitrary status reason phrase is accepted.
///
/// `representation_length` is checked against `finish`'s actual byte slice,
/// including for HEAD. HEAD then omits the wire payload while emitting that
/// verified length. Statuses 204 and 304 require an empty representation and
/// omit Content-Length; 205 requires empty content and emits Content-Length: 0.
///
/// ```
/// use vesper_prism::response::{ConnectionPolicy, ResponseBuilder, ResponseContext, ResponseLimits};
/// let response = ResponseBuilder::new(200, ResponseContext::Ordinary,
///     ConnectionPolicy::Close, 3, ResponseLimits::default())?
///     .header("Content-Type", b"application/octet-stream")?
///     .finish(&[0, 128, 255])?;
/// let wire = response.into_bytes();
/// assert!(wire.ends_with(&[0, 128, 255]));
/// # Ok::<(), vesper_prism::response::ResponseError>(())
/// ```
///
/// Finalization consumes the builder, preventing accidental reuse:
///
/// ```compile_fail
/// use vesper_prism::response::{ConnectionPolicy, ResponseBuilder, ResponseContext, ResponseLimits};
/// let builder = ResponseBuilder::new(200, ResponseContext::Ordinary,
///     ConnectionPolicy::Close, 0, ResponseLimits::default()).unwrap();
/// let first = builder.finish(b"");
/// let duplicate = builder.finish(b""); // builder was moved
/// ```
pub struct ResponseBuilder {
    plan: binding::ResponsePlan,
    connection: ConnectionPolicy,
}
impl fmt::Debug for ResponseBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plan = self.plan.as_ref();
        f.debug_struct("ResponseBuilder")
            .field("status", &plan.status())
            .field("connection", &self.connection)
            .field("representation_length", &plan.representation_length())
            .field("head_length", &plan.head_length())
            .field("header_count", &plan.header_count())
            .finish_non_exhaustive()
    }
}
impl ResponseBuilder {
    pub fn new(
        status: u16,
        context: ResponseContext,
        connection: ConnectionPolicy,
        representation_length: usize,
        limits: ResponseLimits,
    ) -> Result<Self, ResponseError> {
        let plan = binding::ResponsePlan::new(
            u64::from(status),
            context,
            connection,
            representation_length as u64,
            limits,
        )?;
        Ok(Self { plan, connection })
    }

    /// Append an ordinary field. Order and repetitions are preserved; names
    /// are emitted in ASCII lowercase. Framing/connection fields are reserved.
    /// Header values retain bytes, including obs-text, but forbid all controls.
    pub fn header(mut self, name: &str, value: &[u8]) -> Result<Self, ResponseError> {
        match self.plan.header(name.as_bytes(), value) {
            Some(error) => Err(error),
            None => Ok(self),
        }
    }

    /// Validate the exact representation and consume this builder into wire
    /// bytes. No partial output is returned on an error.
    pub fn finish(self, body: &[u8]) -> Result<EncodedResponse, ResponseError> {
        let bytes = self.plan.finish(body)?;
        Ok(EncodedResponse {
            bytes,
            connection: self.connection,
        })
    }
}
