use crate::{Budget, Error, Limits};
use std::collections::BTreeMap;

pub type Row = Vec<i64>;

/// Canonical nonzero weights; private representation prevents malformed batches.
#[cfg(not(feature = "vesper-port"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZSet {
    pub(crate) width: usize,
    pub(crate) rows: BTreeMap<Row, i64>,
}
/// Canonical nonzero weights; private representation prevents malformed batches.
///
/// The Vesper build keeps a set as the flat cells the port's kernels read and
/// write in place: each row's `width` columns, then its weight, rows strictly
/// ascending. [`ZSet::entries`] lends rows as `&Row`, so it builds them once,
/// on first use.
#[cfg(feature = "vesper-port")]
pub struct ZSet {
    pub(crate) width: usize,
    pub(crate) cells: Vec<i64>,
    rows: std::sync::OnceLock<Vec<Row>>,
}
impl ZSet {
    pub fn empty(width: usize, limits: Limits) -> Result<Self, Error> {
        if width > limits.max_columns {
            return Err(Error::Limit("columns"));
        }
        Ok(Self::from_rows(width, BTreeMap::new()))
    }
    /// Borrows the raw owner, including on overflow/limit/schema errors.
    #[cfg(not(feature = "vesper-port"))]
    pub fn consolidate(
        width: usize,
        entries: &[(Row, i64)],
        limits: Limits,
    ) -> Result<Self, Error> {
        if entries.len() > limits.max_input_entries {
            return Err(Error::Limit("input entries"));
        }
        let mut acc = Accumulator::new(width, limits)?;
        let mut budget = Budget::new(limits);
        for (row, weight) in entries {
            acc.add(row, i128::from(*weight), &mut budget)?;
        }
        acc.finish()
    }
    /// Borrows the raw owner, including on overflow/limit/schema errors.
    #[cfg(feature = "vesper-port")]
    pub fn consolidate(
        width: usize,
        entries: &[(Row, i64)],
        limits: Limits,
    ) -> Result<Self, Error> {
        if entries.len() > limits.max_input_entries {
            return Err(Error::Limit("input entries"));
        }
        if width > limits.max_columns {
            return Err(Error::Limit("columns"));
        }
        // The kernel takes rows of the right width; the first other row is
        // a schema error once the rows before it are added (its own charge
        // comes first).
        let good = entries
            .iter()
            .position(|(row, _)| row.len() != width)
            .unwrap_or(entries.len());
        let mut cells = Vec::with_capacity(good * (width + 1));
        for (row, weight) in &entries[..good] {
            cells.extend_from_slice(row);
            cells.push(*weight);
        }
        let mut budget = Budget::new(limits);
        let out = match crate::port::consolidate(width, &cells, good, limits, &mut budget) {
            // An overflowing sum fails only when the accumulator finishes,
            // after every entry: a later bad row comes first. The kernel
            // passed the work check, so the prefix's charges were paid.
            Err(Error::Overflow) if good < entries.len() => {
                budget.remaining -= good * (width + 1);
                None
            }
            out => Some(out?),
        };
        if let Some((row, _)) = entries.get(good) {
            budget.charge(1usize.checked_add(row.len()).ok_or(Error::Overflow)?)?;
            return Err(Error::Schema);
        }
        out.ok_or(Error::Overflow)
    }
    pub fn width(&self) -> usize {
        self.width
    }
    #[cfg(not(feature = "vesper-port"))]
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    #[cfg(feature = "vesper-port")]
    pub fn len(&self) -> usize {
        self.cells.len() / (self.width + 1)
    }
    #[cfg(not(feature = "vesper-port"))]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    #[cfg(feature = "vesper-port")]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
    #[cfg(not(feature = "vesper-port"))]
    pub fn entries(&self) -> impl Iterator<Item = (&Row, i64)> {
        self.rows.iter().map(|(r, w)| (r, *w))
    }
    #[cfg(feature = "vesper-port")]
    pub fn entries(&self) -> impl Iterator<Item = (&Row, i64)> {
        let rows = self
            .rows
            .get_or_init(|| self.iter().map(|(r, _)| r.to_vec()).collect());
        rows.iter().zip(self.iter().map(|(_, w)| w))
    }
    pub fn plus(&self, other: &Self, limits: Limits) -> Result<Self, Error> {
        union(&[self, other], limits, &mut Budget::new(limits))
    }
    /// A set from rows already canonical (every weight nonzero).
    #[cfg(not(feature = "vesper-port"))]
    pub(crate) fn from_rows(width: usize, rows: BTreeMap<Row, i64>) -> Self {
        Self { width, rows }
    }
    /// A set from rows already canonical (every weight nonzero).
    #[cfg(feature = "vesper-port")]
    pub(crate) fn from_rows(width: usize, rows: BTreeMap<Row, i64>) -> Self {
        let mut cells = Vec::with_capacity(rows.len() * (width + 1));
        for (row, weight) in rows {
            cells.extend_from_slice(&row);
            cells.push(weight);
        }
        Self::from_cells(width, cells)
    }
    /// A set from the canonical cells a kernel returned.
    #[cfg(feature = "vesper-port")]
    pub(crate) fn from_cells(width: usize, cells: Vec<i64>) -> Self {
        Self {
            width,
            cells,
            rows: std::sync::OnceLock::new(),
        }
    }
    /// Rows and weights in ascending row order, rows borrowed as slices.
    #[cfg(not(feature = "vesper-port"))]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&[i64], i64)> {
        self.rows.iter().map(|(r, w)| (r.as_slice(), *w))
    }
    /// Rows and weights in ascending row order, rows borrowed as slices.
    #[cfg(feature = "vesper-port")]
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&[i64], i64)> {
        let width = self.width;
        self.cells
            .chunks_exact(width + 1)
            .map(move |c| (&c[..width], c[width]))
    }
    /// The rows, borrowed as slices, in ascending order.
    pub(crate) fn rows(&self) -> impl Iterator<Item = &[i64]> {
        self.iter().map(|(r, _)| r)
    }
}
#[cfg(feature = "vesper-port")]
impl Clone for ZSet {
    fn clone(&self) -> Self {
        Self::from_cells(self.width, self.cells.clone())
    }
}
#[cfg(feature = "vesper-port")]
impl PartialEq for ZSet {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width && self.cells == other.cells
    }
}
#[cfg(feature = "vesper-port")]
impl Eq for ZSet {}
/// Prints what the Rust build's derived `Debug` prints.
#[cfg(feature = "vesper-port")]
impl std::fmt::Debug for ZSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Rows<'a>(&'a ZSet);
        impl std::fmt::Debug for Rows<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_map().entries(self.0.iter()).finish()
            }
        }
        f.debug_struct("ZSet")
            .field("width", &self.width)
            .field("rows", &Rows(self))
            .finish()
    }
}
pub(crate) struct Accumulator {
    width: usize,
    limits: Limits,
    rows: BTreeMap<Row, i128>,
}
impl Accumulator {
    pub(crate) fn new(width: usize, limits: Limits) -> Result<Self, Error> {
        if width > limits.max_columns {
            return Err(Error::Limit("columns"));
        }
        Ok(Self {
            width,
            limits,
            rows: BTreeMap::new(),
        })
    }
    pub(crate) fn add(
        &mut self,
        row: &[i64],
        weight: i128,
        budget: &mut Budget,
    ) -> Result<(), Error> {
        budget.charge(1usize.checked_add(row.len()).ok_or(Error::Overflow)?)?;
        if row.len() != self.width {
            return Err(Error::Schema);
        }
        if weight == 0 {
            return Ok(());
        }
        if !self.rows.contains_key(row) && self.rows.len() >= self.limits.max_rows {
            return Err(Error::Limit("rows"));
        }
        let old = self.rows.get(row).copied().unwrap_or(0);
        self.rows.insert(
            row.to_vec(),
            old.checked_add(weight).ok_or(Error::Overflow)?,
        );
        Ok(())
    }
    pub(crate) fn finish(self) -> Result<ZSet, Error> {
        let mut rows = BTreeMap::new();
        for (row, w) in self.rows {
            if w != 0 {
                rows.insert(row, i64::try_from(w).map_err(|_| Error::Overflow)?);
            }
        }
        Ok(ZSet::from_rows(self.width, rows))
    }
}
#[cfg(not(feature = "vesper-port"))]
pub(crate) fn union(sets: &[&ZSet], limits: Limits, budget: &mut Budget) -> Result<ZSet, Error> {
    let width = sets.first().ok_or(Error::Schema)?.width;
    let mut acc = Accumulator::new(width, limits)?;
    for set in sets {
        if set.width != width {
            return Err(Error::Schema);
        }
        for (row, w) in set.entries() {
            acc.add(row, i128::from(w), budget)?;
        }
    }
    acc.finish()
}
/// Up to three sets per kernel call; a set of another width is a schema
/// error after the sets before it are added.
#[cfg(feature = "vesper-port")]
pub(crate) fn union(sets: &[&ZSet], limits: Limits, budget: &mut Budget) -> Result<ZSet, Error> {
    let width = sets.first().ok_or(Error::Schema)?.width;
    let good = sets
        .iter()
        .position(|s| s.width != width)
        .unwrap_or(sets.len());
    if good > 3 {
        return Err(Error::Partition("union of more than three sets"));
    }
    let out = crate::port::union(width, &sets[..good], limits, budget)?;
    if good < sets.len() {
        return Err(Error::Schema);
    }
    Ok(out)
}
