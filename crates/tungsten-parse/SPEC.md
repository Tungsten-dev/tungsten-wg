# `tungsten-parse`: text format, parser, elaborator and canonical printer

Status: implemented (see §15 for where the implementation differs or fills in details). Refines `TESTING.md` §8. Where they disagree, this document
wins, and the TESTING.md example is updated to match (§13).

## 1. Summary

`tungsten-parse` reads `.tw` test files and writes `OptimizedProgram`s back as text. It has three
stages:

1. **`parse`**: text → syntax AST with spans. Pure syntax, no dependencies, never panics.
2. **`elaborate`**: AST + `Env` → `OptimizedProgram` + `SourceMap` + envelope. Resolves names,
   derives the few omitted fields, expands `use`, and assigns arena slots. The `Env` supplies the
   extern registry and library sources (the prelude). The crate itself knows neither.
3. **`print`**: `OptimizedProgram` → canonical text. Total over every structurally valid program,
   including real tungsten-mc output. It needs no `Env`, and its output elaborates under `NoEnv`.

The format has two layers: a lenient input language for people writing tests, and a canonical
form that is fully explicit, deterministic, and a fixpoint of `print ∘ parse`.

## 2. Decisions

| Topic | Decision |
| --- | --- |
| Architecture | `parse` → AST, then `elaborate(ast, &dyn Env)`. Besides `tungsten-ir`, tungsten-parse uses well-maintained crates where they replace hand-written infrastructure: `logos` (lexer), `codespan-reporting` (diagnostics), `stacker` (stack growth). The parser, elaborator and printer are hand-written. The testsuite implements `Env` (registry + prelude). |
| Printer scope | Total over any program whose refs resolve: weird names, duplicate names, `HostInput::Dynamic`, unregistered externs, density reads from helpers, shared fns. Dangling refs panic, like `Arena::get`. |
| Top-level shape | Flat. Every fn is top level. A unit names its main fn (`main f;`) and its inputs. Density reads are `density(unit.input)`, so a read can sit in any fn, including a helper shared between units. |
| Fn labels | Label ⇔ `canonical_name`: `f` ⇔ `Some("f")`, `` `a:b/c` `` quoted, `%N` ⇔ `None`, `f#N` when the name is duplicated. Unit, table, entry and input labels are syntax only. |
| Variables | Canonical: declared in slot order (typed params first when they're a slot prefix, then a `vars { }` block). Lenient: `let x: T [= e];` appends a slot. Arena order should be semantically insignificant, and a metamorphic test checks that (§12). |
| Namespaces | Separate per kind. Bare identifiers in expressions are vars. Other references are wrapped: `const(c)`, `host(h)`, `table(t)`, `density(u.d)`, `ambient(k)`. Lenient: a bare name that isn't a var falls back to a constant. |
| Statements | Mandatory `;` after every simple statement and simple item. Whitespace is fully free. |
| Precedence | Rust precedence. Comparisons don't associate. Shift and bitwise ops (`<< >> >>> & ^ \|`) can't mix unparenthesized with any other binary operator. Canonical form fully parenthesizes. |
| Negative literals | A `-` glued to a digit or `inf` in operand position is part of the literal. `Negate` prints as `-(e)`. |
| Literal typing | The suffix alone decides inside bodies and const exprs. Contextual int→float only in attribute and envelope positions. |
| Derived fields | Only: extern signatures (from `Env`), `Field` type and base type (from `type_of(base)`), `source_hash = 0`, the density-input `var`. Lists (`helpers`, `tables`, `hosts`, `consts`, fn `@hosts`) are never derived. Omitted means empty. |
| Failed derivation | `ElabError` with a span. `@expect reject` tests write those fields explicitly. |
| Prelude | `use lib::{a, b};` copies the named fns plus their transitive library fns and consts. `print` emits them expanded. |
| `&'static str` | A global leaking interner, one copy per distinct string. |
| Diagnostics | First error per stage, with span, carrying its source files so it renders with file name, line and underline. `SourceMap` maps entities, statements and expressions (by path) to spans, so checker and interpreter errors point at lines. |
| Entries | Labeled (`entry main = top …;`). `@expect` refers to entry labels. |
| Envelope | `@knownfail(backend, reason[, case=…][, stage=…])`, `@expect reject[(Kind)]`, grid, all-cells and composite expectations. |
| Inspect | The printer API (with a reusable `Labels` table) is designed so inspect can replace `ir_text.rs` later. Out of scope here. |
| Location | This file. TESTING.md §8 points here. |

## 3. Crate layout and API

```
crates/tungsten-parse/
  Cargo.toml            # deps: tungsten-ir, logos, codespan-reporting, stacker.
                        # Dev: proptest, tungsten-optimizer, tungsten-legacy, bumpalo.
  SPEC.md
  src/lib.rs            # public API, re-exports. Only `ast` is a public module.
  src/span.rs           # Span, FileId, SourceFiles, Diagnostic (rendered by codespan-reporting)
  src/lexer.rs          # tokens (logos), keywords, glued-minus and `joint` flags
  src/numlit.rs         # literal → value conversion (ranges, bit patterns, float parsing)
  src/tables.rs         # one list per fact: numeric kinds, operators, type keywords, ambients
  src/stack.rs          # stack growth for the recursive stages
  src/ast.rs            # syntax tree, every node carries a Span; nesting limits; walkers
  src/parser.rs         # recursive descent + precedence climbing
  src/elaborate/        # mod.rs (two-pass driver, name resolution), body.rs (statements and
                        # expressions), types.rs (derivation), library.rs (use),
                        # envelope.rs (directives and values)
  src/source_map.rs     # SourceMap, StmtPath, ExprPath
  src/printer/          # labels.rs (Labels), program.rs, expr.rs, float.rs, envelope.rs
  src/intern.rs
  src/tests.rs          # L0
  tests/                # roundtrip (P1–P4), fuzz (P5), floats (P6), permute (P7, partly),
                        # depth (P1 at the nesting limits), legacy (converted programs)
```

```rust
pub fn parse(src: &str, file: FileId) -> Result<ast::File, Diagnostic>;
pub fn elaborate(file: &ast::File, env: &dyn Env) -> Result<TestFile, Diagnostic>;
/// parse + elaborate. The common entry point.
pub fn load(src: &str, file: FileId, env: &dyn Env) -> Result<TestFile, Diagnostic>;

pub fn print(program: &OptimizedProgram) -> String;
pub fn print_test_file(t: &TestFile) -> String;
/// Shared label table: inspect and error messages use the same names as the printed text.
pub struct Labels { /* … */ }
impl Labels {
    pub fn new(program: &OptimizedProgram) -> Self;
    pub fn function(&self, f: Ref<Function>) -> &str;
    pub fn unit(&self, u: Ref<ComputeUnit>) -> &str;
    pub fn var(&self, f: Ref<Function>, v: Ref<Var>) -> &str;
    // … const, host, table, input, entry
}
pub fn print_function(p: &OptimizedProgram, labels: &Labels, f: Ref<Function>) -> String;

pub trait Env {
    fn extern_fn(&self, name: &str) -> Option<ExternSig>; // params, return type
    /// Ordered fields of a registry record type.
    fn extern_type(&self, name: &str) -> Option<Vec<(&'static str, VariableType)>>;
    /// Source text of a library (`use prelude::…` → `library("prelude")`), with a FileId of
    /// its own.
    fn library(&self, name: &str) -> Option<(FileId, &str)>;
    /// The name diagnostics use for a file (a path). Default: none, which names a library file
    /// after its library and any other file `file<N>`.
    fn file_name(&self, file: FileId) -> Option<String> { None }
}
pub struct NoEnv; // knows nothing; canonical text must elaborate with it
```

`TestFile` holds the program, the `SourceMap`, entry labels, and the envelope (§9). The returned
program is compact by construction: no holes, and every generation is 0.

## 4. Lexical structure

- **Source**: UTF-8. Whitespace is space, tab, LF and CRLF, and is never significant. Comments are
  `//` to end of line only (no block comments). Columns count Unicode scalar values, 1-based.
- **Identifiers**: `[A-Za-z_][A-Za-z0-9_]*`, excluding keywords. Keywords take no label
  suffix. **Quoted identifiers**:
  `` `…` `` with escapes `` \` `` and `\\`. Any other character except a newline is allowed.
  Keywords, and names like `minecraft:overworld/x`, must be quoted.
- **Label suffixes** (no whitespace inside): `name#N`, `name##N`, `%N`, where `name` is an
  identifier or a quoted identifier. `N` is a decimal slot number. Their meaning per namespace is
  in §5.2. The parser never gives `N` semantics: it only uses it for uniqueness. Slot order always
  comes from textual order.
- **Strings**: `"…"` with `\\ \" \n \t \u{hex}`. Used for perlin idents, case names and reasons.
- **Keywords**: `program fn unit main input entry host table const use let vars return break if
  else while repeat true false as new extern ext mut dynamic none perlin base3d density ambient
  helpers tables hosts consts inf nan` and the type names (§5.4).

### 4.1 Numeric literals

| Form | Example | Node |
| --- | --- | --- |
| Decimal int, no suffix | `42`, `1_000` | `Int` (i32) |
| Decimal int + `b` / `_i8` | `-128b` | `Byte` |
| Decimal int + `L` / `_i64` | `1L` | `Long` |
| Hex / binary int | `0xFFFF_FFFF`, `0b101` | bit pattern of the width: `0xFFFFFFFF` = `Int(-1)` |
| Hex / binary + `_i8` / `_i64` | `0x80_i8`, `0x8000000000000000_i64` | bit pattern |
| Float (`.` or exponent), no suffix or `_f64` | `1.5`, `1e10`, `2.0e-7` | `Double` |
| Float + `f` / `_f32` | `0.1f` | `Float` |
| Specials | `inf`, `-inf`, `nan`, `nan(0x7ff8000000000001)` | `Double` |
| f32 specials | `inf_f32`, `-inf_f32`, `nan_f32`, `nan_f32(0x7fc00001)` | `Float` |
| Bool | `true`, `false` | `Bool` |

Rules:

- Short suffixes (`b`, `L`, `f`) are only allowed on decimal literals, because `0x1b` and `0x1f`
  are hex digits. Hex and binary use `_i8` / `_i64`.
- The radix prefixes are lowercase (`0x`, `0b`); `0X10` is an error. A `_` separates two digits
  (`1_000`, `0xFFFF_FFFF`) or comes right before a long suffix (`1_i64`). Any other `_`
  (`1__0`, `1_`, `0xFF_`) is an error.
- A decimal integer must be in range for its type, including the glued sign: `-2147483648` is
  fine, `2147483648` is an error. Hex and binary literals can't have more significant bits than
  the width, and can't take a glued sign.
- Float literals are parsed **directly in their own type** (`str::parse::<f32>` for f32), never
  through f64, which would round twice. A finite literal that overflows to infinity is an error:
  infinities must be spelled `inf`. Likewise a literal with a nonzero digit that rounds to zero
  (`1e-400`) is an error; subnormals are fine.
- Where a literal is checked: in bodies and const exprs against its own kind (`3000000000` is out
  of range for i32), in attribute and envelope positions against the kind expected there
  (`@origin_scale(3000000000, 1, 1)` is the f64 3e9).
- `1.` and `.5` are not literals. A float needs digits on both sides of `.`.
- `nan(0x…)` gives the exact bits, sign bit included. The bits must encode a NaN (exponent all
  ones, mantissa nonzero), and there are at most 16 / 8 hex digits. Bare `nan` / `nan_f32` is the
  canonical quiet NaN (`0x7ff8000000000000` / `0x7fc00000`). `-nan` is an error.
- **Glued sign**: the lexer marks a `-` that is immediately followed (no whitespace) by a digit or
  `inf`. In operand position the parser folds it into the literal. In infix position `-` is always
  binary. So `a -1` is `a - 1`, `a - -1` subtracts `Int(-1)`, `- 1` and `-(1)` are
  `Negate(Int(1))`, and `--1` is `Negate(Int(-1))`.

## 5. Program syntax

### 5.1 Grammar

```ebnf
file      = directive* program directive* ;
program   = "program" "{" item* "}" ;
item      = use | host | table | const | fn | unit | entry ;

use       = "use" IDENT "::" "{" label ("," label)* ","? "}" ";" ;
host      = "host" label ( ":" type | "dynamic" ) ";" ;
table     = "table" label "=" ( "perlin" "(" STRING "," (STRING | "none") "," UINT ")" | "base3d" ) ";" ;
const     = "const" label "=" cexpr ";" ;
fn        = "fn" label "(" (param ("," param)* ","?)? ")" "->" type fn_attr* "{" vars? stmt* "}" ;
param     = label (":" type)? ;
vars      = "vars" "{" (label ":" type ("," label ":" type)* ","?)? "}" ;
fn_attr   = "@hash" "(" UINT ")" | "@hosts" "(" labels? ")" ;
unit      = "unit" label ("@hash" "(" UINT ")")? "{" unit_item* "}" ;
unit_item = "main" label ";"
          | ("helpers" | "tables" | "hosts" | "consts") "[" labels? "]" ";"
          | "input" label "=" label grid_attrs ";" ;   (* grid_attrs plus "@var" "(" label "." label ")" *)
entry     = "entry" (label "=")? label grid_attrs ";" ;
grid_attrs= "@dims" "(" INT "," INT "," INT ")"
            "@origin_scale" "(" num "," num "," num ")"
            "@position_scale" "(" num "," num "," num ")" ;   (* any order, each exactly once *)
labels    = label ("," label)* ","? ;
label     = IDENT | QUOTED | LABEL_SUFFIXED | "%" UINT ;

stmt      = "let" label ":" type ("=" expr)? ";"
          | label "=" expr ";"
          | "return" expr ";"
          | "break" ";"
          | "if" expr block ("else" (block | if_stmt))?
          | "while" expr block
          | "repeat" UINT block ;
block     = "{" stmt* "}" ;
```

`num` in `grid_attrs` is an attribute position: an int literal is accepted for an f64 (§4.1, §2).
An `input`'s `@var(f.v)` may sit anywhere among its grid attributes; the printer puts it last.

### 5.2 Labels per namespace

| Namespace | IR source | `x` | `x#N` | `x##N` | `%N` |
| --- | --- | --- | --- | --- | --- |
| var (per fn) | `Var.name: Name` | `Named("x")` | `Prefixed("x")` | `Named("x")`, printed only when `Named("x")` occurs more than once in the arena | `Anonymous` |
| host | `HostInput::Static.name` | `Named` | `Prefixed` | dup `Named` | `Anonymous` (or `dynamic`) |
| fn | `canonical_name` | `Some("x")` | `Some("x")`, name is duplicated | — | `None` |
| const | `Constant.name` | `"x"` | `"x"`, name is duplicated | — | — |
| unit, table, entry, input | none (syntax only) | free | free | — | free |

The printer emits `#N` / `##N` / `%N` with `N` = the compacted slot index of that entity in its
arena. Every label must be unique within its namespace, or it's an `ElabError`. Var namespaces are
per fn. Input namespaces are per unit.

**Printer-generated labels** for syntax-only kinds:

- tables: `t<N>`.
- units: the main fn's label, if it's a plain identifier, not of the form `u<digits>`, and unique
  among units. Otherwise `u<N>`.
- inputs: the input var's base string, if it's an identifier, unique within the unit and not of
  the form `d<digits>`. Otherwise `d<N>`.
- entries: the unit label, if the unit has exactly one entry and the label isn't of the form
  `e<digits>`. Otherwise `e<N>`.

### 5.3 Items

- **Slot order** is textual order within each kind. Imported library items go first (§8). How
  kinds are interleaved in the text doesn't matter, and forward references are allowed
  everywhere (elaboration is two-pass: collect labels, then elaborate bodies).
- **`const c = cexpr;`**: no type annotation, because `Constant` stores none. `cexpr` is the
  `ConstExpr` subset of `expr`: literals, `const(c)` (or a bare name, which is always a const
  here), unary and binary ops, casts, field (`::<T>` / `::<T; B>` required unless derivable), index, array
  literal, construct. Anything else is an `ElabError`.
- **`fn`**: the `@hosts(...)` list is `Function.host_inputs`, and omitted means empty. Omitted
  `@hash` means 0.
- **`unit`**: `main` is required exactly once. Each list appears at most once, and omitted means
  empty. Lists are taken **verbatim**, so they can be missing reachable entries or contain extras,
  and the checker judges them. A unit has no return type in the IR, so it has none in the text.
- **`input d = target grid_attrs @var(f.v);`**: one `DensityInput` in the unit's arena. `@var`
  names the var that `DensityInput.var` points to: var `v` of fn `f`, which can be any fn (after
  inlining it's a helper's var). If `@var` is omitted, elaboration appends a new var labeled `d`
  (so `d`, `d#N` and `%N` give `Named`, `Prefixed` and `Anonymous`, §5.2) of type `density` to
  the unit's main fn, after all of that fn's declared vars, in input order. If the main fn
  already has a var labeled `d`, that's an `ElabError`, to avoid silent aliasing. A repeated
  input label is an `ElabError` of its own.
- **`entry [l =] u grid_attrs;`**: without `l =`, the label is the unit label. Duplicate entry
  labels are an error.

### 5.4 Types

```
bool i8 i32 i64 f32 f64 vec3 pos3 density perm_table interpolated_noise_sampler
[T; n]                 // Array(T, n)
extern Name            // Extern(&'static str); Name is an identifier or quoted
```

These are the spellings of `VariableType`'s `Debug` impl, except that arrays are `[T; n]`.

### 5.5 Variables

- A fn's var namespace is flat and covers the whole function. Nested blocks don't scope, `let`
  can't shadow, and a reference resolves anywhere in the fn regardless of textual position. Which
  vars are definitely assigned is the checker's job.
- Slots are assigned in this order: typed params in order, then `vars { }` entries, then `let`
  declarations in textual order (depth-first), then implicit input vars (§5.3).
- An untyped param (`(a, b)`) must be declared in `vars { }`. A param whose declaration appears
  later in the list than another param's isn't an error: `parameters` order is the param list,
  and slot order is the declaration order. An untyped param may repeat (`(a, a)`): the checker
  judges that.
- `let x: T = e;` is a declaration plus `Assign`. `let x: T;` is only a declaration and produces
  no statement. A `let` with no type is an error (no inference).

### 5.6 Statements

`If` with an empty `else_branch` prints without `else`. An `else_branch` that's exactly one `If`
prints as `else if`. `repeat N` takes a non-negative decimal (`usize`). `while` and `if`
conditions aren't parenthesized in the grammar (the canonical form parenthesizes them anyway when
they're binary ops, §10).

## 6. Expressions

### 6.1 Forms

| IR | Lenient | Canonical |
| --- | --- | --- |
| `Variable(v)` | `x` | `x` |
| `Constant(c)` | `const(c)`, or bare `c` if no var `c` exists | `const(c)` |
| `HostInput(h)` | `host(h)` | `host(h)` |
| `PermutationTable(t)` | `table(t)` | `table(t)` |
| `DensityVariable(d, None)` | `density(u.d)`, or `density(d)` (below) | `density(u.d)` |
| `DensityVariable(d, Some(i))` | `density(u.d, i)` | `density(u.d, i)` |
| `Ambient(k)` | `ambient(origin \| pos3 \| origin_scale \| position_scale)` | same |
| `FunctionCall` | `f(a, …)` | `f(a, …)` |
| `ExternCall` | `ext name(a, …)` (signature from `Env`) or `ext name::<(T, …) -> R>(a, …)` | always with `::<…>` |
| `Field` | `e.x` (field and base type derived), `e.x::<T>` (base type derived), `e.x::<T; B>` | `e.x::<T>`, or `e.x::<T; B>` when `type_of(e)` isn't `B` |
| `ArrayAccess` | `e[i]` | `e[i]` |
| `Construct { t, args }` | `vec3(a,b,c)`, `pos3(a,b,c)`, `new T(args)` | `vec3(…)`/`pos3(…)` for those types, else `new T(…)` |
| `ConstructExtern { t, args }` | `new T { f: e, … }` (any `T`; a non-extern one is the checker's business) | same, fields in IR order |
| `ArrayLiteral` | `[a, b]`, `[]` | same |
| `ExplicitCast` | `e as T` | `(e as T)` |
| `UnaryOp Negate / Not` | `-e`, `!e` | `-(e)`, `!(e)` |
| `UnaryOp Ref(false/true)` | `&e`, `&mut e` | `&(e)`, `&mut (e)` |
| `BinaryOp` | infix | `(l op r)` |

Operators: `+ - * /`, `== != < <= > >=`, `&& ||` (`And`/`Or`), `& | ^` (`BitAnd`/`BitOr`/`BitXor`),
`<< >> >>>` (`ShiftLeft`/`ShiftRight`/`LogicalShiftRight`). `!` is the single `Not` operator,
so logical vs bitwise follows from the type. In operand position, `&&` lexes as two `&`.

**Unqualified `density(d)`**: allowed when the enclosing fn is the main fn of, or listed in
`helpers` of, exactly one unit, and that unit has input `d`. Otherwise it's an `ElabError` that
asks for qualification.

**Struct-literal ambiguity**: `ConstructExtern` always starts with `new`, so `if x { … }` never
reads `x { … }` as a construction.

### 6.2 Precedence (lenient)

From tightest to loosest: postfix (`()` `.f` `::<>` `[]`) · unary (`- ! & &mut`) · `as` ·
`* /` · `+ -` · `<< >> >>>` · `&` · `^` · `|` · comparisons (non-associative) · `&&` · `||`.
Everything is left-associative except comparisons.

**Mixing restriction**: let `R = {<<, >>, >>>, &, ^, |}`. An unparenthesized binary expression
with an operator in `R` may only be an operand of the same operator, and an operator in `R` may
only have unparenthesized binary operands with the same operator. `a | b | c` and `a << 1 << 2`
are fine. `a & 1 == 0`, `a << 1 + 2` and `a & b | c` are errors. This rules out every place where
Rust and Java precedence disagree. Unary and `as` operands are unrestricted.

**Nesting limits.** `MAX_DEPTH` (256) caps, each on its own: expression depth (AST nodes, a leaf
is 1, parentheses don't count), block depth (`if`/`while`/`repeat` bodies; each `else if` link
is one level, as in the IR), type depth and envelope value depth. Going deeper is a
`Diagnostic`. Separately, the parser's recursion is capped at `MAX_RECURSION` (2048), far above
what the limits allow, so only pathological input (thousands of redundant parentheses) hits it,
with its own message. Every recursive stage grows its stack on the heap when it runs low
(`stacker`), so these are the only limits, in debug and release builds alike.

### 6.3 Derivation

These are the only derivations. Anything else that's missing is a syntax error.

1. **Extern signature**: `ext name(…)` without `::<…>` takes `parameter_types` and `return_type`
   from `Env::extern_fn`. An unknown extern is an `ElabError`. The arity isn't checked against
   the args (that's the checker's job), so arity tests stay writable.
2. **Field type and base type**: `e.x` without `; B` takes `base_type = type_of(e)`. Without
   `::<…>` the field type is derived from the base type: `vec3.{x,y,z}` → `f64`,
   `pos3.{x,y,z}` → `i32`, and `extern R` → the field's type from `Env::extern_type`.
3. **Density-input var**: §5.3.
4. **Defaults**: `source_hash = 0`. Omitted lists are empty.

`type_of(e)` is defined over the IR expression being built:

| Expression | Type |
| --- | --- |
| var / const / host | declared type / `type_of(const expr)` / declared type |
| literal, `Ambient` | its type (`origin`, `origin_scale`, `position_scale`: `vec3`; `pos3`: `pos3`) |
| `density(…)`, with or without index | return type of the target unit's main fn |
| `table(…)` | `perm_table` |
| fn call / ext call | return type |
| binary arithmetic | identical operand types → that type. `vec3`∘`pos3` either way → `vec3` |
| comparison, `&&`, `\|\|` | `bool` |
| bitwise | identical operand types → that type |
| shifts | left operand type |
| `-`, `!`, `&`, `&mut` | operand type |
| field | its (explicit or derived) type |
| index | `[T; n]` → `T`, `vec3` → `f64`, `pos3` → `i32` |
| construct / construct-extern / cast | `t` / `t` / target |
| array literal | `[type_of(first); len]` |

Any other case (for example `i32 + f64`, an empty array literal, a field on `f64`) is
**underivable**. Only an underivable type that is actually needed causes an `ElabError`. Reject
tests write `::<…>` on the affected nodes, so the checker is what's under test. These rules must
stay a subset of TESTING.md §5.3, so a derived type never disagrees with the checker.

Const types (`type_of(const expr)`) are derived the same way. Only a field of an `extern` record
consults `Env`, and canonical const exprs write every field type, so canonical text needs none.

## 7. Interning

`VariableType::Extern` and `ConstructExtern` field names are `&'static str`. `intern(&str)` keeps
a global `Mutex<HashSet<&'static str>>` and leaks one copy per distinct string. The random program
generator (L6) draws extern and field names from a fixed pool, so memory stays bounded under fuzzing.

## 8. Libraries and `use`

- A library is a `.tw` file whose `program` holds only `fn`, `const` and `use`. Its source comes
  from `Env::library`. Spans in its diagnostics carry the library's `FileId`.
- `use lib::{a, b};` imports the named fns and consts plus their **transitive closure** of library
  fns (via `FunctionCall`) and consts (via `Constant` and `const` references). The closure is
  deduplicated across all `use` items and inserted at the **front** of the fns and consts arenas:
  libraries in order of first `use`, and each library's items in its textual order (its own
  imports first).
- Library code can't use host inputs, tables or density reads, and library fns can't have
  `@hosts`. Every library needs a `FileId` of its own, distinct from the main file's.
- Libraries can `use` other libraries. Cycles are an `ElabError`. A library is elaborated with the
  same `Env`, and its externs resolve through it.
- An imported label that collides with a local label is an `ElabError` (no shadowing, no
  renaming), with a note pointing at the imported definition.
- Lists stay explicit: a unit that calls `sin` lists `sin` **and every library fn `sin` reaches**
  in `helpers`. The checker reports any that are missing. (This is the cost of non-derived lists,
  §14.)
- `print` emits the expanded definitions and no `use`. A printed file is self-contained and
  doesn't depend on later prelude edits.

## 9. Envelope

### 9.1 Directives

Directives come before and/or after `program`, in any order. Each starts with `@` and has a fixed
grammar, so it needs no terminator.

```ebnf
directive = "@knownfail" "(" IDENT "," STRING ("," "case" "=" STRING)? ("," "stage" "=" ("original" | "inlined"))? ")"
          | "@fuel" "(" UINT ")"
          | "@tier" "(" IDENT ")"
          | "@cells" "(" "sample" "=" UINT "," "seed" "=" UINT ")"
          | "@case" STRING "origin" "(" num "," num "," num ")" "seed" "(" UINT ")" ("host" "(" binding ("," binding)* ")")*
          | "@expect" "reject" ("(" IDENT ")")?
          | "@expect" STRING expectation ;
binding     = label "=" value ;
expectation = "error" "(" IDENT ")"
            | label "[" INT "," INT "," INT "]" "=" value
            | label "[" "*" "]" "=" value
            | label "=" "[" value ("," value)* ","? "]" ;
value       = literal | "vec3" "(" value "," value "," value ")" | "pos3" "(" value "," value "," value ")"
            | "[" (value ("," value)*)? "]" | "new" type "{" (IDENT ":" value ("," …)*)? "}" ;
```

- **`@knownfail`**: the backend is stored as a string and the harness validates it (`rust`, `java`
  or `cuda`). `case` and `stage` narrow the scope. The harness requires every covered
  (backend, case, stage) to fail, and a covered combination that passes is an error.
- **`@case`**: the name is unique. `seed` is `u64` (decimal or hex). Every `Static` host must be
  bound exactly once per case, a binding to an unknown or `dynamic` host is an error, and values
  are checked against the host's type. `origin` is an attribute position (ints are allowed).
- **`@expect reject[(Kind)]`**: `Kind` is stored as a string, so the parser doesn't depend on the
  checker, and the harness matches it against the checker's error enum. A reject file can't have
  `@case` or other `@expect` directives.
- **Expectations**: the label is an entry label. Cell coordinates must be inside the entry's
  `dims`. The grid form (`top = [ … ]`) lists every cell in flat x-fastest order (TESTING.md §6.2),
  and its length must be `dx*dy*dz`. A grid always has one element per cell, so array-typed
  entries use nested brackets (`top = [[1, 2]]` for a 1×1×1 entry). Two expectations that cover
  the same (case, entry, cell) are an error.
- **Values** are typed by their expected type (host or entry return type). They're attribute
  positions: an unsuffixed int is accepted for a float, and a suffix that's present must agree.
  A record (`new T { f: v, … }`) is typed by the registry's fields and comes out in their order.
  Without a registry entry (under `NoEnv`, say) it keeps the written order and each field is
  typed by how it's written. Canonical text spells every kind unambiguously (`1`, `1b`, `1L`,
  `1.0`, `1.0f`), so P3 holds without the registry.
  `nan` is a valid expected value, and the comparison rule makes any NaN match it. `perm_table`,
  `interpolated_noise_sampler` and `density` have no literal values, so binding them is an error.

