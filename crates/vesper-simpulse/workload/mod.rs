//! A seeded workload that drives every `Simulator` transition, shared by
//! tests/stress.rs (byte-identical journals on both builds) and
//! examples/abba.rs (the benchmark).
#![allow(dead_code)]
use vesper_simpulse::*;

/// SplitMix64, independent of the simulator's own generator.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// FNV-1a over the `Debug` text of every outcome, errors included.
pub struct Digest(pub u64, bool);
impl Digest {
    pub fn new() -> Self {
        Self(0xcbf29ce484222325, true)
    }
    /// A digest that formats nothing (the benchmark times the simulator).
    pub fn off() -> Self {
        Self(0, false)
    }
    pub fn add(&mut self, value: &impl std::fmt::Debug) {
        if !self.1 {
            return;
        }
        for b in format!("{value:?};").bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
    }
}

fn result(rng: &mut Rng) -> IoResult {
    match rng.below(4) {
        0 => IoResult::Error {
            code: -(rng.below(200) as i32),
        },
        1 => IoResult::Cancelled,
        _ => IoResult::Success {
            bytes: rng.below(1 << 20),
        },
    }
}

/// The mix of the workload: what the dispatched task does, in percent.
#[derive(Clone, Copy)]
pub struct Mix {
    pub yield_task: u64,
    pub sleep: u64,
    pub schedule_io: u64,
    pub park_io: u64,
    /// Side actions per dispatch, in percent: cancel, stale completion,
    /// spawn.
    pub side: u64,
    /// Largest sleep and I/O delay.
    pub delay: u64,
}

pub const MIXED: Mix = Mix {
    yield_task: 33,
    sleep: 30,
    schedule_io: 20,
    park_io: 15,
    side: 12,
    delay: 1000,
};

/// Spawn `tasks` tasks and dispatch about `dispatches` times, then finish
/// every task; the number of dispatches.
pub fn drive(
    sim: &mut Simulator<u64>,
    seed: u64,
    tasks: u64,
    dispatches: u64,
    mix: Mix,
    d: &mut Digest,
) -> u64 {
    let mut rng = Rng(seed ^ 0x5eed_5eed);
    let mut spawned = vec![];
    let mut completions = vec![];
    let mut external = vec![];
    for label in 0..tasks {
        let task = sim.spawn(label).unwrap();
        spawned.push(task);
    }
    let mut n = 0;
    loop {
        let token = match sim.next_task() {
            Err(e) => {
                d.add(&e);
                break;
            }
            Ok(None) => {
                if external.is_empty() {
                    break;
                }
                let i = rng.below(external.len() as u64) as usize;
                let c = external.swap_remove(i);
                d.add(&sim.complete_io(c, result(&mut rng)));
                continue;
            }
            Ok(Some(token)) => token,
        };
        n += 1;
        d.add(&(
            token.task(),
            token.reason(),
            token.cancel_requested(),
            sim.now_ns(),
            *token.payload(),
        ));
        let ending = n >= dispatches;
        if rng.below(100) < mix.side {
            match rng.below(4) {
                0 => {
                    let t = spawned[rng.below(spawned.len() as u64) as usize];
                    d.add(&sim.request_cancel(t));
                }
                1 if !completions.is_empty() => {
                    let c = completions[rng.below(completions.len() as u64) as usize];
                    d.add(&sim.complete_io(c, result(&mut rng)));
                }
                2 if !external.is_empty() => {
                    let c = external.swap_remove(rng.below(external.len() as u64) as usize);
                    d.add(&sim.complete_io(c, result(&mut rng)));
                }
                _ if !ending => {
                    let t = sim.spawn(n).unwrap();
                    spawned.push(t);
                }
                _ => {}
            }
        }
        let pick = if ending { 100 } else { rng.below(100) };
        let a = mix.yield_task;
        let b = a + mix.sleep;
        let c = b + mix.schedule_io;
        let e = c + mix.park_io;
        if pick < a {
            d.add(&sim.yield_task(token));
        } else if pick < b {
            let r = sim.sleep(token, rng.below(mix.delay));
            if let Ok(id) = &r {
                completions.push(*id);
            }
            d.add(&r);
        } else if pick < c {
            let delay = rng.below(mix.delay);
            let r = sim.schedule_io(token, delay, result(&mut rng));
            if let Ok(id) = &r {
                completions.push(*id);
            }
            d.add(&r);
        } else if pick < e {
            let r = sim.park_io(token);
            if let Ok(id) = &r {
                completions.push(*id);
                external.push(*id);
            }
            d.add(&r);
        } else {
            d.add(&sim.finish(token));
        }
    }
    d.add(&(sim.is_complete(), sim.pending_count(), sim.now_ns()));
    n
}
