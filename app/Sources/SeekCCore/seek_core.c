/*
 * seek_core.c -- the translation unit of the core's C target.
 *
 * The Rust core is a static library with a C ABI, so this target only
 * exists to carry the header and the module map to Swift. There is nothing
 * to compile here, but a C target needs at least one source file for
 * SwiftPM to recognise it.
 */

#include "include/SeekCore.h"

/* The core's functions are provided by the Rust static library. */
int sk_core_translation_unit_anchor(void);
int sk_core_translation_unit_anchor(void) {
    return 0;
}