### 9.2 `TestFile`

```rust
pub struct TestFile {
    pub program: OptimizedProgram,
    pub source_map: SourceMap,
    pub entry_labels: Vec<String>,          // index = entry_points index; informational only
    pub cases: Vec<Case>,                   // name, origin [f64; 3], seed u64, hosts Vec<(Ref<HostInput>, Value)>
    pub expects: Vec<Expect>,               // case index, entry index, ExpectKind (Cell | All | Grid | Error(String))
    pub reject: Option<Option<String>>,
    pub knownfails: Vec<KnownFail>,
    pub fuel: Option<u64>,
    pub tier: Option<String>,
    pub cells: Option<CellSample>,
}
pub enum Value { Bool(bool), I8(i8), I32(i32), I64(i64), F32(f32), F64(f64),
                 Vec3([f64; 3]), Pos3([i32; 3]), Array(Vec<Value>), Record(&'static str, Vec<(&'static str, Value)>) }
```

Expectations reference entries **by index**, so `print_test_file` regenerates labels with the
printer rules (§5.2) and rewrites the references. `Value` equality compares floats by bits.

`print_test_file` uses a fixed order: `@knownfail`*, `@fuel`, `@tier`, `@cells`, `program`,
`@case`* (source order), `@expect`* (source order, each in its original form). Composite values
use the canonical expression spellings. Floats use §10.2.

