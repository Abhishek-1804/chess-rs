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
  Moves slide into place (turn this off with *Animate moves*).
- **Promotion**: a picker lets you choose queen, rook, bishop or knight.
- **Bot strength**: set the *depth* slider (1–40 plies) and the *threads* slider (defaults to all CPU cores).
  The estimated Elo for the chosen depth shows under the slider (see [How strong is the bot?](#how-strong-is-the-bot)).
  Depth 10–14 replies almost instantly. Above about 20, use **Move now** to cut a long think short.
- **While the bot thinks**, the side panel shows a live count of the positions it has searched, plus positions per second and elapsed time.
- **Other controls**: Undo, Hint (shows a blue arrow), Flip board, Resign, move list with history review,
  Copy FEN/PGN, Load FEN.
- **Modes** (buttons at the top of the side panel): Human vs Bot, Bot vs Bot (watch it play
  itself; each game opens with a random line from a built-in opening book, so games differ),
  Human vs Human, and Online (against a friend on another computer, see below).

### How strong is the bot?

Estimated Elo per depth, measured by playing the engine against Stockfish 19 at calibrated
strength levels (`UCI_Elo`, anchored to the CCRL 40/4 rating list). Each depth played 108
games from 27 balanced openings, each with both colours; the standard error is about ±35.
(`elo:calibrate` now draws its openings from the full opening book.)

| Depth | Elo | | Depth | Elo |
|---|---|---|---|---|
| 1 | 1301 | | 7 | 2235 |
| 2 | 1451 | | 8 | 2410 |
| 3 | 1551 | | 9 | 2529 |
| 4 | 1660 | | 10 | 2580 |
| 5 | 1797 | | 11 | 2706 |
| 6 | 1985 | | 12 | 2761 |

Beyond depth 12 (the default) the numbers are extrapolated, with each extra ply worth a little
less than the one before: about 2870 at depth 14, 2940 at 16, 3005 at 19 and 3070 at 40.

CCRL is an engine rating scale, so treat these as a rough guide to human ratings. The
measurements used one search thread; the app's default of all cores is a little stronger at the
same depth. `mise run elo` lists every depth, and `mise run elo:calibrate` repeats the
measurement (it needs a Stockfish binary: `--stockfish <PATH>`).

### Playing online

Play someone on another computer, over the internet, with a short code. No account, server
or port forwarding needed.

1. One player clicks the **Online** mode button, chooses a side, and clicks **Host game**. The app
   shows a code such as `K7QM-X2PA`. Send it to your opponent.
2. The other player clicks **Online** too, types the code under **Join a game** (case and
   dashes don't matter), and clicks **Join**.
3. Both apps connect and the game starts, usually within a few seconds. The host's chosen side
   decides the colours; Black sees the board from their side.

During the game you can **Offer draw** (the opponent accepts or declines), **Resign**, or
**Leave**. There are no take-backs or hints. Online games are saved in `games/` like any
other, so you can review them afterwards.

You can also play from the terminal, against the app or another terminal:

```sh
mise run online -- host                  # prints a code; --color white|black|random, --fen <FEN>
mise run online -- join K7QM-X2PA
```

Type moves (`e4`, `Nf3`, `O-O`, `e7e8q`) or `draw`, `decline`, `resign`, `board`, `quit`.
Moves typed before your turn are queued as premoves, so you can also pipe a game in.

**How it works:** the two apps connect peer to peer with [iroh](https://www.iroh.computer)
(QUIC with hole-punching), falling back to a free public relay when a direct connection isn't
possible. There is no chess-rs server: the host's network identity (an ed25519 key) is derived
from the code, so the code alone is enough for the joiner to look the host up through n0's
public address-lookup service. The traffic is end-to-end encrypted. Anyone who knows a code
can join that game until it starts, so share it only with your opponent; a game that has
started turns away everyone else.

### Saved games

Every game is saved automatically as a standard PGN file in `games/`. The file is rewritten
after every move, so nothing is lost if the app closes mid-game, and it opens in any chess program.
The **Games** tab lists them newest first, with result, length and review accuracy, plus
**Review** and **Delete** buttons. You can also paste any PGN there to review it.

To fill the library quickly, let the bot play itself:

```sh
mise run games:generate                          # 6 games at depth 4
mise run games:generate -- --games 10 --depth 6
```

Each self-play game opens with a random line from the opening book, so the games differ.

### Game Review (like chess.com)

Open **Game Review** (from the Play tab's *Game review* button, or **Review** in the Games
tab). The engine analyses every position and shows:

- an **accuracy** score per player (lichess's accuracy formula on win-percentage loss);
- a rating for every move: **Brilliant** (a sound piece sacrifice), **Great** (the only good
  move), **Best**, **Excellent**, **Good**, **Inaccuracy**, **Mistake**, **Miss** (failing to
  punish an opponent's error), **Blunder** and **Forced**. These use chess.com's
  expected-points bands, and each has a symbol (!!, !, ?!, ?, ??) as well as a colour;
- an **evaluation graph**: hover for details, and click or drag to jump to any move;
- for each move, an explanation ("g4 is a blunder"), the change in evaluation, and the better
  move with its line (green arrow). The blue arrow is the engine's best move in the position shown.

Reviews are saved into the game's PGN file as standard `[%eval]` comments and move annotations,
so reopening a reviewed game is instant and other tools such as lichess can read them.

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

Separately from saved games, every position the bot finishes searching is remembered in `cache/positions.bin`, along with the deep
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
| `mise run online -- host` / `mise run online -- join <CODE>` | Play online from the terminal |
| `mise run elo` | Estimated Elo of every depth |
| `mise run elo:calibrate -- --stockfish <PATH>` | Re-measure Elo per depth against Stockfish (about an hour on 16 cores) |
| `mise run test` | Full test suite: engine, published suites, headless GUI end-to-end |
| `mise run test:engine` | Engine tests only |
| `mise run test:gui` | GUI tests; screenshots land in `target/tmp/ui-screenshots/` |
| `mise run test:deep` | Deep perft validation (tens of billions of nodes) |
| `mise run test:online` | A full game between two players over the real internet (needs network access) |
| `mise run ci` | Format check + clippy (warnings are errors) + tests |
| `mise run review -- game.pgn` | Review PGN games in the terminal (see [Command-line tool](#command-line-tool-chess-cli)) |
| `mise run cli -- help` | Any `chess-cli` command; `help` prints the full reference |
| `mise run perft -- 6` | Count legal move-tree leaves (`--fen`, `--divide`) |
| `mise run bench -- --depth 14` | Search speed benchmark (`--threads N`) |
| `mise run analyze -- --fen "<FEN>" --depth 20` | Analyse a position in the terminal |
| `mise run lint` / `mise run fmt` | Clippy / rustfmt |
| `mise run games:generate` | Bot-vs-bot games into your library (`--games N --depth D`) |
| `mise run games:list` / `mise run games:clear` | List / delete saved games |
| `mise run cache:clear` | Delete the position cache |

## How it works

Two crates in one Cargo workspace:

- **`crates/engine`** (`chess-engine`): the engine itself, with zero dependencies.
- **`crates/gui`** (`chess-rs`): the desktop app, built on [egui/eframe](https://github.com/emilk/egui),
  with online play over [iroh](https://www.iroh.computer) (`src/online.rs`).

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
- Online play runs on a networking thread of its own. The two apps exchange one line of text per
  event (`MOVE 4 g1f3`, `RESIGN`, `DRAW?`, …), and each app checks every incoming move against
  its own board, so a buggy or modified opponent can't put an illegal move on your board.
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
| Opening book & strength | Every book line is legal and distinct, book moves follow the game and stop when it leaves the book, and Elo estimates never drop as depth rises. |
| Rules | Fool's mate, stalemate, threefold repetition, fifty-move rule, insufficient material, draw by agreement, undo, PGN export. |
| Online | Two players in one process over loopback with the real iroh transport: a game to checkmate, either side hosting, resign, draw offers, leaving, a third player turned away, a wrong code cancelled cleanly, and out-of-order or illegal moves rejected. Codes are case- and dash-insensitive and typos are explained. `test:online` plays the same game through the public internet. |
| PGN | Reads real-world PGN (comments, variations, NAGs, `0-0`, custom FEN starts). Anderssen's *Immortal Game* and Morphy's *Opera Game* must parse and end in checkmate, and writing them back out must be byte-identical. |
| Game Review | lichess win-% and accuracy formulas, every rating band, Forced/Great/Brilliant/Miss detection, the Opera Game review (Morphy's accuracy beats his opponent's), and reviews surviving a round trip through PGN. |
| GUI end-to-end | The real app driven headlessly (egui_kittest): clicking, dragging, en passant, castling, promotion picker, checkmate, undo, hint, keyboard shortcuts, Move now, the live positions counter, cache reuse and persistence across restarts, games autosaved and updated on undo/resign,
the Games tab (list, review, delete with confirmation), Game Review (rates fool's mate's `2.g4`
as a blunder, accuracy cards, eval-graph clicks), reviewing pasted PGN, two apps playing each other online (hosting, joining by typed code, moves, draw offers, resigning, leaving, the saved PGN), move animations (clicked moves slide, dropped pieces don't, castling slides king and rook), Bot vs Bot opening variety, and every mode being a visible button. It also renders screenshots. |

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

## Command-line tool (`chess-cli`)

Everything the engine can do is also available from the terminal, using standard chess formats
(PGN for games, FEN for positions), so it works with games from chess.com, lichess or any other
tool. Run it with `mise run cli -- <command> ...`. Shortcuts exist for the common commands
(`mise run review -- ...`, `mise run analyze -- ...`, `mise run perft -- ...`, `mise run bench -- ...`),
and `mise run cli -- help` prints the full reference.

> File paths are relative to the project folder (where mise runs tasks); use absolute paths for
> files elsewhere. After `mise run build` you can also call the binary directly from anywhere:
> `/path/to/chess-rs/target/release/chess-cli review ~/Downloads/game.pgn`.

### `review`: Game Review for any PGN

```sh
mise run review -- my_game.pgn                      # every game in the file
mise run review -- lichess_export.pgn --game 3      # just the 3rd game
mise run review -- game.pgn --depth 18              # deeper (slower, more precise)
mise run review -- game.pgn --format pgn > annotated.pgn
mise run review -- games/*.pgn --depth 10          # every game you've played in chess-rs
```

This gives the same review as the app: accuracy per player, a rating for every move, and the
better move (with the engine's line) for every inaccuracy, mistake, miss and blunder. Example
(Morphy's Opera Game, abridged):

```
Game 2/2: Paul Morphy vs Duke Karl / Count Isouard (1-0)  Paris FRA 1858.??.??
Reviewed at depth 12, 33 plies

                     White   Black
Accuracy              96.2    85.9
Brilliant (!!)           2       0
Great (!)                5       0
...
Moves (evaluation after the move, from White's side):
    1. e4       Best *           +0.30
  4... Bxf3     Inaccuracy ?!    +1.10   better: Nc6  (Nc6 exd6 Qxd6 Qxd6 Bxd6 Be3)
   10. Nxb5     Brilliant !!     +1.19
 11... Nbd7     Mistake ?        +3.12   better: Kd8  (Kd8 Bxf6 gxf6 Qd5+ Kc8 Ba6+)
   13. Rxd7     Brilliant !!     +5.99
   17. Rd8#     Great !           mate
```

| Option | Meaning | Default |
|---|---|---|
| `<FILE.pgn>...` or `-` | One or more PGN files (each may hold many games), or `-` for stdin | required |
| `--depth <N>` | Analysis depth in plies (higher = slower, more accurate) | 14 |
| `--game <N>` | Only review game N of a multi-game file (counting from 1) | all games |
| `--format text\|pgn` | `text`: the report above. `pgn`: the game with the review embedded | text |
| `--out <FILE>` | Also write the annotated PGN to a file | none |
| `--threads <N>` | Search threads. `1` gives exactly reproducible results | all cores |
| `--hash <MB>` | Transposition table size | 256 |

The annotated PGN stores the review in standard form: `[%eval]` comments after every move,
move-quality symbols (`$4` = `??`, `$2` = `?`, `$6` = `?!`, `$1` = `!`, `$3` = `!!`), and
`WhiteAccuracy`/`BlackAccuracy` tags. lichess and most chess programs can import it.

Getting your games:

- **chess.com**: open a game, then *Share*, *PGN*, *Download*.
- **lichess**: on a game, *Share & export*, *Download PGN*. Or export all your games from your profile.
- **chess-rs**: every game is already in `games/`.

The report goes to stdout and progress to stderr, so `> report.txt` saves just the report.
The command exits with code 1 (and a clear message) if a file can't be read or a move in the PGN is
illegal, naming the bad move.

### `search`: analyse a position

```sh
mise run analyze -- --fen "r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3" --depth 20
mise run analyze -- --pgn game.pgn --depth 18      # the position at the end of the game
```

This prints each completed depth: score (`cp` = centipawns, from the side to move's perspective;
`mate N`), nodes, speed, time and the best line in SAN, then `bestmove`. Options: `--fen`,
`--pgn <FILE>` (with `--game N`), `--depth` (default 12), `--threads`, `--hash`. For debugging,
`--no-pruning` (or `--no-nmp`, `--no-rfp`, `--no-futility`, `--no-lmp`, `--no-lmr`) turns off
the corresponding search shortcuts.

### `mate`: forced-mate solver

```sh
mise run cli -- mate 2 --fen "r2qkb1r/pp2nppp/3p4/2pNN1B1/2BnP3/3P4/PPP2PPP/R2bK2R w KQkq - 1 1"
# mate in 2: Nf6+ (0.00s)
```

This is an exhaustive search with no pruning, so the answer is a proof: it finds the shortest
forced mate up to N moves, or reports `no forced mate in N`. It is useful for checking puzzles. The
cost grows quickly; up to about 4 moves is fast.

### `perft` and `bench`: engine checks

```sh
mise run perft -- 6                    # nodes 119060324 (the standard start-position count)
mise run perft -- 4 --fen "<FEN>" --divide
mise run bench -- --depth 14 --threads 8
```

`perft` counts every legal move sequence to a depth and compares against published numbers,
which makes it the standard test of a move generator. `--divide` breaks the count down by first
move, for tracking down a bug. `bench` measures search speed on 8 fixed positions.
