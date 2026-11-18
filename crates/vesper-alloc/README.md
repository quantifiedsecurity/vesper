# vesper-alloc (Strata)

The Strata host allocators of manuscript section 15: bounded regions with
checked handles, region collections (`RegionVec`, `RegionString`,
`RegionFlatTable`, `RegionSlotArena`, `RegionTiled2`), `CacheBTree`, `Tiled`,
slabs and virtual arenas. `ARENA.md`, `FLAT.md` and `TILED.md` describe the
collections.

**Port freeze (port wave P1 lane 6, #903):** the Rust code takes bug fixes
only. The region bookkeeping is ported to Vesper in `vesper/` (package
`@vesper/alloc`); this package, `vesper-alloc`, is the port-backed build. The
compiler and every other dependent link the Rust seed copy in `seed/`
(`vesper-alloc-seed`), which stays the default until the port matches the
Rust build in speed. Report: `docs/parallel-results/port-p1-strata-alloc.md`.