## 10. Canonical printer

### 10.1 Layout

- 4-space indent, LF line endings, a trailing newline, one item or statement per line, and one
  blank line between fns and between units.
- Item order: `host`s, `table`s, `const`s, `fn`s, `unit`s, `entry`s, each in arena iteration
  order. Because the order is slot order, `parse(print(m))` reproduces the compacted slot order.
- **Fn header**: if `parameters` are exactly the first `k` slots, in order, they print typed
  (`(a: f64, b: f64)`), and `vars { }` lists the remaining slots. Otherwise params print untyped
  and `vars { }` lists every slot. `vars { }` is omitted when it would be empty, and has one entry
  per line with a trailing comma. No `let` in canonical output.
- `@hosts(...)` is printed when non-empty. `@hash(0x…)` is always printed (lowercase hex, no
  padding). Unit lists are printed when non-empty, in the order `main`, `helpers`, `tables`,
  `hosts`, `consts`, then `input`s. `@var(f.v)` is always printed.
- Every binary op, cast and unary op is parenthesized as in §6.1, including the outermost
  expression of a statement. The `if` / `while` condition is printed as a plain expression, so it
  carries its own parens when it's a binary op.
- `ext` always carries `::<(T, …) -> R>`, fields always carry `::<T>`, and density reads are
  always qualified. A field writes `; B` only when `type_of(base)` (§6.3, replayed on the IR by
  `printer/derive.rs`) isn't its `base_type`.
