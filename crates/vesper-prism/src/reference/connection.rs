//! The Rust reference build of the connection state machine (see
//! `crate::connection::Connection`).

use std::{collections::VecDeque, io};

use crate::{
    Decoder, Error, RequestHead,
    connection::{
        Abandoned, ConnectionError, ConnectionLimits, ConnectionState, Inbound, RequestEvent,
        RequestId, Response, Shutdown, SubmitError, SubmitErrorKind, UnsentOutput,
        next_connection_id,
    },
    response::{
        ConnectionPolicy, EncodedResponse, ResponseBuilder, ResponseContext, ResponseError,
        ResponseLimits,
    },
};

enum SlotState {
    Awaiting,
    Ready(EncodedResponse),
    /// Parse failure marker, optionally with a generated error response.
    Failure(Option<EncodedResponse>),
}
struct Slot {
    id: Option<RequestId>,
    head: bool,
    request_close: bool,
    state: SlotState,
}
struct Chunk {
    id: Option<RequestId>,
    bytes: Vec<u8>,
}

pub struct Connection {
    connection: u64,
    limits: ConnectionLimits,
    decoder: Decoder,
    /// Sequence of `slots[0]`; equal to `next_sequence` when empty.
    base_sequence: u64,
    next_sequence: u64,
    slots: VecDeque<Slot>,
    outbound: VecDeque<Chunk>,
    cursor: usize,
    /// Encoded bytes in Ready slots plus unwritten outbound bytes.
    retained: usize,
    /// The connection closes after this sequence's response.
    close_after: Option<u64>,
    stop_issuing: bool,
    input_ended: bool,
    close_promoted: bool,
    failure: Option<Error>,
    failure_reported: bool,
    abandoned: Vec<Abandoned>,
    discarded_input: usize,
}

impl Connection {
    pub fn new(limits: ConnectionLimits) -> Result<Self, ConnectionError> {
        if limits.max_in_flight == 0 || limits.max_outbound_bytes < limits.response.max_total_bytes
        {
            return Err(ConnectionError::InvalidLimits);
        }
        let decoder = Decoder::new(limits.request).map_err(ConnectionError::Decoder)?;
        let mut slots = VecDeque::new();
        slots
            .try_reserve_exact(limits.max_in_flight)
            .map_err(|_| ConnectionError::Decoder(Error::AllocationFailed))?;
        Ok(Self {
            connection: next_connection_id(),
            limits,
            decoder,
            base_sequence: 0,
            next_sequence: 0,
            slots,
            outbound: VecDeque::new(),
            cursor: 0,
            retained: 0,
            close_after: None,
            stop_issuing: false,
            input_ended: false,
            close_promoted: false,
            failure: None,
            failure_reported: false,
            abandoned: Vec::new(),
            discarded_input: 0,
        })
    }
    pub fn limits(&self) -> ConnectionLimits {
        self.limits
    }

