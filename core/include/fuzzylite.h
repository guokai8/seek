/*
 * fuzzylite.h -- C ABI for the fuzzylite search core.
 *
 * The core is a Rust static library (libfuzzylite_core.a) that provides the
 * filesystem traversal and fuzzy ranking used by the fuzzylite macOS app.
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

#ifndef FUZZYLITE_H
#define FUZZYLITE_H

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
} FlSearchResult;

/* Creates a new engine. Returns NULL on failure. */
FlEngine *fl_engine_new(void);

/* Destroys an engine. The pointer must not be used afterwards. */
void fl_engine_free(FlEngine *engine);

/*
 * Sets the roots to index. The paths are comma-separated; a leading `~` is
 * expanded. Returns 0 on success, -1 on failure.
 */
int fl_engine_set_roots(FlEngine *engine, const char *roots);

/*
 * Sets the exclusion patterns. The patterns are comma-separated globs,
 * matched the way fd's --exclude works. Returns 0 on success, -1 on failure.
 */
int fl_engine_set_excludes(FlEngine *engine, const char *patterns);

/* Sets whether matching is case sensitive. Returns 0 on success, -1 on failure. */
int fl_engine_set_case_sensitive(FlEngine *engine, int case_sensitive);

/*
 * Sets the scoring scheme: 0 for the default scheme, 1 for the path scheme.
 * Returns 0 on success, -1 on failure.
 */
int fl_engine_set_scheme(FlEngine *engine, int scheme);

/* Sets the maximum number of results a search returns. Returns 0 on success. */
int fl_engine_set_limit(FlEngine *engine, int limit);

/*
 * Starts a background scan of the configured roots.
 * Returns 0 on success, -1 on failure.
 */
int fl_engine_start_scan(FlEngine *engine);

/*
 * Collects the results of a finished scan into the index and rebuilds the
 * searchable corpus. Returns the number of entries in the index, or -1 on
 * failure. The scan is complete when fl_engine_scan_finished returns non-zero.
 */
int fl_engine_finish_scan(FlEngine *engine);

/* Returns 1 when the background scan has finished, 0 when it is still running. */
int fl_engine_scan_finished(FlEngine *engine);

/* Returns the number of entries in the index. */
int fl_engine_count(FlEngine *engine);

/*
 * Runs a search and writes the results into a caller-provided buffer.
 * Returns the number of results written, or -1 on failure.
 */
int fl_engine_search(FlEngine *engine,
                     const char *query,
                     FlSearchResult *out,
                     int32_t capacity);

/*
 * Returns the path of an entry as a newly allocated C string, or NULL when
 * the id is unknown. The caller owns the string and must free it with
 * fl_string_free.
 */
char *fl_engine_path(FlEngine *engine, uint32_t id);

/*
 * Returns the matched byte offsets for a result. Writes at most `capacity`
 * offsets into `out` and returns the total number of offsets. The offsets
 * refer to the file name of the entry, not to the full path.
 */
int fl_engine_positions(FlEngine *engine,
                        uint32_t id,
                        const char *pattern,
                        int32_t *out,
                        int32_t capacity);

/* Frees a string returned by this library. */
void fl_string_free(char *s);

/* Returns the version of the core library as a static C string. */
const char *fl_version(void);

/*
 * Loads an index snapshot from `path` and installs it in the engine. The
 * snapshot's roots must match the engine's roots, otherwise it is rejected.
 * Returns the number of entries that were loaded, or -1 on failure.
 */
int fl_engine_load_snapshot(FlEngine *engine, const char *path);

/* Writes the engine's index to `path` as a snapshot. Returns 0 on success. */
int fl_engine_save_snapshot(FlEngine *engine, const char *path);

/*
 * Returns the default path of the snapshot file as a newly allocated C
 * string. The caller owns the string and must free it with fl_string_free.
 */
char *fl_engine_default_snapshot_path(void);

/* Returns 1 when a snapshot exists at `path`, 0 when it does not. */
int fl_snapshot_exists(const char *path);

#ifdef __cplusplus
}
#endif

#endif /* FUZZYLITE_H */