- Labels are quoted when they aren't identifiers or when they're keywords.

### 10.2 Floats

- Finite values print as Rust `{:?}` **of the value's own type** (shortest round-trip), with `.0`
  inserted before `e` when the mantissa has no `.` (so `1e16` → `1.0e16`), then `f` for f32.
  `-0.0` prints as `-0.0` / `-0.0f`.
- Infinities print as `inf`, `-inf`, `inf_f32`, `-inf_f32`.
- NaNs always print with full-width bits: `nan(0x7ff8000000000000)`, `nan_f32(0x7fc00000)`.
- Integers print in decimal with their suffix: `1b`, `1`, `1L`.

### 10.3 Totality

The precondition is that every `Ref` resolves: arena refs into their program arenas,
`DensityVariable` refs into **some** unit's `density_inputs` (found by tag, so reads from shared
helpers work), and `DensityInput.var` into **some** fn's `variables` (found with
`Arena::contains`). Anything else panics with a message that names the dangling entity. Every
other IR shape prints, including `HostInput::Dynamic` (`host %N dynamic;`), unregistered externs,
type-inconsistent expressions, `base_type`s that disagree with the base,
`ConstructExtern` with a non-extern type and repeated `parameters`, and the output loads again.
The one exception is depth: a program nested deeper than the limits of §6.2 prints, but its text
doesn't load.

