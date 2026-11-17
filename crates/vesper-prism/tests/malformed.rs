//! Malformed and truncated input never panics, on either build: every byte
//! string is a typed error, an incomplete frame or a request (port wave P1
//! lane 14, #911). The inputs are deterministic mutations of valid requests.
use vesper_prism::{
    Decoder, Limits,
    connection::{Connection, ConnectionLimits, RequestEvent, Response},
    target::{Target, TargetLimits},
};

const SEEDS: [&[u8]; 4] = [
    b"GET /a/b?x=1&y HTTP/1.1\r\nHost: [::1]:80\r\nConnection: keep-alive, close\r\n\r\n",
    b"POST /p%41 HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 3\r\n\r\nabc",
    b"OPTIONS * HTTP/1.1\r\nHost: a.test.\r\nX: \x80\xff\r\n\r\n",
    b"HEAD /%C3%A9 HTTP/1.1\r\nhost: 0xcorp.example\r\n\r\n",
];
const ALPHABET: &[u8] = b"\r\n :%/?&=.[]0x\t\0\x7f\xffA";

/// xorshift64*, so the corpus is the same on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn mutations() -> Vec<Vec<u8>> {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::new();
    for _ in 0..3000 {
        let mut bytes = SEEDS[rng.below(SEEDS.len())].to_vec();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(bytes.len());
            match rng.below(3) {
                0 => bytes[at] = ALPHABET[rng.below(ALPHABET.len())],
                1 => bytes.insert(at, ALPHABET[rng.below(ALPHABET.len())]),
                _ => {
                    bytes.remove(at);
                }
            }
        }
        out.push(bytes);
    }
    out
}

/// FNV-1a over the outcome log, so both builds must agree on every result.
fn digest(log: &str) -> u64 {
    log.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The outcome digest of `mutated_requests_decode_or_fail_typed`, the same
/// for the Rust and the Vesper build.
const DECODE_DIGEST: u64 = 16_759_313_125_583_877_486;

#[test]
fn mutated_requests_decode_or_fail_typed() {
    let mut log = String::new();
    let limits = Limits {
        max_head_bytes: 256,
        max_headers: 8,
        max_body_bytes: 64,
        max_buffered_bytes: 512,
    };
    for bytes in mutations() {
        for split in [0, bytes.len() / 2, bytes.len()] {
            let mut decoder = Decoder::new(limits).unwrap();
            log += &format!("{:?}", decoder.push(&bytes[..split]));
            log += &format!("{:?}", decoder.next_request());
            log += &format!("{:?}", decoder.push(&bytes[split..]));
            decoder.end_input();
            for _ in 0..3 {
                let next = decoder.next_request();
                log += &format!("{next:?}");
                if let Ok(Some(decoded)) = next {
                    let target =
                        Target::parse(&decoded.request.head.target, TargetLimits::default());
                    log += &format!("{target:?}");
                }
            }
        }
    }
    assert_eq!(digest(&log), DECODE_DIGEST, "outcome digest");
}

#[test]
fn mutated_pipelines_keep_the_connection_consistent() {
    for bytes in mutations().chunks(3) {
        let input = bytes.concat();
        let mut connection = Connection::new(ConnectionLimits::default()).unwrap();
        let mut rest = input.as_slice();
        let mut issued = Vec::new();
        for _ in 0..16 {
            let accepted = connection.receive(rest).unwrap_or(rest.len());
            rest = &rest[accepted..];
            if rest.is_empty() {
                connection.end_input();
            }
            loop {
                match connection.poll_request() {
                    RequestEvent::Request(inbound) => issued.push(inbound.id),
                    RequestEvent::Failed(_) | RequestEvent::NoMoreRequests => break,
                    RequestEvent::NeedInput | RequestEvent::Backpressure => break,
                }
            }
            for id in issued.drain(..).rev() {
                let _ = connection.submit(id, Response::new(200, b"ok".to_vec()));
            }
            while !connection.pending_output().is_empty() {
                let n = connection.pending_output().len();
                connection.advance_output(n).unwrap();
            }
            if rest.is_empty() {
                break;
            }
        }
        let _ = connection.shutdown();
    }
}
