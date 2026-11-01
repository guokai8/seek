/*
 * seek.h -- C ABI for the seek search core.
 *
 * The core is a Rust static library (libseek_core.a) that provides the
 * filesystem traversal and fuzzy ranking used by the seek macOS app.
 *
 * Ownership
 *   Every `const char *` returned by this API is owned by the Rust side and
 *   stays valid until the next call that returns a string. Copy it into a
 *   Swift String immediately.
 *
 * Threading
 *   All functions are safe to call from any thread. The engine keeps its
 *   state behind a mutex, so a search on the main thread can never observe a
 *   half-applied index update.
 */

#ifndef SEEK_H
#define SEEK_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* The opaque engine handle. */
typedef struct FlEngine FlEngine;

/* A single search result. */
typedef struct {
    uint32_t id;     /* the index entry id                                    */
    int32_t score;   /* the match score                                      */
    int32_t start;   /* byte offset of the first matched character           */
    int32_t end;     /* byte offset just past the last matched character      */
    uint8_t is_dir;  /* 1 when the entry is a directory, 0 otherwise          */
} FlSearchResult;

/* Creates a new engine. Returns NULL on failure. */
FlEngine *sk_engine_new(void);

/* Destroys an engine. The pointer must not be used afterwards. */
void sk_engine_free(FlEngine *engine);

/*
 * Sets the roots to index. The paths are comma-separated; a leading `~` is
 * expanded. Returns 0 on success, -1 on failure.
 */
int sk_engine_set_roots(FlEngine *engine, const char *roots);

/*
 * Sets the exclusion patterns. The patterns are comma-separated globs,
 * matched the way fd's --exclude works. Returns 0 on success, -1 on failure.
 */
int sk_engine_set_excludes(FlEngine *engine, const char *patterns);

/* Sets whether matching is case sensitive. Returns 0 on success, -1 on failure. */
int sk_engine_set_case_sensitive(FlEngine *engine, int case_sensitive);

/*
 * Sets what a bare term means: 1 for an exact substring, 0 for a fuzzy
 * subsequence. This is fzf's --exact. A leading ' on a term always asks for
 * the other one. Returns 0 on success, -1 on failure.
 */
int sk_engine_set_exact_match(FlEngine *engine, int exact);

/*
 * Sets whether accented Latin letters are folded to their base letter.
 * Returns 0 on success, -1 on failure.
 */
int sk_engine_set_normalize(FlEngine *engine, int normalize);

/*
 * Sets the scoring scheme: 0 for the default scheme, 1 for the path scheme.
 * Returns 0 on success, -1 on failure.
 */
int sk_engine_set_scheme(FlEngine *engine, int scheme);

/*
 * Starts a background scan of the configured roots.
 * Returns 0 on success, -1 on failure.
 */
int sk_engine_start_scan(FlEngine *engine);

/*
 * Collects the results of a finished scan into the index and rebuilds the
 * searchable corpus. Returns the number of entries in the index, or -1 on
 * failure. The scan is complete when sk_engine_scan_finished returns non-zero.
 */
int sk_engine_finish_scan(FlEngine *engine);

/* Returns 1 when the background scan has finished, 0 when it is still running.
 *
 * A scan is also finished once it has been given longer than its deadline:
 * opendir() on an unresponsive directory cannot be cancelled, so the index
 * stops waiting rather than never publishing at all. */
int sk_engine_scan_finished(FlEngine *engine);

/* Returns 1 when the last scan walked everything it was asked to, 0 when it
 * was cut short by its deadline and the index is therefore partial. */
int sk_engine_scan_complete(FlEngine *engine);

/* Returns the number of entries in the index. */
int sk_engine_count(FlEngine *engine);

/*
 * Runs a search and writes the results into a caller-provided buffer.
 * Returns the number of results written, or -1 on failure.
 *
 * `capacity` is both the size of the buffer and how many results are
 * wanted: it is the limit the ranking is cut down to.
 *
 * When `matched` is not NULL, it receives how many entries matched in all,
 * which is larger than the return value whenever the matches outgrow the
 * buffer. It is set to 0 on failure and for a query that cannot be parsed.
 */
int sk_engine_search(FlEngine *engine,
                     const char *query,
                     FlSearchResult *out,
                     int32_t capacity,
                     int32_t *matched);

/*
 * The callback a streaming search reports its progress through.
 *
 * `rows` holds the best `count` rows found so far and `matched` is how many
 * candidates have matched so far. `context` is the pointer the search was
 * given. Return 0 to abandon the search, anything else to carry on.
 *
 * `rows` is borrowed for the duration of the call: the next publish writes
 * over the same buffer, so a callback that means to keep the rows must copy
 * them. The callback runs on the thread that called the search, between
 * chunks, so it is not contending with the workers.
 */
typedef int (*FlSearchProgress)(const FlSearchResult *rows,
                                int32_t count,
                                int32_t matched,
                                void *context);