## 11. Diagnostics and `SourceMap`

- `Diagnostic { stage: Lex | Parse | Elab, span, message, notes: Vec<(Span, String)> }`, plus the
  `SourceFiles` it points into, attached by `parse` / `elaborate` / `load`. `Display` renders with
  `codespan-reporting` (ASCII): `error: …`, then `--> name:line:col`, the source lines and an
  underline; each note is a secondary label at its own location (a plain note if its file is
  unknown). File names come from `Env::file_name`, else the library name, else `file<N>`.
  `snippet()`, `file_name()` and `files()` give the same facts to tools. Each stage stops at its
  first error. Tests are small, so recovery isn't worth the complexity.
- `parse` never panics on any input, including unterminated strings, stray bytes in quoted
  identifiers, and deep nesting.
- `SourceMap` holds spans for:
  - entities: fn, unit, const, host, table, entry index, `(fn, var)`, `(unit, input)`;
  - statements: `(fn, StmtPath)`, where `StmtPath` is a list of steps `Index(i)`, `Then`, `Else`
    and `Body`, which index into the IR statement lists. A declaration-only `let` produces no
    statement, and `else if` is `Else, Index(0)`;
  - expressions: `(fn, StmtPath, ExprPath)` and `(const, ExprPath)`, where `ExprPath` steps
    are `Left`, `Right`, `Operand`, `Arg(i)`, `Base`, `Array`, `Index`, `Elem(i)`, `Inner` (cast),
    `DensityIndex` and `FieldInit(i)`. The root is the statement's single expression.