    /// Accept a prefix of `bytes` that fits the bounded input queue and return
    /// its length; the caller retains the rest. `Ok(0)` with non-empty input
    /// is backpressure: poll requests (and submit/write responses) first.
    /// Once no further requests can be issued, input is counted and discarded.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<usize, ConnectionError> {
        if self.input_ended {
            return Err(ConnectionError::InputEnded);
        }
        if self.stop_issuing {
            self.discarded_input = self.discarded_input.saturating_add(bytes.len());
            return Ok(bytes.len());
        }
        let capacity = self.limits.request.max_buffered_bytes - self.decoder.buffered_bytes();
        let accepted = bytes.len().min(capacity);
        self.decoder
            .push(&bytes[..accepted])
            .map_err(ConnectionError::Decoder)?;
        Ok(accepted)
    }
    /// Transport EOF from the peer. Complete queued requests can still drain.
    pub fn end_input(&mut self) {
        self.input_ended = true;
        self.decoder.end_input();
    }

    /// Issue at most one request. Performs one bounded decoder step.
    pub fn poll_request(&mut self) -> RequestEvent {
        if let Some(error) = self.failure
            && !self.failure_reported
        {
            self.failure_reported = true;
            return RequestEvent::Failed(error);
        }
        if self.stop_issuing {
            return RequestEvent::NoMoreRequests;
        }
        if self.slots.len() >= self.limits.max_in_flight {
            return RequestEvent::Backpressure;
        }
        match self.decoder.next_request() {
            Ok(Some(decoded)) => {
                let sequence = self.next_sequence;
                self.next_sequence += 1;
                let id = RequestId {
                    connection: self.connection,
                    sequence,
                };
                let head = &decoded.request.head;
                let request_close = wants_close(head);
                self.slots.push_back(Slot {
                    id: Some(id),
                    head: head.method == "HEAD",
                    request_close,
                    state: SlotState::Awaiting,
                });
                if request_close {
                    self.close_after = Some(sequence);
                    self.stop_issuing = true;
                }
                RequestEvent::Request(Inbound {
                    id,
                    request: decoded.request,
                })
            }
            Ok(None) if self.input_ended => {
                self.stop_issuing = true;
                if !self.slots.is_empty() {
                    self.close_after = Some(self.next_sequence - 1);
                }
                RequestEvent::NoMoreRequests
            }
            Ok(None) => RequestEvent::NeedInput,
            Err(error) => {
                self.stop_issuing = true;
                self.failure = Some(error);
                self.failure_reported = true;
                let sequence = self.next_sequence;
                self.next_sequence += 1;
                let response = error_status(error).and_then(|status| {
                    ResponseBuilder::new(
                        status,
                        ResponseContext::Ordinary,
                        ConnectionPolicy::Close,
                        0,
                        self.limits.response,
                    )
                    .and_then(|builder| builder.finish(b""))
                    .ok()
                });
                if let Some(response) = &response {
                    self.retained += response.len();
                }
                self.slots.push_back(Slot {
                    id: None,
                    head: false,
                    request_close: true,
                    state: SlotState::Failure(response),
                });
                self.close_after = Some(sequence);
                self.promote();
                RequestEvent::Failed(error)
            }
        }
    }

    /// Associate a response with its request. On any rejection the response is
    /// returned and the wire queue is unchanged.
    pub fn submit(&mut self, id: RequestId, response: Response) -> Result<(), SubmitError> {
        let reject = |kind, response| Err(SubmitError { kind, response });
        if id.connection != self.connection {
            return reject(SubmitErrorKind::Foreign, response);
        }
        if id.sequence >= self.next_sequence {
            return reject(SubmitErrorKind::Unknown, response);
        }
        if self.close_after.is_some_and(|last| id.sequence > last) {
            return reject(SubmitErrorKind::Abandoned, response);
        }
        if id.sequence < self.base_sequence {
            return reject(SubmitErrorKind::Stale, response);
        }
        let index = (id.sequence - self.base_sequence) as usize;
        let slot = &self.slots[index];
        if !matches!(slot.state, SlotState::Awaiting) {
            return reject(SubmitErrorKind::Duplicate, response);
        }
        let close = response.close || slot.request_close || self.close_after == Some(id.sequence);
        let context = if slot.head {
            ResponseContext::Head
        } else {
            ResponseContext::Ordinary
        };
        let encoded = match encode(&response, context, close, self.limits.response) {
            Ok(encoded) => encoded,
            Err(error) => return reject(SubmitErrorKind::Encode(error), response),
        };
        // An out-of-order response cannot be written until earlier ones are,
        // so it must leave room for one maximal response at the front.
        // Otherwise later completions could exhaust the budget and block the
        // earliest submission forever. Unsent wire bytes drain by writing, so
        // retained Ready bytes stay at most max_outbound - max_total.
        let reserve = if index == 0 {
            0
        } else {
            self.limits.response.max_total_bytes
        };
        if self
            .retained
            .saturating_add(encoded.len())
            .saturating_add(reserve)
            > self.limits.max_outbound_bytes
        {
            return reject(SubmitErrorKind::OutboundLimit, response);
        }
        self.retained += encoded.len();
        self.slots[index].state = SlotState::Ready(encoded);
        if close {
            self.close_at(id.sequence);
        }
        self.promote();
        Ok(())
    }

    /// Responses cut off by a closing response since the last call.
    pub fn take_abandoned(&mut self) -> Vec<Abandoned> {
        std::mem::take(&mut self.abandoned)
    }

    /// The unwritten remainder of the front wire response; empty if none.
    pub fn pending_output(&self) -> &[u8] {
        self.outbound
            .front()
            .map_or(&[][..], |chunk| &chunk.bytes[self.cursor..])
    }
    /// Total unwritten bytes already in wire order.
    pub fn pending_output_len(&self) -> usize {
        self.outbound
            .iter()
            .map(|chunk| chunk.bytes.len())
            .sum::<usize>()
            - self.cursor
    }
    /// Record that the transport accepted `written` bytes of `pending_output`.
    /// Overruns are rejected without changing state.
    pub fn advance_output(&mut self, written: usize) -> Result<(), ConnectionError> {
        let pending = self.pending_output().len();
        if written > pending {
            return Err(ConnectionError::OutputOverrun {
                pending,
                advanced: written,
            });
        }
        self.cursor += written;
        self.retained -= written;
        if pending > 0 && written == pending {
            self.outbound.pop_front();
            self.cursor = 0;
        }
        Ok(())
    }
    /// One `write` call with the pending slice. A short write advances only by
    /// the reported count; an error or zero-length write advances nothing.
    pub fn write_to(&mut self, writer: &mut impl io::Write) -> io::Result<usize> {
        let pending = self.pending_output();
        if pending.is_empty() {
            return Ok(0);
        }
        let written = writer.write(pending)?;
        if written == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        self.advance_output(written)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(written)
    }

    pub fn state(&self) -> ConnectionState {
        if !self.stop_issuing {
            ConnectionState::Open
        } else if self.slots.is_empty() && self.outbound.is_empty() {
            ConnectionState::Closed
        } else {
            ConnectionState::Closing
        }
    }
    pub fn in_flight(&self) -> usize {
        self.slots.len()
    }
    pub fn retained_outbound_bytes(&self) -> usize {
        self.retained
    }
    pub fn buffered_input(&self) -> usize {
        self.decoder.buffered_bytes()
    }
    pub fn failure(&self) -> Option<Error> {
        self.failure
    }

    /// Consume the connection, returning every retained owner and byte.
    pub fn shutdown(mut self) -> Shutdown {
        let mut unsent = Vec::new();
        for (index, chunk) in self.outbound.drain(..).enumerate() {
            unsent.push(UnsentOutput {
                id: chunk.id,
                bytes: chunk.bytes,
                written: if index == 0 { self.cursor } else { 0 },
            });
        }
        let mut blocked = Vec::new();
        let mut awaiting = Vec::new();
        for slot in self.slots.drain(..) {
            match (slot.id, slot.state) {
                (Some(id), SlotState::Ready(response)) => blocked.push((id, response)),
                (Some(id), SlotState::Awaiting) => awaiting.push(id),
                (None, SlotState::Failure(Some(response))) => unsent.push(UnsentOutput {
                    id: None,
                    bytes: response.into_bytes(),
                    written: 0,
                }),
                _ => {}
            }
        }
        let (front, back) = self.decoder.buffered_slices();
        Shutdown {
            unsent,
            blocked,
            awaiting,
            abandoned: self.abandoned,
            unprocessed_input: [front, back].concat(),
            discarded_input: self.discarded_input,
            failure: self.failure,
        }
    }

    /// An id of this connection that may never have been issued.
    #[cfg(test)]
    pub(crate) fn request_id(&self, sequence: u64) -> RequestId {
        RequestId {
            connection: self.connection,
            sequence,
        }
    }

    /// Truncate every slot after `sequence`, retaining their owners.
    fn close_at(&mut self, sequence: u64) {
        self.close_after = Some(sequence);
        self.stop_issuing = true;
        let keep = (sequence - self.base_sequence + 1) as usize;
        for slot in self.slots.drain(keep.min(self.slots.len())..) {
            let response = match slot.state {
                SlotState::Ready(response) | SlotState::Failure(Some(response)) => {
                    self.retained -= response.len();
                    Some(response)
                }
                SlotState::Awaiting | SlotState::Failure(None) => None,
            };
            // A generated error response for a later parse failure is simply
            // superseded: the connection closes before that position.
            if let Some(id) = slot.id {
                self.abandoned.push(Abandoned { id, response });
            }
        }
    }
    /// Move completed responses at the front, in order, to the wire queue.
    fn promote(&mut self) {
        while !self.close_promoted {
            let Some(front) = self.slots.front() else {
                return;
            };
            if matches!(front.state, SlotState::Awaiting) {
                return;
            }
            let slot = self.slots.pop_front().expect("front exists");
            let sequence = self.base_sequence;
            self.base_sequence += 1;
            let bytes = match slot.state {
                SlotState::Ready(response) => Some(response.into_bytes()),
                SlotState::Failure(response) => response.map(EncodedResponse::into_bytes),
                SlotState::Awaiting => unreachable!("checked above"),
            };
            if let Some(bytes) = bytes {
                self.outbound.push_back(Chunk { id: slot.id, bytes });
            }
            if self.close_after == Some(sequence) {
                self.close_promoted = true;
            }
        }
    }
}

