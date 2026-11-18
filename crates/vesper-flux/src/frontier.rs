use crate::{Error, identity};

/// An affine Rust owner of a live watermark hold. Private origin/generation
/// prevents safe callers forging, copying, or transferring tickets across trackers.
/// Dropping/forgetting this value DOES NOT retire its hold: progress can stall.
#[must_use = "explicitly advance or retire the ticket; dropping leaves a live hold"]
#[derive(Debug)]
pub struct FrontierTicket {
    origin: u64,
    slot: usize,
    generation: u64,
    timestamp: i64,
}
impl FrontierTicket {
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }
}
#[derive(Debug)]
pub struct TicketError {
    pub error: Error,
    pub ticket: FrontierTicket,
}
#[cfg(not(feature = "vesper-port"))]
#[derive(Debug)]
struct Slot {
    generation: u64,
    hold: Option<i64>,
}
#[cfg(not(feature = "vesper-port"))]
#[derive(Debug)]
pub struct Frontier {
    origin: u64,
    slots: Vec<Slot>,
    capacity: usize,
    watermark: i64,
}
/// The Vesper build keeps the hold table as three columns the port's
/// `frontier_*` functions update in place: slot i is `gens[i]`, `holds[i]`
/// and `live[i]` (1 while the slot holds `holds[i]`).
#[cfg(feature = "vesper-port")]
#[derive(Debug)]
pub struct Frontier {
    origin: u64,
    gens: Vec<u64>,
    holds: Vec<i64>,
    live: Vec<u8>,
    capacity: usize,
    watermark: i64,
}
impl Frontier {
    #[cfg(not(feature = "vesper-port"))]
    pub fn new(initial: i64, capacity: usize) -> Result<Self, Error> {
        Ok(Self {
            origin: identity()?,
            slots: Vec::new(),
            capacity,
            watermark: initial,
        })
    }
    #[cfg(feature = "vesper-port")]
    pub fn new(initial: i64, capacity: usize) -> Result<Self, Error> {
        Ok(Self {
            origin: identity()?,
            gens: Vec::new(),
            holds: Vec::new(),
            live: Vec::new(),
            capacity,
            watermark: initial,
        })
    }
    /// Published conservative lower bound. Empty trackers retain their last bound;
    /// this API does not infer infinity or completed input from owner disappearance.
    pub fn watermark(&self) -> i64 {
        self.watermark
    }
    #[cfg(not(feature = "vesper-port"))]
    pub fn live_holds(&self) -> usize {
        self.slots.iter().filter(|s| s.hold.is_some()).count()
    }
    #[cfg(not(feature = "vesper-port"))]
    pub fn acquire(&mut self, timestamp: i64) -> Result<FrontierTicket, Error> {
        if timestamp < self.watermark {
            return Err(Error::WatermarkRegression);
        }
        let index = if let Some(i) = self
            .slots
            .iter()
            .position(|s| s.hold.is_none() && s.generation < u64::MAX)
        {
            i
        } else {
            if self.slots.len() >= self.capacity {
                return Err(Error::Limit("frontier holds"));
            }
            self.slots.push(Slot {
                generation: 0,
                hold: None,
            });
            self.slots.len() - 1
        };
        let slot = &mut self.slots[index];
        slot.generation = slot
            .generation
            .checked_add(1)
            .ok_or(Error::IdentityExhausted)?;
        slot.hold = Some(timestamp);
        let ticket = FrontierTicket {
            origin: self.origin,
            slot: index,
            generation: slot.generation,
            timestamp,
        };
        self.publish();
        Ok(ticket)
    }
    /// Adds a separately owned equal hold. Backpressure returns the original owner.
    pub fn fork(
        &mut self,
        ticket: FrontierTicket,
    ) -> Result<(FrontierTicket, FrontierTicket), TicketError> {
        if let Err(error) = self.validate(&ticket) {
            return Err(TicketError { error, ticket });
        }
        match self.acquire(ticket.timestamp) {
            Ok(new) => Ok((ticket, new)),
            Err(error) => Err(TicketError { error, ticket }),
        }
    }
    #[cfg(not(feature = "vesper-port"))]
    fn validate(&self, ticket: &FrontierTicket) -> Result<(), Error> {
        if ticket.origin != self.origin {
            return Err(Error::ForeignTicket);
        }
        let slot = self.slots.get(ticket.slot).ok_or(Error::StaleTicket)?;
        if slot.generation != ticket.generation || slot.hold != Some(ticket.timestamp) {
            return Err(Error::StaleTicket);
        }
        Ok(())
    }
    #[cfg(not(feature = "vesper-port"))]
    pub fn advance(
        &mut self,
        mut ticket: FrontierTicket,
        next: i64,
    ) -> Result<FrontierTicket, TicketError> {
        let check = self.validate(&ticket).and_then(|()| {
            if next < ticket.timestamp {
                Err(Error::WatermarkRegression)
            } else {
                self.slots[ticket.slot]
                    .generation
                    .checked_add(1)
                    .ok_or(Error::IdentityExhausted)
            }
        });
        match check {
            Err(error) => Err(TicketError { error, ticket }),
            Ok(generation) => {
                let slot = &mut self.slots[ticket.slot];
                slot.generation = generation;
                slot.hold = Some(next);
                ticket.generation = generation;
                ticket.timestamp = next;
                self.publish();
                Ok(ticket)
            }
        }
    }
    #[cfg(not(feature = "vesper-port"))]
    pub fn retire(&mut self, ticket: FrontierTicket) -> Result<(), TicketError> {
        if let Err(error) = self.validate(&ticket) {
            return Err(TicketError { error, ticket });
        }
        self.slots[ticket.slot].hold = None;
        self.publish();
        Ok(())
    }
    #[cfg(not(feature = "vesper-port"))]
    fn publish(&mut self) {
        if let Some(min) = self.slots.iter().filter_map(|s| s.hold).min() {
            self.watermark = min;
        }
    }
}

