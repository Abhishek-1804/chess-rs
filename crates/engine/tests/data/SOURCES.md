# Test data sources

Public chess test positions used by `tests/suites.rs`. Positions and node
counts are factual data published by the chess-programming community.

| File | Contents | Source |
|------|----------|--------|
| `perft_standard.epd` | 128 perft positions with exact node counts (D1..D6) | Ethereal — https://github.com/AndyGrant/Ethereal/blob/master/src/perft/standard.epd |
| `wac.epd` | Win At Chess, 300 tactical positions (`bm` = best move) | Arasan — https://github.com/jdart1/arasan-chess/blob/master/tests/wacnew.epd |
| `bratko_kopec.epd` | Bratko-Kopec test, 24 positions | sunfish — https://github.com/thomasahle/sunfish/tree/master/tests/files |
| `mate1.fen`, `mate2.fen`, `mate3.fen` | forced mates in 1, 2 and 3 | sunfish (same directory) |
| `stalemate0.fen`, `stalemate1.fen`, `stalemate2.fen` | positions whose correct value is a draw (stalemate now / tricks) | sunfish (same directory) |
| `famous_games.pgn` | The Immortal Game (Anderssen 1851) and the Opera Game (Morphy 1858), public-domain game scores | standard historical record |
| `threefold.epd` | repetition traps: replay `pv`, then `am` must be avoided | sunfish (same directory, `3fold.epd`) |
