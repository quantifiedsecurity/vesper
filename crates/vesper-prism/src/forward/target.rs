//! Forwards [`crate::target::Target::parse`] to the `vesper/target` package, then
//! builds the public decoded-target types from the byte ranges it returns.

use crate::target::{OriginTarget, Query, QueryEntry, Segment, Target, TargetError, TargetLimits};

/// The types at the library boundary (`--rust-types`).
pub(crate) mod abi {
    pub use crate::target::{TargetError, TargetLimits, Utf8Policy};
}

#[allow(unsafe_code)]
#[rustfmt::skip]
#[path = "../../port/target.rs"]
mod binding;

pub(crate) fn parse(raw: &str, limits: TargetLimits) -> Result<Target, TargetError> {
    let parsed = binding::target_parse(raw.as_bytes(), limits)?;
    let view = parsed.as_ref();
    if view.asterisk() {
        return Ok(Target::Asterisk);
    }
    let bytes = view.segments().to_vec();
    let mut segments = Vec::with_capacity(view.segment_ends().len());
    let mut at = 0;
    for end in view.segment_ends().iter() {
        let end = end as usize;
        segments.push(Segment(bytes[at..end].to_vec()));
        at = end;
    }
    let query = view.has_query().then(|| {
        let keys = view.keys().to_vec();
        let values = view.values().to_vec();
        let (mut key_at, mut value_at) = (0, 0);
        let mut entries = Vec::with_capacity(view.key_ends().len());
        for ((key_end, value_end), present) in view
            .key_ends()
            .iter()
            .zip(view.value_ends().iter())
            .zip(view.value_present().iter())
        {
            let (key_end, value_end) = (key_end as usize, value_end as usize);
            entries.push(QueryEntry {
                key: keys[key_at..key_end].to_vec(),
                value: present.then(|| values[value_at..value_end].to_vec()),
            });
            (key_at, value_at) = (key_end, value_end);
        }
        Query { entries }
    });
    Ok(Target::Origin(OriginTarget {
        segments,
        trailing_slash: view.trailing_slash(),
        query,
    }))
}
