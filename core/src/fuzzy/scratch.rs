//! A reusable scratch buffer for the fuzzy matcher.
//!
//! fzf avoids per-candidate allocations by reusing a `util.Slab` of `int16`
//! and `int32` values across every candidate it scores. This module provides
//! the same service: the caller keeps one [`Scratch`] and lends it to the
//! matcher for each candidate.
//!
//! Reusing the buffer is what makes it possible to score hundreds of
//! thousands of candidates per keystroke without the allocator becoming the
//! bottleneck.
//!
//! The fields are separate so that the matcher can borrow all of them at
//! once through a single destructuring.

/// Reusable buffers for the matcher's working memory.
#[derive(Debug, Default)]
pub struct Scratch {
    /// The folded candidate window.
    pub(crate) window: Vec<char>,
    /// The per-position bonus.
    pub(crate) bonus: Vec<i16>,
    /// The first score row.
    pub(crate) h0: Vec<i16>,
    /// The first consecutive-count row.
    pub(crate) c0: Vec<i16>,
    /// The score matrix.
    pub(crate) h: Vec<i16>,
    /// The consecutive-count matrix.
    pub(crate) c: Vec<i16>,
    /// The first occurrence of each pattern character.
    pub(crate) f: Vec<u32>,
    /// The recovered match positions.
    pub(crate) positions: Vec<i32>,
}

impl Scratch {
    /// Creates an empty scratch buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears the buffers, keeping their capacity.
    pub fn clear(&mut self) {
        self.window.clear();
        self.bonus.clear();
        self.h0.clear();
        self.c0.clear();
        self.h.clear();
        self.c.clear();
        self.f.clear();
        self.positions.clear();
    }

    /// The approximate memory held by the buffer, in bytes.
    pub fn memory_usage(&self) -> usize {
        self.window.capacity() * std::mem::size_of::<char>()
            + self.bonus.capacity() * 2
            + self.h0.capacity() * 2
            + self.c0.capacity() * 2
            + self.h.capacity() * 2
            + self.c.capacity() * 2
            + self.f.capacity() * 4
            + self.positions.capacity() * 4
    }
}
