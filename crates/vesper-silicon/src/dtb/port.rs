//! The port-backed build of [`DeviceTree::parse`]: the Vesper port
//! (vesper/src/dtb.ves) checks the whole blob and returns a tape of
//! offsets, which this wrapper turns into the same nodes, properties,
//! reservations and phandle table, and the same errors, as the Rust build.

use std::collections::BTreeMap;

use super::{DeviceTree, DtbError, MemRange, Node, Prop};
use crate::binding;

const TRUNCATED: [&str; 7] = [
    "header",
    "totalsize",
    "memory reservation",
    "structure token",
    "node name",
    "property header",
    "property value",
];
const BLOCKS: [&str; 4] = [
    "structure",
    "strings",
    "memory reservation",
    "property name",
];
const MISALIGNED: [&str; 2] = ["structure block", "memory reservation block"];
const STRUCTURE: [&str; 6] = [
    "second root node",
    "root node has a name",
    "unbalanced end node",
    "property outside a node",
    "FDT_END inside a node",
    "no root node",
];
const PHANDLE_PROPS: [&str; 2] = ["phandle", "linux,phandle"];

fn fault(f: binding::DtbFault) -> DtbError {
    use binding::DtbFault as F;
    match f {
        F::Truncated(w) => DtbError::Truncated {
            what: TRUNCATED[w as usize],
        },
        F::BadMagic(m) => DtbError::BadMagic(m as u32),
        F::UnsupportedVersion {
            version,
            last_compatible,
        } => DtbError::UnsupportedVersion {
            version: version as u32,
            last_compatible: last_compatible as u32,
        },
        F::BlockOutOfBounds(b) => DtbError::BlockOutOfBounds {
            block: BLOCKS[b as usize],
        },
        F::Misaligned(w) => DtbError::Misaligned {
            what: MISALIGNED[w as usize],
        },
        F::BadToken { offset, token } => DtbError::BadToken {
            offset: offset as usize,
            token: token as u32,
        },
        F::UnterminatedString(offset) => DtbError::UnterminatedString {
            offset: offset as usize,
        },
        F::NotUtf8(offset) => DtbError::NotUtf8 {
            offset: offset as usize,
        },
        F::TooDeep => DtbError::TooDeep,
        F::TooManyNodes => DtbError::TooManyNodes,
        F::TooManyReservations => DtbError::TooManyReservations,
        F::BadStructure(w) => DtbError::BadStructure(STRUCTURE[w as usize]),
    }
}

/// Bytes the port checked as UTF-8.
fn text(blob: &[u8], at: u64, len: u64) -> String {
    let bytes = &blob[at as usize..(at + len) as usize];
    std::str::from_utf8(bytes)
        .expect("the port checked this name as UTF-8")
        .to_string()
}

pub(super) fn parse(blob: &[u8]) -> Result<DeviceTree, DtbError> {
    let fdt = binding::dtb_parse(blob).map_err(fault)?;
    let v = fdt.as_ref();
    let reservations = v
        .rsv()
        .as_slice()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|r| MemRange {
            base: r[0],
            size: r[1],
        })
        .collect();
    let words = v.nodes();
    let words = words.as_slice();
    let mut nodes: Vec<Node> = Vec::with_capacity(words.len() / 3);
    for (id, n) in words.as_chunks::<3>().0.iter().enumerate() {
        let parent = n[2].checked_sub(1).map(|p| p as usize);
        nodes.push(Node {
            name: text(blob, n[0], n[1]),
            parent,
            children: Vec::new(),
            props: Vec::new(),
        });
        if let Some(p) = parent {
            nodes[p].children.push(id);
        }
    }
    for p in v.props().as_slice().as_chunks::<5>().0 {
        let value = &blob[p[3] as usize..(p[3] + p[4]) as usize];
        nodes[p[0] as usize].props.push(Prop {
            name: text(blob, p[1], p[2]),
            value: value.to_vec(),
        });
    }
    let phandles: BTreeMap<u32, usize> = v
        .phandles()
        .as_slice()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|e| (e[0] as u32, e[1] as usize))
        .collect();
    let tree = DeviceTree {
        version: v.version() as u32,
        boot_cpuid: v.boot_cpuid() as u32,
        reservations,
        nodes,
        phandles,
    };
    match v.ph_fault() {
        0 => Ok(tree),
        1 => {
            let node = v.ph_a() as usize;
            let len = v.ph_c();
            let detail = if len % 4 != 0 {
                format!("{len} bytes is not whole cells")
            } else {
                format!("{} cells, expected 1", len / 4)
            };
            Err(DtbError::BadProperty {
                node: tree.path(node),
                prop: PHANDLE_PROPS[v.ph_b() as usize].to_string(),
                detail,
            })
        }
        _ => Err(DtbError::DuplicatePhandle(v.ph_a() as u32)),
    }
}
