//! The aggregate decisions of the native optimize path as text, for the
//! self-hosting differential of `@vesper/codegen-aggregates` (SH4 lane 8,
//! #1282; tests/self_host_codegen_aggregates.rs).
//!
//! Read-only: this module calls the decision functions of
//! `native_aggregates.rs` ([`super::aggregates::split_locals`],
//! [`super::aggregates::plan`], [`super::aggregates::split_sources`],
//! [`super::aggregates::split_words`], [`super::aggregates::plain_data`])
//! the way `Codegen` calls them for a top-level graph with no generic
//! arguments, and prints what they decide. It changes no lowering.
use super::aggregates::{self, Home};
use crate::check::World;
use crate::ir::{FunctionIr, Program};
use std::collections::BTreeMap;
use std::fmt::Write as _;

fn path_text(path: &[String]) -> String {
    if path.is_empty() {
        "-".into()
    } else {
        path.join(".")
    }
}

/// One graph's decisions, one per line, in a fixed order:
///
/// - `local L pd=P sw=N` for every local: `plain_data` of its type (0/1)
///   and the word count of `split_words` (`-` when it is not splittable);
/// - `split L P1 P2 ..` for every local `split_locals` splits, with the
///   field paths its uses read (`-` reads it whole);
/// - `home vV result`, `home vV field vP OFFSET` or
///   `home vV place PLACE BLOCK INDEX` for every value `plan` gives a home;
/// - `source vV L` for every read `split_sources` lends to split local `L`.
pub fn function_decisions(world: &World, f: &FunctionIr, out: &mut String) {
    let none = BTreeMap::new();
    let _ = writeln!(out, "fn {}", f.name);
    for (n, t) in &f.locals {
        let ty = crate::numeric::default_type(t);
        let pd = aggregates::plain_data(world, &ty) as u8;
        let sw =
            aggregates::split_words(world, &ty).map_or("-".to_owned(), |w| w.len().to_string());
        let _ = writeln!(out, "local {n} pd={pd} sw={sw}");
    }
    let split = aggregates::split_locals(world, f, &none);
    for (n, reads) in &split {
        let _ = write!(out, "split {n}");
        for r in reads {
            let _ = write!(out, " {}", path_text(r));
        }
        out.push('\n');
    }
    let homes = aggregates::plan(world, f, &none, false, &split);
    for (v, h) in &homes {
        let _ = match h {
            Home::Result => writeln!(out, "home v{v} result"),
            Home::Field { parent, offset, .. } => {
                writeln!(out, "home v{v} field v{parent} {offset}")
            }
            Home::Place { place, at } => {
                writeln!(out, "home v{v} place {place} {} {}", at.0, at.1)
            }
        };
    }
    for (v, n) in aggregates::split_sources(f, &split) {
        let _ = writeln!(out, "source v{v} {n}");
    }
}

/// [`function_decisions`] of every function of `program` the native
/// optimize path plans (every function but `fip` ones), in name order.
pub fn program_decisions(world: &World, program: &Program) -> String {
    let mut out = String::new();
    for f in program.functions.values() {
        if world.functions.get(&f.name).is_some_and(|src| src.fip) {
            continue;
        }
        function_decisions(world, f, &mut out);
    }
    out
}
