# fuzzylite

A native macOS launcher that combines the filesystem traversal of
[`fd`](https://github.com/sharkdp/fd) with the fuzzy matching of
[`fzf`](https://github.com/junegunn/fzf) — and ranks the results exactly the
way `fzf` ranks them.

A floating Spotlight-style panel opens with `Cmd+Space`. Every keystroke is
answered from an in-memory index, so results appear in well under a
millisecond.

![fuzzylite](docs/screenshot.png)

## Why

Spotlight indexes the whole disk, which means it is slow to configure, slow to
update, and impossible to change the ranking of. `fuzzylite` indexes exactly
the folders you choose, keeps the index current with FSEvents, and gives you
`fzf`'s ranking — which is very good.

## How it works

```
core/   Rust: traversal, fuzzy matching, ranking, persistence
app/    SwiftUI: the floating panel, the hotkey, the settings
```

The Rust core is compiled into a static library and exposed through a C ABI
(`core/include/fuzzylite.h`). The Swift front end links against it.

### Traversal

The walk is fd's: the `ignore` crate's parallel walker, with `.gitignore`,
`.ignore` and `.fdignore` handling, hidden-file skipping, custom exclusion
globs, and the same `!pattern` convention fd uses for excludes. The walk
runs on its own thread and fills a shared buffer, so the index can be built
while the panel is already usable.

### Matching

The matcher is a port of fzf's `FuzzyMatchV2`: a Smith-Waterman-style dynamic
programming score matrix, the same character classification, the same bonus
matrix for word boundaries, delimiters, camelCase humps and consecutive
matches, and the same Latin accent folding.

The port is verified against the real `fzf` binary by a differential test
that compares the match sets, the scores and the full ranking over tens of
thousands of paths:

```
cargo test --test fzf_parity -- --ignored --nocapture
```

### Ranking

fzf's tiebreak chain is reproduced exactly: score, then length, chunk, path
name, then match position. The sort keys are precomputed during the scan, so
ranking a quarter of a million candidates only compares integers.

### Performance

The search is allocation-free: a reusable scratch buffer holds the dynamic
programming matrix, and the corpus is a flat vector of names that Rayon can
slice without locking.

| corpus | query | fuzzylite | fzf (incl. process start) |
| --- | --- | --- | --- |
| 5,400 paths | `index` | 0.20 ms | 31 ms |
| 5,400 paths | `cargo` | 1.16 ms | 44 ms |
| 259,000 paths | `fuzzylite` | 2.5 ms | — |

### Persistence

The index is snapshotted to `~/Library/Application Support/Fuzzylite/index.flidx`
as a `postcard` payload inside a `zstd` container. The container carries a
format version and a checksum, so a snapshot from an incompatible build is
rejected instead of misread. A snapshot built from different roots is
rejected too, because its entry ids refer to a different tree.

## Building

The Rust core needs a stable toolchain; the app needs Xcode.

```sh
./build.sh          # debug build
./build.sh release  # release build
./bundle.sh         # builds build/Fuzzylite.app
```

`build.sh` relinks the app after the core is rebuilt, because SwiftPM does
not notice when a static library changes.

## Usage

| key | action |
| --- | --- |
| `Cmd+Space` | show / hide the panel |
| `↑` `↓` | move the selection |
| `Home` `End` | jump to the first / last result |
| `Return` | open the result |
| `Cmd+Opt+R` | reveal the result in Finder |
| `Cmd+,` | open the settings |
| `Esc` | close the panel |

### Query syntax

The query language follows fzf's:

| syntax | meaning |
| --- | --- |
| `foo` | fuzzy match |
| `'foo'` | exact substring |
| `^foo` | prefix match |
| `foo$` | suffix match |
| `!foo` | exclude results that match |
| `foo bar` | both terms must match |

## Tests

```sh
cd core
cargo test                                        # unit and integration tests
cargo test --test fzf_parity -- --ignored         # parity with the real fzf
cargo run --release --example bench -- ~/         # search latency
```

The app has an end-to-end check that scans a directory, searches it, writes a
snapshot, restores it and verifies that the ranking is unchanged:

```sh
cd app
swift run Fuzzylite --smoke-test ~/projects
```

## Layout

```
core/src/
  fuzzy/      the fzf port: scoring, bonus matrix, normalisation, ranking
  scan.rs     the fd-style parallel walk
  index.rs    the in-memory index and the incremental updates
  corpus.rs   the flat, searchable form of the index
  search.rs   the parallel search
  query.rs    the query parser
  persist.rs  the snapshot format
  watch.rs    the FSEvents watcher
  ffi.rs      the C ABI

app/Sources/Fuzzylite/
  FuzzyliteApp.swift    the app, the hotkey and the floating panel
  ContentView.swift     the panel's layout
  ResultRow.swift       a result, with the match highlighted
  AppModel.swift        the state and the background work
  SearchEngine.swift    the Swift wrapper around the C ABI
  PreferencesView.swift  the settings window
  SmokeTest.swift       the end-to-end check
```

## Licence

MIT.