- The map is valid for the program `elaborate` returned. Use `Arena::translate` across a clone. It
  doesn't survive `perform_inline`.

## 12. Properties and tests

| # | Property | Where |
| --- | --- | --- |
| P1 | `elaborate(parse(print(m)), NoEnv) ≅ compact(m)` for every valid `m` within the nesting limits (§6.2). Canonical text needs no `Env`. | L1 `tests/roundtrip.rs` (corpus, generator, post-inline, imports), `tests/depth.rs` (each limit exactly, in debug), `tests/legacy.rs` (converted programs) |
| P2 | `print(load(print(m))) == print(m)` | L1 |
| P3 | `print_test_file(load(print_test_file(t))) == print_test_file(t)` | L1 |
| P4 | Lenient and canonical agree: `load(src) ≅ load(print(load(src)))` | L1, corpus and generated imports |
| P5 | `parse` never panics: random bytes, random token streams, and mutated corpus files (proptest) | L0 |
| P6 | Float literals: every edge value (±0, min subnormal, max, ±inf, NaN payloads with and without the sign bit, `0.1f` vs `0.1`) plus a random bit-pattern sample print and re-parse bit-identically, for f32 and f64 | L0 |
| P7 | **Slot-order insignificance** (metamorphic): `permute(m, seed)` shuffles every arena's slot order (vars, fns, consts, hosts, tables, units, density inputs) and rewrites refs. Interpreter and backend results are equal to the unpermuted program's. | testsuite, next to L5 |