#[cfg(feature = "vesper-port")]
impl Frontier {
    pub fn live_holds(&self) -> usize {
        crate::port::binding::frontier_live_holds(&self.live) as usize
    }
    pub fn acquire(&mut self, timestamp: i64) -> Result<FrontierTicket, Error> {
        let slot = crate::port::binding::frontier_acquire(
            &mut self.gens,
            &mut self.holds,
            &mut self.live,
            self.capacity as u64,
            &mut self.watermark,
            timestamp,
        )
        .map_err(crate::port::frontier_error)? as usize;
        Ok(FrontierTicket {
            origin: self.origin,
            slot,
            generation: self.gens[slot],
            timestamp,
        })
    }
    fn validate(&self, ticket: &FrontierTicket) -> Result<(), Error> {
        if ticket.origin != self.origin {
            return Err(Error::ForeignTicket);
        }
        if !crate::port::binding::frontier_valid(
            &self.gens,
            &self.holds,
            &self.live,
            ticket.slot as u64,
            ticket.generation,
            ticket.timestamp,
        ) {
            return Err(Error::StaleTicket);
        }
        Ok(())
    }
    pub fn advance(
        &mut self,
        mut ticket: FrontierTicket,
        next: i64,
    ) -> Result<FrontierTicket, TicketError> {
        if ticket.origin != self.origin {
            return Err(TicketError {
                error: Error::ForeignTicket,
                ticket,
            });
        }
        match crate::port::binding::frontier_advance(
            &mut self.gens,
            &mut self.holds,
            &self.live,
            &mut self.watermark,
            ticket.slot as u64,
            ticket.generation,
            ticket.timestamp,
            next,
        ) {
            Err(e) => Err(TicketError {
                error: crate::port::frontier_error(e),
                ticket,
            }),
            Ok(generation) => {
                ticket.generation = generation;
                ticket.timestamp = next;
                Ok(ticket)
            }
        }
    }
    pub fn retire(&mut self, ticket: FrontierTicket) -> Result<(), TicketError> {
        if ticket.origin != self.origin {
            return Err(TicketError {
                error: Error::ForeignTicket,
                ticket,
            });
        }
        crate::port::binding::frontier_retire(
            &self.gens,
            &self.holds,
            &mut self.live,
            &mut self.watermark,
            ticket.slot as u64,
            ticket.generation,
            ticket.timestamp,
        )
        .map_err(|e| TicketError {
            error: crate::port::frontier_error(e),
            ticket,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_and_generation_exhaustion_preserve_holds() {
        let mut f = Frontier::new(0, 1).unwrap();
        let t = f.acquire(1).unwrap();
        // Module-private adversarial copy models a stale wire handle, not public Clone.
        let stale = FrontierTicket {
            origin: t.origin,
            slot: t.slot,
            generation: t.generation,
            timestamp: t.timestamp,
        };
        let mut t = f.advance(t, 2).unwrap();
        let err = f.retire(stale).unwrap_err();
        assert_eq!(err.error, Error::StaleTicket);
        assert_eq!(f.watermark(), 2);
        assert_eq!(f.live_holds(), 1);
        #[cfg(not(feature = "vesper-port"))]
        {
            f.slots[t.slot].generation = u64::MAX;
        }
        #[cfg(feature = "vesper-port")]
        {
            f.gens[t.slot] = u64::MAX;
        }
        t.generation = u64::MAX;
        let err = f.advance(t, 3).unwrap_err();
        assert_eq!(err.error, Error::IdentityExhausted);
        assert_eq!(err.ticket.timestamp(), 2);
        assert_eq!(f.watermark(), 2);
        f.retire(err.ticket).unwrap();
        assert_eq!(f.live_holds(), 0);
        assert!(matches!(f.acquire(3), Err(Error::Limit("frontier holds"))));
    }
}
