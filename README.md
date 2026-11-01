# seek

A native macOS launcher that combines the filesystem traversal of
[`fd`](https://github.com/sharkdp/fd) with the fuzzy matching of
[`fzf`](https://github.com/junegunn/fzf) — and ranks the results exactly the
way `fzf` ranks them.

A floating Spotlight-style panel opens with `Ctrl+Space`. Every keystroke is
answered from an in-memory index, so results appear in well under a
millisecond.

## Why

Spotlight indexes the whole disk, which means it is slow to configure, slow to
update, and impossible to change the ranking of. `seek` indexes exactly
the folders you choose, keeps the index current with FSEvents, and gives you
`fzf`'s ranking — which is very good.

## How it works

```
core/   Rust: traversal, fuzzy matching, ranking, persistence
app/    SwiftUI: the floating panel, the hotkey, the settings
```

The Rust core is compiled into a static library and exposed through a C ABI
(`core/include/seek.h`). The Swift front end links against it.

### Traversal

The walk is fd's: the `ignore` crate's parallel walker, with `.gitignore`,
`.ignore` and `.fdignore` handling, hidden-file skipping, custom exclusion
globs, and the same `!pattern` convention fd uses for excludes. The walk
runs on its own thread and fills a shared buffer, so the index can be built
while the panel is already usable.

It differs from fd in two ways, both because this runs all day rather than
once. It stays on the filesystem each root lives on — fd spells that
`--one-file-system` and leaves it off, but a network share, a disk image or
a file provider's volume mounted inside your home directory is not part of
your disk, and `opendir()` on an unresponsive one blocks in the kernel with
no way to cancel it. And the walk carries a deadline: when it passes, the
entries collected so far are published and the footer says the scan was cut
short, rather than the index never appearing at all.

### Matching

The matcher is a port of fzf's `FuzzyMatchV2`: a Smith-Waterman-style dynamic
programming score matrix, the same character classification, the same bonus
matrix for word boundaries, delimiters, camelCase humps and consecutive
matches, and the same Latin accent folding.

An exact term is a port of fzf's `exactMatchNaive` over the same bonus
matrix, so the two kinds of term are scored on one scale and a query can
mix them.

The haystack is the path relative to the indexed root — the same thing `fd`
prints — so typing a folder name finds the files inside it, and the
path-aware scoring scheme has the separators it needs to reward a match that
lands on a folder boundary.

The port is verified against the real `fzf` binary by a differential test
that compares the match sets, the scores and the full ranking over tens of
thousands of paths:

```sh
cd core
cargo test --test fzf_parity -- --ignored --nocapture
cargo test --test app_path_parity -- --ignored --nocapture
```

The second one goes through the query parser and the path scheme the panel
actually runs, in both matching modes, against `fzf` and `fzf -e`.

### Ranking

fzf's tiebreak chain is reproduced exactly: score, then length, chunk, path
name, then match position. The sort keys are precomputed during the scan, so
ranking a quarter of a million candidates only compares integers.

### Performance

The search is allocation-free: a reusable scratch buffer holds the dynamic
programming matrix, and the paths live in one contiguous arena that Rayon
can slice without locking. The matched positions are only collected for the
results that are actually shown, because collecting them for every candidate
costs more than the matching itself. Each row of the matrix is taken as a
slice of its own so that the inner loop carries no bounds checks, the score
to the left is carried in a register rather than read back, and the matrix
is not cleared between candidates — only the few cells the loop will not
write are zeroed, because the rest is about to be overwritten anyway.

Measured over the home directory — 187,414 files, 208,164 entries — with fzf
given the same candidate list:

| query | matches | seek | fzf |
| --- | --- | --- | --- |
| `src` | 119,013 | 6.6 ms | 105 ms |
| `cargo` | 29,422 | 7.8 ms | 34 ms |
| `index` | 37,032 | 5.7 ms | 39 ms |
| `test` | 167,203 | 10 ms | 151 ms |
| `te` | 196,684 | 9.3 ms | 156 ms |
| `s` | 205,652 | 9.4 ms | 153 ms |
| `seek` | 132 | 1.4 ms | 2.2 ms |

Reproduce it with `cargo run --release --example bench`.

A broad query matches far more paths than the panel shows, so nearly every
match is scored, ranked and discarded. Most of that work is skipped. No
match can score above a ceiling — every character worth a match plus the
best bonus the scheme pays, the first one's counted twice, and no gaps.
Once enough candidates have turned up at that ceiling, anything that
provably cannot reach it is counted without being scored.

Two things take a path out of the running. A path that does not hold the
query as one consecutive run pays at least one gap, and a gap costs more
than the slack the prune leaves. And a run is not enough on its own:
the ceiling pays the best bonus for *every* character, the best bonus is
only earned by a character following a path separator, and a run's later
characters inherit the bonus its first character got — so a run in the
middle of a word is one bonus short per character, which for any query
of two characters or more is again more than the slack.

That second half is what reaches the queries that hurt. `te` matches
196,684 paths here and 89% of them hold it as a run, so asking only
whether the run exists dismisses one match in nine; asking where it sits
dismisses seven in ten, and the keystroke goes from 20.6 ms to 10.3 ms.

For a query whose best rows sit below the ceiling the prune never arms,
and costs one relaxed atomic load per candidate. It is left off for a
query of several terms, whose score is their sum and so is not bounded by
any one term's ceiling; for anything but plain ASCII, where a byte-wise
test and the matcher's accent folding ask different questions; for a
single-character query, where "contains" and "matches" are the same
question so the search could only confirm a non-match; and for a query
that carries its own boundary, such as `/b`, whose `/` pays the best bonus
to the character after it wherever the run happens to sit.

A query that extends the one before it is answered from what that one
matched, rather than from the whole corpus — fzf does the same, and it is
why typing can get faster as the query gets longer. The rule for when that
is allowed is conservative, because several ways of extending a query in
this language *widen* what it matches: `!na` → `!nat` excludes less, and
`re` → `re:a` turns two fuzzy characters into a regular expression. A pool
that covers more than half the corpus is not kept, because reusing one that
large was measured to save two tenths of a millisecond.

Narrowing helps where the query is selective and not where it is not, and
the panel's default matching is what makes it selective. A bare term is a
substring here, so its pool is far smaller than the same term's as a
subsequence: typing `readme`, the last keystroke costs 0.33 ms against the
1,025 paths that `readm` matched, where fuzzy matching has 47,160 of them
and spends 5.6 ms. Typing `test`, a pool appears at the third keystroke and
the fourth costs 2.8 ms instead of 8.8 ms — in fuzzy matching nothing
narrows there at all, because every prefix of it is a subsequence of more
than half the corpus. What such a query spends its time on is the scoring
of the paths that *match*, and those are all in the pool by definition —
which is why the ceiling bound above, and not narrowing, is what made
fuzzy `test` fast.

A query that matches most of the corpus spends its time somewhere else
again. `s` matches 205,652 paths, and matching them is only 3 ms of it:
the rest is computing a sort key for each one and then picking 500 out of
the lot. The picking is kept proportional to what actually competes — a
candidate that cannot beat the row at the bottom of the page is never
even put in the running — and the row at the bottom of the page is
recomputed as the page fills, not once per chunk. Fixing it once per
chunk is the same thing as not having it at all when there is one chunk,
which is what a plain search is; that cost 2.6 ms of `s`'s 12.

### Results while it searches

The ranking is global, so a broad query cannot be answered until the last
candidate has been looked at — `te` matches 196,680 paths over a home
directory and takes 19 ms. The search therefore works in chunks and hands
over the best rows it has after each one. The first page arrives in 2 to 4
ms, which is the point: what reaches the screen first is not a prefix of
the answer, it is the answer to a prefix of the corpus.

Every page is exactly the top rows of the candidates seen so far, and the
last page is what a plain search returns. A page is provisional and cannot
be frozen early: a candidate found later with the same score but a shorter
path displaces one found earlier, because length is the next tiebreak. So
the rows can still change until the search finishes — which is also why the
footer reports `N so far` until it has the real count, rather than a
number that climbs through every search.

The panel's side of it is cheap because an intermediate page resolves only
32 rows — the paths and the highlight offsets are the expensive half, and
the panel can show 14. The chunking itself costs between −1 and +1.5 ms
over the whole search.

What this is really for is cancelling. A search that the next keystroke has
already superseded returns `0` from the callback and is abandoned, which
takes `te` from 19.3 ms to 4.1 ms. A search that stops early hands back no
pool, because the pool is the query's *complete* match set and the next
keystroke searches it instead of the corpus — a partial one would silently
lose rows.

### Persistence

The index is snapshotted to `~/Library/Application Support/Seek/index.skidx`
as a `postcard` payload inside a `zstd` container. The container carries a
format version and a checksum, so a snapshot from an incompatible build is
rejected instead of misread. A snapshot built from different roots is
rejected too, because its entry ids refer to a different tree.

### Staying current

An FSEvents watcher runs over the indexed roots. The changes it reports are
applied to the index as they arrive, so a file created after the launch
shows up without a restart. The watcher signals that it is running before
the app continues, so that a change made right after startup is not
missed.

## Building

The Rust core needs a stable toolchain; the app needs Xcode.

```sh
./build.sh          # debug build
./build.sh release  # release build
./bundle.sh         # builds build/Seek.app
```

`build.sh` relinks the app after the core is rebuilt, because SwiftPM does
not notice when a static library changes.

`build.sh` builds for the machine it runs on, because that is the one you
wait for on every change. `bundle.sh` is the one that ships, so it builds
**universal** — `arm64` and `x86_64`, both for **macOS 14 and later**. It
builds the core once per target, joins the two static libraries with `lipo`,
and hands the universal archive to a single `swift build --arch arm64 --arch
x86_64`. Afterwards it asks the binary what it is: a missing slice or a slice
built for the wrong minimum fails the build, because either one looks like a
working build right up until the app is copied to the other kind of Mac.

The Intel target has to be installed once:

```sh
rustup target add x86_64-apple-darwin
```

## Usage

| key | action |
| --- | --- |
| `Ctrl+Space` | show / hide the panel (changeable in the settings) |
| `↑` `↓` | move the selection |
| `Home` `End` | jump to the first / last result |
| `Return` | open the result |
| `Tab` | show / hide the preview column |
| `Cmd+E` | switch between exact and fuzzy matching |
| `Cmd+Opt+R` | reveal the result in Finder |
| `Cmd+Shift+C` | copy the path |
| `Cmd+,` | open the settings |
| `Cmd+Q` | quit |
| `Esc` | close the panel |

### Settings

The app is a background agent: it has no Dock icon and does not appear in the
`Cmd+Tab` switcher. It lives in the menu bar instead — the magnifying glass
there says the app is running, says what the hotkey is, and reports how many
files are in the index. The glass is dimmed while the index is still being
built.

The settings are reached from that menu, with `Cmd+,` while the panel is open,
or with the gear button in the bottom right of the panel.

| setting | meaning |
| --- | --- |
| Shortcut | the key that summons the panel. Click **Change…** and press the keys. `Ctrl+Space` is the default; `Cmd+Space` belongs to Spotlight. |
| Indexed folders | the folders that are searched, one per line |
| Excluded patterns | the globs that are left out, the way `fd --exclude` leaves them out |
| Matching | exact or fuzzy, case sensitivity, accent folding, and the scoring scheme |
| Hide after | how long the panel stays open when it is left alone. `Never` keeps it open until it is closed by hand. |
| Show the preview column | whether the panel opens with the preview showing. `Tab` toggles it without coming here. |

### The preview column

`Tab` opens a QuickLook preview of the selected file beside the results,
and the panel widens from 720 to 1080 points to hold it. It is off by
default: the panel is summoned hundreds of times a day to type three
letters and press return, and a panel half again as wide for all of them
would be paying for a preview nobody asked to see. The setting is
remembered.

The panel keeps its top edge and its horizontal centre as it widens, and
is pulled back onto the screen if the column would push it over an edge —
`PanelPlacement`, which is a pure function so that the arithmetic can be
checked without a window and a display. The column is drawn only when
there is a row to follow, so an empty query looks exactly as it does
without the preview. While the column is open the panel stops sizing
itself to its results and takes its full height instead: three rows beside
a 60pt preview box is a preview nobody can read.

The target lags the selection by 120ms. Every file the column is pointed
at is a QuickLook load, and somebody holding the down arrow walks through
twenty rows in well under a second; the delay means only the row they
stopped on is ever loaded. The size and date in the header are read off
the main actor, so a file on a network share that has gone away cannot
freeze the panel.

Which half draws the file is decided per file. An image, a PDF, a video
and a spreadsheet go to QuickLook, so they look the way they do under the
Finder's space bar and nothing written here has to do it worse. Text is
read and drawn by the panel, because QuickLook is the wrong answer for it:
macOS ships no Markdown generator, so a `.md` file previews as its type
icon — and over a corpus of notes and source files that is the most common
file in the list showing nothing.

A file is text when its declared type says so and its bytes agree. The
type is asked first because a PDF's head can be plain ASCII for pages at a
time, and the bytes alone would draw a screen of `%PDF-1.4` and object
definitions. A file whose type is not known — `Makefile`, `LICENSE`,
`.gitignore`, which a launcher lands on constantly — has nothing but its
bytes to go on: no NUL byte in the head and a clean UTF-8 decode, or it
goes to QuickLook. Rich text and HTML are left to QuickLook too; their
formatting is the point of them.

Only the head is read: 64KB, cut to 400 lines. Both ceilings do work. A
log file is gigabytes and the column shows its first screenful either way,
while 64KB of one-letter lines is sixty thousand of them, which stalls
SwiftUI on a row the user only arrowed past.

The shortcut is registered with Carbon's `RegisterEventHotKey`, the same
mechanism macOS uses for its own shortcuts. It therefore works without the
accessibility permission, and it works while another app is in front.

### Query syntax

fzf's operators, plus the two things `fd` can do that fzf cannot:

| syntax | meaning |
| --- | --- |
| `foo` | exact substring, by default — fzf's `--exact` |
| `*.pdf` | glob — any term holding `*` or `?` |
| `re:^src/.*\.rs$` | regular expression |
| `'foo` | the other matching mode, for this term — fzf's own operator, no closing quote |
| `'foo bar'` | exact substring, closed so it can hold a space |
| `^foo` | prefix match |
| `foo$` | suffix match |
| `^foo$` | the whole path, and nothing else |
| `!foo` | exclude what it matches |
| `foo bar` | both terms must match |

Every term is matched against the path relative to its root — the same text
the matcher scores and the panel shows. A glob with no `/` is given an
implicit `**/`, so `*.pdf` finds `docs/report.pdf`; one that does have a `/`
is matched as written, and `*` stops at a directory boundary (`src/*.rs` is
the files directly under `src`, not everything beneath it).

An anchored term is scored, not merely obeyed. fzf's `PrefixMatch`,
`SuffixMatch` and `EqualMatch` all end in the same scoring function its
fuzzy matcher ends in, so `rs$` puts `src/index/rs` above `src/main.rs`:
the run opens on a directory separator in the first and on a dot in the
second, and the path scheme pays more for the separator. A term that
only decided whether a path was in the running would leave every candidate
on score zero, and the order would fall through to the tiebreaks — a list
fzf does not stand behind.

An inverted term is an exact substring whatever the mode: `!ab` keeps
`axb`, here and in fzf, because an exclusion is a blunt instrument and a
fuzzy one throws out far more than the characters typed suggest. `!'ab` is the
fuzzy exclusion, for when that is what is wanted.

A plain term is an exact substring: `nihao` is those five characters with
nothing between them. fzf's default is the other way round — a plain term
is a subsequence there, so `fzf --filter=nihao` matches
`Hao Law NIW EB1 Documents List.pdf` and 174,503 other paths over this
home directory — and `seek` takes fzf's own `--exact` as its default
instead. A launcher is opened to type three letters and press return, and
a list of a hundred and seventy thousand rows is not an answer.

Both modes are a keystroke away. `Cmd+E` switches the panel's own and
re-answers the query on screen at once; the pill beside the field says
which one is on; the setting is **Exact matching**. For a single term,
`'nihao` asks for the other mode — a leading quote with nothing closing
it, which is the operator fzf has, and which fzf's `--exact` inverts in
exactly the same way. The closing quote is this project's own addition,
and all it adds is the ability to put a space inside the term; it stays
exact in either mode, since grouping a phrase is what it is for.

An exact term is scored, not merely filtered: fzf picks the occurrence
whose first character sits on the best boundary and runs the same scoring
function it ends a fuzzy match with, so `src` ranks `app/src/main.rs`
above `unsrcoped.txt`. The panel underlines that run and no other
characters.

Glob needs no operator because `*` and `?` do not occur in real paths, so a
term holding one can only have been meant as a glob. A regex does need one:
its syntax is made of characters that are ordinary in a filename, and `.`
alone would turn every query into a regex.

`!` carries the kind of the term it negates, so `!*.tmp` excludes what the
glob matches. A pattern that does not compile is reported in the panel
rather than answered with an empty list.

## Tests

```sh
cd core
cargo test                                        # unit and integration tests
cargo test --test fzf_parity -- --ignored         # parity with the real fzf
cargo run --release --example bench -- ~/         # search latency

cd ../app
swift test                                        # the app's own tests
```

The tests cover the parts that are easy to get subtly wrong: the matching
against the real `fzf`, the concurrency between the searches and the index
updates, the watcher picking up a file that is created after the scan, the
search covering the whole path, the panel's shortcuts, the parsing of the
watcher's records, the pages a streaming search hands over, the narrowing
of a search to the previous query's matches — which is compared with a cold search field by field, because a
candidate missing from the pool would be a row that silently stops
appearing — and the bound that lets a match go unscored.

That bound gets two kinds of test, because getting it wrong drops rows
while leaving nothing in the output to say so. One asserts the bound itself
over a whole corpus: no match above the ceiling, nothing without a
consecutive run within a gap of it, and nothing whose run sits away from a
separator within a gap of it either — plus the converse, that a path
scoring the ceiling always does hold a run at a separator, since a bound
that held in one direction only would either drop rows or prune nothing.
The pattern that breaks the boundary half is in there as a test of its
own, scoring two below the ceiling in the middle of a name, which is why
the search refuses to prune on a boundary for a query like it. The other
kind compares the rows the search returns with the rows a plain sequential
ranking of the same paths gives, field by field — that ranking scores every
candidate and sorts the lot, so it has nothing to prune and makes an honest
oracle. It runs over two corpora: one where the matches do not hold the
query at all, and one where every single match holds it in the middle of a
word, which is the population the boundary half exists for. The search also
reports how many of its matches it scored, because a prune that quietly
scored everything anyway would leave the rows untouched and the clock the
only evidence.

Streaming gets the same treatment, for the same reason: a page that is not
the top of what has been seen is a row the user never gets offered. The
fixture puts the best matches *last* on purpose, with a precondition test
asserting that it does, and each page is compared field by field with a
plain sequential ranking of the paths the search had reached by then. The
C ABI is driven through a real function pointer, including the case that a
search abandoned halfway must not leave the engine remembering a partial
pool. One thing is deliberately *not* asserted: how many of its matches a
streaming search scored. The prune's counter is shared between the worker
threads, so which candidates escape being scored depends on the
interleaving — the rows do not vary, that number does, and a test pinning
it would be asserting a determinism the design refuses to provide.

The app has an end-to-end check that scans a directory, searches it, writes a
snapshot, restores it and verifies that the ranking is unchanged:

```sh
cd app
swift run Seek --smoke-test ~/projects
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
app/Sources/Seek/
  SeekApp.swift          the app, the panel, the hotkey, the menu bar item
  ContentView.swift      the panel's layout
  ResultRow.swift        a result, with the match highlighted
  Theme.swift            the colours, the type scale and the shapes
  AppModel.swift         the state and the background work
  SearchEngine.swift     the Swift wrapper around the C ABI
  HotKeyManager.swift    the global hotkey, registered with Carbon
  KeyboardHandler.swift  what each key in the panel does
  PanelPlacement.swift   where the panel goes on the screen
  PreviewPane.swift      the preview column
  PreviewController.swift  what the column is pointed at
  PreferencesView.swift  the settings window
  SmokeTest.swift        the end-to-end check
```

## Licence

MIT.