/*
 * Runs a search that reports the best rows it has found as it goes.
 *
 * A search over a quarter of a million paths takes tens of milliseconds, and
 * a panel that shows nothing until it finishes feels like it is thinking.
 * `on_progress` is called every `chunk` candidates with the best rows so far,
 * so the first of them are on screen in a millisecond or two.
 *
 * The ranking is global -- the best match can be the very last candidate --
 * so an intermediate page is PROVISIONAL and a later one may replace it. What
 * never happens is a row that a plain ranking of the candidates seen so far
 * would not have shown.
 *
 * Returning 0 from `on_progress` abandons the search. That is what makes a
 * superseded keystroke cheap: a query the user has already typed past is
 * stopped where it stands. A stopped search also leaves the engine without a
 * pool for the next query, because a partial match set would make the next
 * search lose rows.
 *
 * `chunk` of 0 or less means one chunk, and a NULL `on_progress` means nothing
 * is reported: either way this behaves exactly as sk_engine_search.
 *
 * On return, `out` holds the final rows and the return value is how many --
 * the same answer sk_engine_search gives, and the same as the last publish.
 */
int sk_engine_search_streaming(FlEngine *engine,
                               const char *query,
                               FlSearchResult *out,
                               int32_t capacity,
                               int32_t *matched,
                               int32_t chunk,
                               FlSearchProgress on_progress,
                               void *context);

/*
 * Returns the path of an entry as a newly allocated C string, or NULL when
 * the id is unknown. The caller owns the string and must free it with
 * sk_string_free.
 */
char *sk_engine_path(FlEngine *engine, uint32_t id);

/*
 * Returns the path of an entry relative to its root as a newly allocated C
 * string, or NULL when the id is unknown. This is the path the matcher
 * scored, and it is what `fd` prints. The caller owns the string and must
 * free it with sk_string_free.
 */
char *sk_engine_relative_path(FlEngine *engine, uint32_t id);

/*
 * Resolves the full path and the path relative to the root for a set of
 * results, in one pass.
 *
 * The panel shows a couple of hundred rows, and asking for the paths of
 * each row separately costs one lock acquisition and one walk of the
 * parent chain per row — more than the search itself. This does all of it
 * at once.
 *
 * The paths are written into `out` as null-terminated strings, one after
 * another. The offset of each string is written into `offsets`: the full
 * path of result n first, then the path relative to the root.
 *
 * Returns the number of results that were filled in, or -1 on failure.
 */
int sk_engine_resolve(FlEngine *engine,
                      const uint32_t *ids,
                      int32_t count,
                      char *out,
                      int32_t capacity,
                      int32_t *offsets);

/*
 * Returns the matched byte offsets for a set of results, in one pass.
 *
 * The offsets are written into `out` one result after another, and the
 * number of offsets per result is written into `counts`. The offsets for
 * result n start at out + (the sum of the counts before n).
 *
 * Returns the total number of offsets written.
 */
int sk_engine_positions_batch(FlEngine *engine,
                               const uint32_t *ids,
                               int32_t count,
                               const char *pattern,
                               int32_t *out,
                               int32_t capacity,
                               int32_t *counts);

/*
 * Returns the matched byte offsets for a result. Writes at most `capacity`
 * offsets into `out` and returns the total number of offsets. The offsets
 * refer to the file name of the entry, not to the full path.
 */
int sk_engine_positions(FlEngine *engine,
                        uint32_t id,
                        const char *pattern,
                        int32_t *out,
                        int32_t capacity);

/* Frees a string returned by this library. */
void sk_string_free(char *s);

/* Returns the version of the core library as a static C string. */
const char *sk_version(void);

/*
 * Loads an index snapshot from `path` and installs it in the engine. The
 * snapshot's roots must match the engine's roots, otherwise it is rejected.
 * Returns the number of entries that were loaded, or -1 on failure.
 */
int sk_engine_load_snapshot(FlEngine *engine, const char *path);

/* Writes the engine's index to `path` as a snapshot. Returns 0 on success. */
int sk_engine_save_snapshot(FlEngine *engine, const char *path);

/*
 * Returns the default path of the snapshot file as a newly allocated C
 * string. The caller owns the string and must free it with sk_string_free.
 */
char *sk_engine_default_snapshot_path(void);

/* Returns 1 when a snapshot exists at `path`, 0 when it does not. */
int sk_snapshot_exists(const char *path);

/*
 * A change to the filesystem, reported to the engine by the watcher.
 */
typedef struct {
    /* The path that changed. */
    const char *path;
    /* The kind of change: 0 created, 1 modified, 2 removed, 3 unknown. */
    int kind;
    /* Whether the entry is a directory. */
    int is_dir;
} FlFileChange;

/*
 * Applies a batch of filesystem changes to the index. The corpus is rebuilt
 * afterwards, so that the searches see the new entries. Returns the number
 * of entries in the index, or -1 on failure.
 */
int sk_engine_apply_changes(FlEngine *engine,
                            const FlFileChange *changes,
                            int count);

/*
 * Explains why a query does not parse, so the panel can say what is wrong
 * instead of showing an empty list. Returns 0 when the query is fine,
 * otherwise writes a null-terminated message into `out` and returns its
 * length. Takes no engine, so it never waits on a search or a rebuild.
 */
int sk_query_error(const char *query, char *out, int capacity);

/*
 * Starts watching the engine's roots for changes. Returns 0 on success, -1
 * on failure.
 */
int sk_engine_start_watch(FlEngine *engine);

/*
 * Collects the changes the watchers have reported. The changes are written
 * into `out` as records of the form "kind<TAB>is_dir<TAB>path", one after
 * another, each terminated by a null byte. Returns the number of changes
 * written, or -1 on failure.
 */
int sk_engine_poll_changes(FlEngine *engine, char *out, int capacity);

#ifdef __cplusplus
}
#endif

#endif /* SEEK_H */