fn wants_close(head: &RequestHead) -> bool {
    head.headers("connection").any(|value| {
        value
            .split(|byte| *byte == b',')
            .map(crate::reference::decoder::trim_ows)
            .any(|option| option.eq_ignore_ascii_case(b"close"))
    })
}
fn encode(
    response: &Response,
    context: ResponseContext,
    close: bool,
    limits: ResponseLimits,
) -> Result<EncodedResponse, ResponseError> {
    let policy = if close {
        ConnectionPolicy::Close
    } else {
        ConnectionPolicy::KeepAlive
    };
    let mut builder = ResponseBuilder::new(
        response.status,
        context,
        policy,
        response.body.len(),
        limits,
    )?;
    for (name, value) in &response.headers {
        builder = builder.header(name, value)?;
    }
    builder.finish(&response.body)
}
/// Generated status for a decoder failure; `None` means close silently.
fn error_status(error: Error) -> Option<u16> {
    Some(match error {
        Error::UnexpectedEof | Error::InputEnded | Error::Poisoned => return None,
        Error::AllocationFailed | Error::InvalidLimits => 500,
        Error::BodyLimit | Error::BufferedLimit => 413,
        Error::HeadLimit | Error::HeaderCountLimit => 431,
        Error::UnsupportedVersion => 505,
        Error::UnsupportedExpectation => 417,
        Error::UnsupportedTransferCoding | Error::UnsupportedUpgrade | Error::UnsupportedMethod => {
            501
        }
        _ => 400,
    })
}
