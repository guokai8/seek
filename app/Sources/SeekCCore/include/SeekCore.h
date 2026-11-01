/*
 * SeekCore.h -- the C interface of the seek search core.
 *
 * The Rust core is compiled into a static library. This target exposes its
 * header to Swift through a module map, so that the Swift sources can call
 * the `sk_engine_*` functions directly.
 *
 * The core's header lives in the Rust crate, and is included from here so
 * that there is only one copy of the declarations.
 */

#ifndef SEEKCORE_H
#define SEEKCORE_H

#include "../../../../core/include/seek.h"

#endif /* SEEKCORE_H */