L0 unit tests also cover the glued-minus cases (§4.1), hex suffix rules, out-of-range literals,
each mixing-restriction error, unqualified `density` ambiguity, label quoting and `#N` / `##N`
generation for duplicate and weird names, `@var` defaulting and its collision error, `use` closure
and cycles, and every envelope validation error.

## 13. Changes outside this crate

1. **TESTING.md §3 / §8**: `parse(&str) -> TestFile` becomes `load(src, file, env)`. The §8.2
   example is updated to this syntax (`;`, `const c = …` without a type, units with `main` and no
   return type, `ext`, qualified `density`, `entry l = u`). §8 points here.
2. **tungsten-ir**: `permute(&mut OptimizedProgram, seed)` next to `compact` / structural
   equality (it reuses the `retag.rs` walker), for P7.
3. **tungsten-checker**: lowering keeps each core node's `(fn, StmtPath, ExprPath)` origin (or
   builds a parallel table), so checker and interpreter errors can go through `SourceMap`.
4. **tungsten-inspect** (follow-up, not this change): replace `analyze/ir_text.rs` with
   `print_function` and `Labels`.

## 14. Known consequences and open points

- **Explicit helper lists vs prelude**: using `sin` means listing its whole library closure in
  `helpers`. The checker error names what's missing. If this gets tedious, a list form like
  `helpers [..use]` can be added later without changing the IR mapping.
