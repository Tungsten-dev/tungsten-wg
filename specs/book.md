# Spec: Tungsten usage guide as an mdBook

## Summary

Replace `docs/use-guide.md` with an mdBook in `book/`. It is a usage guide for frontend and tool authors: Rust developers who embed `tungsten-wg`, build programs with `tungsten-builder`, choose backends and run the generated code. The book is built locally with `mdbook build`. There is no CI or deployment yet. Rust snippets are checked as doctests.

## Goals

- One structured guide a reader can follow from first compile to workers, mappings and refinement.
- `use-guide.md` is split into chapters and expanded. It is then deleted.
- The builder is the way to build Tungsten IR (frontends such as tungsten-mc-compile use it), so its section is the most extensive, and is written as a full guide rather than a short chapter.
- Each backend (Rust, Java, CUDA, Inspect) gets its own chapter.
- Rust snippets compile under `cargo test --doc`.

## Non-goals

- No GitHub Actions or Pages deployment (deferred).
- No test-suite chapter. `TESTING.md`, `docs/testing-policy.md` and `crates/tungsten-testsuite/README.md` stay as they are.
- No internals or contributor section. `docs/redesign.md` stays a repo doc and the book links to it.
- No `.tw` content. `.tw` is a test-corpus format, not a user-facing way to write programs, so the book does not cover it.
- No change to crate behaviour.

## Design

### Layout

```
book/
  book.toml          # title "Tungsten", src = "src", build-dir = "../target/book"
  src/
    SUMMARY.md
    introduction.md
    getting-started.md
    model.md
    builder/            # tungsten-builder, the primary way to build IR (extensive)
      overview.md       # why the builder, a first program, the prelude, the mental model
      programs.md       # ProgramBuilder: units, entries, grids, finish_unit, build, merging and names
      functions.md      # FnBuilder: params, locals, statements, control flow, ret
      expressions.md    # typed expressions, operators, casts, ambient reads (pos3, origin, scales)
      types.md          # type system, typing rules, vec3/pos3/array, arithmetic and checked ops
      inputs.md         # density inputs, density(name) reads, mappings and @previous from the builder
      autoconsts.md     # params, hosts, autoconsts, constants
      externs.md        # extern functions, records, registry_from, the extern_fns!/record_decls! macros
      macros.md         # build!, body!, vt!, vec3!, pos3!, array!
      errors.md         # collected errors with file and line, derived unit lists, determinism
      patterns.md       # worked example: frontend lowering an enum density tree (as in the README), refinement from the builder
    compiling.md        # Compiler, passes, CompileError, deny_warnings, custom backends
    backends/
      rust.md
      java.md
      cuda.md
      inspect.md
    running.md          # Orchestration and workers, per-language
    mappings.md
    refinement.md
    reference.md        # common mistakes, where to look when something fails
```

Builder section sources: `crates/tungsten-builder/README.md`, the public API in `crates/tungsten-builder/src/` (`program.rs`, `function.rs`, `expr.rs`, `derive.rs`, `error.rs`, `macros.rs`) and its tests (`tests/api.rs`, `typing.rs`, `macros.rs`, `merge.rs`, `slices/`) for real usage. The `slices/` tests (aquifer, noise, recursive, top_surface) are the source for the worked patterns page. The builder README stays as the short crate-level doc and links to the book for depth.

Other chapter sources: use-guide §1–2 → introduction and model; §3.1 and README usage → getting-started and compiling; §3.2–3.3 and §4 are dropped (`.tw` and the test suite); §5.1 → mappings; §5.2 → running; §5.3 → refinement; §5.4 and §6 → reference. Backend chapters draw on the README, `crates/tungsten-cuda/README.md` and the backend crates' extension traits (`RustCompile`, `JavaCompile`, `CudaCompile`, `InspectCompile`). Each states its config, the output type, how to run the result, and backend-specific caveats.

`target/book` keeps the build output inside the already-ignored `/target`, so `.gitignore` needs no change.

### Doctests

The builder section carries most of the Rust snippets, and every page there must have runnable (not `ignore`d) examples built on `tungsten_wg::builder::prelude::*` and `registry_from`, ending in a successful `build()` where the page is about construction. `src/lib.rs` already compiles the root and builder READMEs under `#[cfg(doctest)]` with `include_str!`. The book follows that pattern: a `#[cfg(doctest)]` module per chapter containing Rust snippets, using `include_str!("../book/src/<chapter>.md")`. Snippets use the same hidden-line conventions (`# fn main()`, `registry_from`). Snippets that cannot run (CUDA, Java, or fragments needing a GPU or JVM) use `rust,ignore`, as the README does. Java blocks use their own language tags and are not compiled. `mdbook test` is not used.

### README and cross-references

- `README.md` links to the book (build instructions: `mdbook build book`, `mdbook serve book`) and its "use-guide.md" mention changes to point at the mappings and refinement chapters.
- `specs/workers.md`, `specs/density-mappings.md`, `specs/refinement.md` and `specs/combined-rollout.md` mention `use-guide.md` as a landed-docs record. These are historical notes, so they are updated to name the book chapters (or left reading "use-guide.md (now `book/`)") to avoid dangling references.
- Crate READMEs stay as API-level docs. Where the book and a README overlap, the book links to the README instead of copying (builder API, CUDA stream details).

## Behavior and edge cases

- `mdbook build book` must succeed with no broken links. mdbook 0.5.4 is installed.
- Chapters must not describe unavailable features. use-guide says everything is available on every backend unless a section says otherwise. Each backend chapter must state any difference (for example CUDA's `enqueue`/`sync`/`result` flow and `Profile::Tolerance`).
- Code that would drift (generated names such as `final_density_worker`, `refine_<name>`) is described by its naming rule and shown through one snippet.
- Doctests behind features (`cuda`, `java`) must not break default `cargo test --doc`. They are `ignore`d or `cfg`-gated.

## Decisions

| Decision | Chosen | Rejected |
| --- | --- | --- |
| Relation to use-guide | Split into chapters, delete the old file | `{{#include}}` of existing docs; keep both (drift) |
| Audience | Frontend and tool authors | Worldgen modders; two tracks |
| Location, publishing | `book/`, local build only | `docs/` as the book root; GitHub Pages now |
| Testing | Rust only, as doctests via `include_str!` | `mdbook test` (needs `-L` paths and features) |
| Structure | Builder section (the largest, 11 pages); one chapter per backend | A single short builder chapter |
| Scope | No test-suite chapter, no internals section | Including `TESTING.md` and `redesign.md` |

## Validation plan

1. `mdbook build book` succeeds, with no warnings about missing chapters or broken links.
2. `cargo test --doc` (default features) passes with the book's snippets included.
3. Every chapter named in the Layout is listed in `SUMMARY.md` and non-empty.
4. A grep for `use-guide` over the repo (excluding `target`) finds no stale reference.
5. Spot check: each use-guide section's content appears in exactly one chapter, with nothing lost.

## Open questions / deferred

- GitHub Pages deployment through Actions.
- Theme, logo and a dependency diagram (README has a TODO for a draw.io diagram).
- Final page split of the builder section is a starting point. Pages may merge or split while writing, as long as the topics are covered.
- Whether to later add a test-suite or internals section.
