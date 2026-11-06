# chess-rs

Play chess against a fast, multi-threaded engine in a native desktop GUI, written in Rust.

> **Note:** this project is entirely vibecoded. It was built end-to-end by an
> AI coding agent (Claude) from a few lines of instructions, with no hand-written
> code. It is backed by a large test suite (below), but read it with that in mind.

## Quick start

The only thing you need installed is **[mise](https://mise.jdx.dev)**. mise downloads
the pinned Rust toolchain and runs every task for you. You don't need to know Rust.

```sh
git clone <this repo> chess-rs && cd chess-rs
mise trust          # allow this project's mise.toml
mise run setup      # first time only: installs Rust + builds (a few minutes)
mise run play       # play!
```

`mise tasks` lists every available task.

### Playing

- **Move**: click a piece, then click a destination, or drag and drop. Legal moves show as dots and capture targets as rings.
- **Promotion**: a picker lets you choose queen, rook, bishop or knight.
- **Bot strength**: set the *depth* slider (1–40 plies) and the *threads* slider (defaults to all CPU cores).
  Depth 10–14 replies almost instantly. Above about 20, use **Move now** to cut a long think short.
- **While the bot thinks**, the side panel shows a live count of the positions it has searched, plus positions per second and elapsed time.
- **Other controls**: Undo, Hint (shows a blue arrow), Flip board, Resign, move list with history review,
  Copy FEN/PGN, Load FEN.
- **Modes**: Human vs Bot, Bot vs Bot (watch it play itself), Human vs Human.

| Shortcut | Action |
|---|---|
| Ctrl+Z | Undo |
| Ctrl+N | New game |
| H | Hint |
| F | Flip board |
| Space | Bot: move now |
| ← / → / Home / End | Review game history |
| Esc | Deselect / cancel promotion |

### Command-line options

Pass options after `--`:

```sh
mise run play -- --depth 18 --color black
mise run play -- --fen "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1"
mise run play -- --threads 8 --hash 4096
mise run watch                  # bot vs bot
mise run help                   # all options
```

## Position cache (persists across games)

Every position the bot finishes searching is remembered in `cache/positions.bin`, along with the deep
entries of its transposition table. On the next game, or the next time you launch the app:

- if the bot reaches a position it has already searched at least as deeply, it replies **instantly**
  (the engine panel shows `Source: position cache (instant)`);
- otherwise its search starts with the cached deep results already loaded.

Openings repeat a lot, so the bot gets faster the more you play. The cache:

- is saved at the end of each game, on exit, and via **Save now**;
- is written atomically, so a crash can't corrupt it;
- is versioned and checksummed: an incompatible or damaged file is ignored and rebuilt.

`mise run cache:clear` deletes it.

## Tasks

| Task | What it does |
|---|---|
| `mise run play` | Launch the game (GUI) |
| `mise run watch` | Bot vs bot |
| `mise run test` | Full test suite: engine, published suites, headless GUI end-to-end |
| `mise run test:engine` | Engine tests only |
| `mise run test:gui` | GUI tests; screenshots land in `target/tmp/ui-screenshots/` |
| `mise run test:deep` | Deep perft validation (tens of billions of nodes) |
| `mise run ci` | Format check + clippy (warnings are errors) + tests |
| `mise run perft -- 6` | Count legal move-tree leaves (`--fen`, `--divide`) |
| `mise run bench -- --depth 14` | Search speed benchmark (`--threads N`) |
| `mise run analyze -- --fen "<FEN>" --depth 20` | Analyse a position in the terminal |
| `mise run lint` / `mise run fmt` | Clippy / rustfmt |
| `mise run cache:clear` | Delete the position cache |

## How it works

Two crates in one Cargo workspace:

- **`crates/engine`** (`chess-engine`): the engine itself, with zero dependencies.
- **`crates/gui`** (`chess-rs`): the desktop app, built on [egui/eframe](https://github.com/emilk/egui).

### Engine

- **Board**: bitboards plus a mailbox. `Position` is a ~160-byte `Copy` struct, and moves are applied
  copy-make, so there is no unmake bookkeeping to get wrong. The Zobrist hash and piece-square scores
  are updated incrementally.
- **Move generation**: fully legal (pin and check masks), so there is no make/test/unmake.
  - Sliding pieces use magic bitboards. `build.rs` finds the magic numbers and emits every attack
    table as a `static`, so there is no start-up initialisation.
  - Quiescence search uses a const-generic `NOISY` variant of the generator, which compiles the
    quiet-move code out entirely.
- **Search**: alpha-beta minimax (negamax with principal variation search).
  - Iterative deepening with aspiration windows and check extensions.
  - Transposition table cutoffs.
  - Null-move pruning, guarded against zugzwang.
  - Reverse futility, futility and late-move pruning, all disabled near mate scores and in sparse endgames.
  - Late move reductions.
  - Move ordering by MVV-LVA plus SEE, killer moves and history heuristic.
  - Quiescence search with SEE and delta pruning.
- **Parallelism**: Lazy SMP over `std::thread::scope`, sharing a lock-free transposition table.
  - Each slot is two `AtomicU64`s using Hyatt's XOR trick, so threads never lock.
  - The table is allocated with `alloc_zeroed`, so even multi-GB tables appear instantly.
- **Evaluation**: tapered PeSTO piece-square tables, plus pawn structure, mobility, rook files,
  bishop pair and mop-up.
- **Endgame knowledge**:
  - An exact **KPK bitbase**: king + pawn vs king, computed at compile time by retrograde analysis
    over 196,608 positions.
  - Wrong-bishop and rook-pawn fortress detection.
  - Draw scaling for material that can't force a win.
- **Rules**: checkmate, stalemate, threefold repetition, the fifty-move rule, insufficient material,
  en passant (including the horizontal-pin edge case), castling rights, and under-promotion.

### GUI

- The engine runs on its own thread. The UI sends search jobs over a channel, and each job carries
  its own cancellation flag, so the window never blocks.
- Pieces are original vector art (SVG), rasterised by resvg at the exact display size.

### Performance (16-thread desktop)

| Metric | Result |
|---|---|
| Perft, start position, depth 7 (3.2 billion leaves) | ~1 s (parallel, bulk-counted) |
| Search, single thread | ~3 M nodes/s |
| Search, 16 threads (Lazy SMP) | ~36–50 M nodes/s |
| Benchmark, 8 positions at depth 14 | 3.5 s on 1 thread, 0.8 s on 16 threads |

The release build uses fat LTO, `codegen-units = 1`, `panic = "abort"` and `target-cpu=native`
(set in `.cargo/config.toml`).

## Testing

More than 100 tests run in about 10 seconds. Many of them use public reference data from the chess
programming community; see `crates/engine/tests/data/SOURCES.md` for where each file comes from.

| Suite | What it checks |
|---|---|
| Perft | Exact move counts for the 6 chessprogramming.org reference positions, Sedlak's edge cases (en passant discovered checks, castling through check, promotions out of check, …) and the 128-position standard EPD suite. `test:deep` goes to billions of nodes. |
| Forced mates | Published mate-in-1, mate-in-2 and mate-in-3 suites. The engine must find the **exact** mate distance with a PV that really mates. A brute-force solver independently verifies the distances. |
| Draws | The sunfish stalemate and draw suites (130 positions). 3 positions that need specialist endgame theory are tracked explicitly; the test fails if any other position stops drawing. |
| Repetition | Trap positions where the engine must avoid (or seek) a repetition. |
| Tactics | Regression gates on Win At Chess (257/300 at depth 9) and Bratko-Kopec. |
| Soundness | Pruned search must agree with plain alpha-beta on forced mates. Each pruning technique can be switched off, and the engine must still work. |
| Invariants | Incremental hash and evaluation equal a from-scratch recomputation; the evaluation is colour-symmetric; FEN and SAN round-trip; magic lookups match slow reference attacks. |
| Rules | Fool's mate, stalemate, threefold repetition, fifty-move rule, insufficient material, undo, PGN export. |
| GUI end-to-end | The real app driven headlessly (egui_kittest): clicking, dragging, en passant, castling, promotion picker, checkmate, undo, hint, keyboard shortcuts, Move now, the live positions counter, cache reuse and persistence across restarts. It also renders screenshots. |

As a sanity check that the suite catches real bugs, these were injected one at a time; each one made
between 3 and 14 tests fail:

- no en passant
- en passant ignores the horizontal pin
- castling through an attacked square
- queen-only promotions
- a rook capture keeps the castling right
- no repetition detection
- mate scores ignore distance
- pinned pieces move freely

## Engine tools

```sh
mise run perft -- 6 --fen "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1"
mise run analyze -- --fen "<FEN>" --depth 20
mise run analyze -- --fen "<FEN>" --depth 12 --no-pruning   # plain alpha-beta, for comparison
```

`chess-cli mate <n> --fen "<FEN>"` is an exhaustive forced-mate solver with no pruning, useful for
checking puzzles.
