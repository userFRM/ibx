//! A quote a reader can take without waiting for the writer.

use super::*;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use crate::types::*;

/// SeqLock-protected quote slot. Writer (hot loop) never blocks.
/// A quote published by the hot loop and read by any number of consumers.
///
/// The version counter is the freshness test: odd means a write is in flight,
/// and a reader that sees the same even value on both sides of its snapshot
/// took a whole one. The payload itself is held as words rather than a plain
/// copy of the struct, so the concurrent read and write are both defined
/// operations — a version counter can discard a torn snapshot but cannot make
/// the racing access that produced it legal.
#[repr(align(64))]
pub struct SeqQuote {
    version: AtomicU64,
    data: [AtomicI64; QUOTE_WORDS],
}

impl Default for SeqQuote {
    fn default() -> Self {
        Self::new()
    }
}

impl SeqQuote {
    /// An empty one.
    pub fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            data: std::array::from_fn(|_| AtomicI64::new(0)),
        }
    }

    /// Write a quote (hot loop side). Never blocks.
    #[inline]
    pub fn write(&self, quote: &Quote) {
        // AcqRel, not Release: the payload writes below must not be reordered
        // above this store. Release alone only fences what precedes it; the
        // Acquire half is what pins *following* accesses inside the odd window.
        self.version.fetch_add(1, Ordering::AcqRel); // odd = writing
        // And the reader needs something to pair with. Not reordering the
        // payload writes is a statement about this thread; a reader that took
        // one of them still has no edge back to the counter that says a write
        // was in flight, because a relaxed store is not a release operation
        // and its acquire fence synchronises with nothing. Left out, a reader
        // may take a word from this pass and then read the version it started
        // with, and the torn snapshot passes a counter that never moved. This
        // is what the reader's fence pairs with. Free on x86-64.
        std::sync::atomic::fence(Ordering::Release);
        for (slot, word) in self.data.iter().zip(quote_to_words(quote)) {
            slot.store(word, Ordering::Relaxed);
        }
        self.version.fetch_add(1, Ordering::Release); // even = stable
    }

    /// Read a consistent quote snapshot (reader side). Spins on conflict.
    #[inline]
    pub fn read(&self) -> Quote {
        loop {
            let v1 = self.version.load(Ordering::Acquire);
            if v1 & 1 != 0 { continue; } // writer active
            let mut words = [0i64; QUOTE_WORDS];
            for (word, slot) in words.iter_mut().zip(self.data.iter()) {
                *word = slot.load(Ordering::Relaxed);
            }
            // The fence is what makes the check mean anything: an Acquire
            // load constrains what comes after it, so without this the payload
            // reads above may be satisfied after the version read below and a
            // torn snapshot would pass a counter that never moved.
            std::sync::atomic::fence(Ordering::Acquire);
            let v2 = self.version.load(Ordering::Relaxed);
            if v1 == v2 { return quote_from_words(words); }
        }
    }
}
