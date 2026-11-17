//! The Rust reference build of `Target::parse`.

use crate::target::{
    OriginTarget, Query, QueryEntry, Segment, Target, TargetError, TargetLimits, Utf8Policy,
};

pub(crate) fn parse(raw: &str, limits: TargetLimits) -> Result<Target, TargetError> {
    let bytes = raw.as_bytes();
    if bytes.len() > limits.max_target_bytes {
        return Err(TargetError::TooLong);
    }
    if bytes == b"*" {
        return Ok(Target::Asterisk);
    }
    // Reuse the decoder's origin-form grammar: leading single `/`, valid
    // escapes, no fragment, backslash, controls or raw non-ASCII bytes.
    if !crate::reference::decoder::valid_target(b"GET", bytes) {
        return Err(classify_syntax(bytes));
    }
    let (path, query) = match bytes.iter().position(|b| *b == b'?') {
        Some(index) => (&bytes[..index], Some(&bytes[index + 1..])),
        None => (bytes, None),
    };
    let rest = &path[1..];
    let (rest, trailing_slash) = match rest.strip_suffix(b"/") {
        Some(stripped) => (stripped, true),
        None => (rest, false),
    };
    let segment_count = if rest.is_empty() {
        if trailing_slash {
            // Only `//`, which the grammar already rejects.
            return Err(TargetError::EmptySegment);
        }
        0
    } else {
        rest.iter().filter(|b| **b == b'/').count() + 1
    };
    if segment_count > limits.max_segments {
        return Err(TargetError::TooManySegments);
    }
    let entry_count = match query {
        Some(query) if !query.is_empty() => query.iter().filter(|b| **b == b'&').count() + 1,
        _ => 0,
    };
    if entry_count > limits.max_query_entries {
        return Err(TargetError::TooManyQueryEntries);
    }
    let mut segments = Vec::new();
    segments
        .try_reserve_exact(segment_count)
        .map_err(|_| TargetError::AllocationFailed)?;
    if segment_count > 0 {
        for raw in rest.split(|b| *b == b'/') {
            if raw.is_empty() {
                return Err(TargetError::EmptySegment);
            }
            let decoded = decode(raw, false)?;
            if decoded.contains(&b'/') {
                return Err(TargetError::EncodedSlash);
            }
            if decoded.contains(&b'\\') {
                return Err(TargetError::EncodedBackslash);
            }
            if decoded.contains(&0) {
                return Err(TargetError::EncodedNul);
            }
            if decoded == b"." || decoded == b".." {
                return Err(TargetError::DotSegment);
            }
            check_utf8(&decoded, limits.utf8)?;
            segments.push(Segment(decoded));
        }
    }
    let query = match query {
        None => None,
        Some(query) => {
            let mut entries = Vec::new();
            entries
                .try_reserve_exact(entry_count)
                .map_err(|_| TargetError::AllocationFailed)?;
            if !query.is_empty() {
                for raw in query.split(|b| *b == b'&') {
                    if raw.is_empty() {
                        return Err(TargetError::EmptyQueryEntry);
                    }
                    let (key, value) = match raw.iter().position(|b| *b == b'=') {
                        Some(index) => (&raw[..index], Some(&raw[index + 1..])),
                        None => (raw, None),
                    };
                    if key.is_empty() {
                        return Err(TargetError::EmptyQueryKey);
                    }
                    let key = decode_query(key, limits.utf8)?;
                    let value = value.map(|v| decode_query(v, limits.utf8)).transpose()?;
                    entries.push(QueryEntry { key, value });
                }
            }
            Some(Query { entries })
        }
    };
    Ok(Target::Origin(OriginTarget {
        segments,
        trailing_slash,
        query,
    }))
}

fn classify_syntax(bytes: &[u8]) -> TargetError {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let valid = bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit);
            if !valid {
                return TargetError::InvalidEscape;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    TargetError::InvalidSyntax
}
fn check_utf8(bytes: &[u8], policy: Utf8Policy) -> Result<(), TargetError> {
    if policy == Utf8Policy::Require && std::str::from_utf8(bytes).is_err() {
        return Err(TargetError::InvalidUtf8);
    }
    Ok(())
}
fn decode_query(raw: &[u8], policy: Utf8Policy) -> Result<Vec<u8>, TargetError> {
    let decoded = decode(raw, true)?;
    if decoded.contains(&0) {
        return Err(TargetError::EncodedNul);
    }
    check_utf8(&decoded, policy)?;
    Ok(decoded)
}
fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
/// The only percent decoder in Prism. Output is never re-fed to it.
fn decode(raw: &[u8], plus_is_space: bool) -> Result<Vec<u8>, TargetError> {
    let mut out = Vec::new();
    out.try_reserve_exact(raw.len())
        .map_err(|_| TargetError::AllocationFailed)?;
    let mut index = 0;
    while index < raw.len() {
        let byte = raw[index];
        if byte == b'%' {
            let high = raw.get(index + 1).copied().and_then(hex);
            let low = raw.get(index + 2).copied().and_then(hex);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(TargetError::InvalidEscape);
            };
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(if plus_is_space && byte == b'+' {
                b' '
            } else {
                byte
            });
            index += 1;
        }
    }
    Ok(out)
}
