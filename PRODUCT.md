# Product

<!-- impeccable:product-schema 1 -->

## Platform

macOS native (SwiftUI). Not one of the schema's web/ios/android values; this is a
desktop app built with SwiftUI, AppKit, and a Rust static library reached through a
C ABI.

## Stack

- Rust core (`core/`) compiled to a static library, exposed through a C ABI
  (`core/include/seek.h`). Handles traversal, fuzzy matching, ranking, and
  the on-disk index.
- SwiftUI front end (`app/`) built with SwiftPM. The floating panel is an
  `NSPanel` with `NSVisualEffectView` vibrancy, not a regular window.
- The app is an accessory (`LSUIEement = true`): no Dock icon, no menu bar.

## Users

Developers and power users on macOS who already live in the terminal. They use
`fd` and `fzf` daily and know that fzf's ranking is good and Spotlight's is not.
They open a launcher hundreds of times a day, so the panel has to appear instantly
and the results have to be in the order they expect.

## Product Purpose

Replace Spotlight for file and folder search with something that searches exactly
the folders the user chooses, ranks results the way fzf ranks them, and stays
current as the filesystem changes.

Success means: a keystroke produces the right file at the top of the list, and
opening it is one more keystroke.

## Positioning

Two mechanisms a neighbouring product could not truthfully copy:

1. **fzf's ranking, exactly.** The matcher is a port of fzf's `FuzzyMatchV2`,
   including the bonus matrix, the character classification, accent folding, and
   the tiebreak chain (score, then length, chunk, pathname, begin, end). It is
   verified against the real `fzf` binary by a differential test that compares
   match sets, scores, and full ranking order.
2. **The user's own filesystem, indexed live.** Only the folders the user names
   are indexed, and an FSEvents watcher keeps the index current, so a file
   created after launch appears without a restart.
3. **The search covers the path, the way fzf's does.** The matcher scores the
   whole path, not just the file name. The path is what the result row shows,
   so a search that could not find what it displays would be a search that
   lies. The path-aware scoring scheme only means something because the haystack
   contains the separators.

## Operating Context

- The launcher is summoned with `Cmd+Space`, which requires the accessibility
  permission. The app asks for it on first launch.
- The panel floats above everything, disappears on `Esc`, and returns focus to
  the previous app.
- The index is snapshotted to `~/Library/Application Support/Seek/index.skidx`
  (postcard + zstd), so a warm launch is instant.
- Searches run off the main actor; the panel stays responsive while a quarter of
  a million files are being matched.
- The default root is the home directory, with `Library`, `.Trash`, `.cache`,
  `.npm`, `.cargo`, `node_modules`, `.git`, and `Applications` excluded.

## Capabilities and Constraints

Confirmed:

- `Cmd+Space` toggles the panel; `↑`/`↓` navigate, `Home`/`End` jump, `Return`
  opens, `Cmd+Opt+R` reveals in Finder, `Cmd+Shift+C` copies the path,
  `Cmd+,` opens settings, `Cmd+Q` quits, `Esc` closes.
- The query language follows fzf's: fuzzy terms, `'exact'`, `^prefix`, `suffix$`,
  `!exclude`, and space-separated terms that must all match.
- Matching can be case sensitive or accent folding can be turned off.
- Two scoring schemes: path-aware (default) and default.
- The settings window edits the indexed folders, the exclusion globs, and the
  matching options. Changes take effect on the next launch, and the window
  offers a relaunch button and a quit button, because the app has no Dock icon
  and no menu bar to offer them anywhere else.

Technical constraints:

- The search must stay sub-millisecond on a large corpus; the matcher is
  allocation-free and the corpus is a flat vector that Rayon can slice.
- The engine state is behind a reader-writer lock. Searches take the read lock
  and run in parallel; index rebuilds take the write lock.
- The index must survive a crash mid-write, so the snapshot container carries a
  version and a checksum.
- The panel's own keys are intercepted with a local event monitor, not with a
  background `NSView`. A background view is not part of the responder chain, so
  it never sees the key presses at all — the search field consumes the arrow
  keys as text cursor movement before anything else can act on them.

Open decisions:

- The `content:` operator is not implemented. The setting is shown disabled and
  labelled "not available yet", rather than as a control that silently does
  nothing.

## Evidence on Hand

- `core/tests/fzf_parity.rs` — differential test against the real `fzf` binary.
- `core/tests/path_matching.rs` — the search covers the whole path, not just
  the file name.
- `core/tests/concurrency.rs` — searches running while the index is rebuilt.
- `core/tests/live_index.rs` — a file created after the scan is found.
- `app/Tests/SeekTests/PanelKeyTests.swift` — the panel's shortcuts map to
  the right actions, and a typed character is not swallowed by them.
- `app/Tests/SeekTests/KeyboardRoutingTests.swift` — the arrow keys move
  the selection, and the panel opens with an empty query.
- `app/Tests/SeekTests/ChangeParsingTests.swift` — the watcher's records
  are read out of the buffer correctly.
- `core/examples/bench.rs` — search latency, currently 0.2–1.2 ms on a
  5,400-path corpus versus fzf's 31–55 ms including process startup.
- `app/Sources/Seek/SmokeTest.swift` — end-to-end scan, search, snapshot,
  restore, and ranking-stability check.

Absences that future work must not fabricate: no user testimonials, no adoption
numbers, no performance claims beyond the measured figures above.

## Product Principles

1. **The keyboard is the whole interface.** Every action has a shortcut; the
   mouse is optional. Nothing may require a pointer to reach its primary function.
2. **fzf's ordering is the contract.** If the ranking differs from fzf's, that
   is a bug, not a design choice.
3. **Never show a stale answer.** The index is either current or the panel says
   it is indexing.
4. **Search only what the user asked for.** No hidden roots, no surprising
   default scope.
5. **The panel must never be the bottleneck.** No visible work happens on the
   main actor that could happen off it.

## Accessibility & Inclusion

- The global hotkey requires macOS accessibility access; the app requests it
  explicitly on first launch rather than silently failing.
- Full keyboard operation of the panel and the settings window.
- Matched characters in a result are distinguished by weight as well as colour,
  so the highlight does not depend on colour perception alone.