- **Density-input var type**: the lenient default declares it as `density`, which is what legacy
  conversion produces and what the Rust backend skips. Real programs that use another type still
  print and parse through the explicit `@var` form.
- **`Constant` has no type**: const types are derived, never declared. If a future IR change adds
  a type to `Constant`, the syntax becomes `const c: T = …` and the printer always emits it.

## 15. Implementation notes

Where the implementation differs from, or fills in, the text above:

- **Generators and proptest.** The generator in `tests/common` draws every choice from an `Rng`:
  a seeded stream, or a recorded choice sequence. proptest generates and shrinks the sequence
  (`programs`, `test_files`), and a shrunk failure is shown as canonical text. Past the end of
  the sequence every choice is 0, the simplest option, so failures shrink to small programs. The
  generator also produces records, repeated params, non-extern `ConstructExtern` and libraries.
- **Quoted identifiers and strings** also accept `\n`, `\t`, `\r` and `\u{hex}`, so every name
  the IR can hold (including ones with newlines or control characters) has a spelling.
- **`0b`** is the byte `0` unless a binary digit follows (`0b101` is still binary).
- **Hex and binary literals are never floats**, not even in attribute positions
  (`origin(0x10, 0, 0)` is an error; write `16`).
- **`Expect.entry`** is `Option<usize>`: `error(Kind)` covers a whole case and has no entry.
- **`isomorphic`**, **`compact`** and **`permute`** live in `tungsten-ir/rebuild.rs`. P7 is tested
  here only up to "the permuted program is valid, deterministic and round-trips"
  (`tests/permute.rs`). The interpreter and backend comparison needs the testsuite.
  `isomorphic` formats both programs in full, so it is for tests only.
- **`Expression::Ambient`** was added to `tungsten-ir` as a prerequisite (TESTING.md §4 item 1).
