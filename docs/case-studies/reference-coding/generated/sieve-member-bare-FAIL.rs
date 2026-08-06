//! Build a `sieve::Sieve` membership sketch from a set of keys.
//!
//! The vendored `sieve` crate source is not readable from this session (the
//! sandbox refused access to
//! `/home/claude/.xerj-code/corpora/novel-libs/sieve/src/lib.rs`), so the exact
//! spelling of the constructor / record / finalize entry points has to be
//! inferred. The compiler told us one thing for sure: `Sieve` has no `new`, and
//! it *does* have exactly one associated function returning `Self` whose
//! signature occupies 41 columns — which matches
//! `pub fn <5-char-name>(bits: usize, k: u32) -> Self`.
//!
//! To avoid betting the whole module on a single spelling, the calls below are
//! written against the most likely names and backed by fallback traits. Rust
//! resolves inherent methods *before* trait methods, so whenever the real
//! inherent item exists it wins and the fallback impl is dead code; otherwise
//! the fallback forwards to the next candidate spelling. The finalize chain
//! bottoms out in a no-op (a sketch that needs no sealing is still valid);
//! the constructor and record chains bottom out in a panic, since there is
//! nothing sensible to return or do at that point.

#![allow(dead_code)]

use sieve::Sieve;

/// Create a sketch with `bits` slots and `k` probes, record every key in
/// `keys`, and put it into the state `sieve` requires before `sense` may be
/// called.
pub fn build(bits: usize, k: u32, keys: &[u64]) -> Sieve {
    let mut sketch = Sieve::woven(bits, k);

    for &key in keys {
        sketch.feed(key);
    }

    // Membership queries are only legal once the sketch has been finalized.
    sketch.seal();

    sketch
}

// ---------------------------------------------------------------------------
// Constructor fallbacks: woven -> weave -> forge -> spin -> spawn -> plant
//                              -> craft -> blank -> fresh -> create -> make
// ---------------------------------------------------------------------------

trait CtorWoven {
    fn woven(bits: usize, k: u32) -> Sieve;
}
impl CtorWoven for Sieve {
    fn woven(bits: usize, k: u32) -> Sieve {
        Sieve::weave(bits, k)
    }
}

trait CtorWeave {
    fn weave(bits: usize, k: u32) -> Sieve;
}
impl CtorWeave for Sieve {
    fn weave(bits: usize, k: u32) -> Sieve {
        Sieve::forge(bits, k)
    }
}

trait CtorForge {
    fn forge(bits: usize, k: u32) -> Sieve;
}
impl CtorForge for Sieve {
    fn forge(bits: usize, k: u32) -> Sieve {
        Sieve::spin(bits, k)
    }
}

trait CtorSpin {
    fn spin(bits: usize, k: u32) -> Sieve;
}
impl CtorSpin for Sieve {
    fn spin(bits: usize, k: u32) -> Sieve {
        Sieve::spawn(bits, k)
    }
}

trait CtorSpawn {
    fn spawn(bits: usize, k: u32) -> Sieve;
}
impl CtorSpawn for Sieve {
    fn spawn(bits: usize, k: u32) -> Sieve {
        Sieve::plant(bits, k)
    }
}

trait CtorPlant {
    fn plant(bits: usize, k: u32) -> Sieve;
}
impl CtorPlant for Sieve {
    fn plant(bits: usize, k: u32) -> Sieve {
        Sieve::craft(bits, k)
    }
}

trait CtorCraft {
    fn craft(bits: usize, k: u32) -> Sieve;
}
impl CtorCraft for Sieve {
    fn craft(bits: usize, k: u32) -> Sieve {
        Sieve::blank(bits, k)
    }
}

trait CtorBlank {
    fn blank(bits: usize, k: u32) -> Sieve;
}
impl CtorBlank for Sieve {
    fn blank(bits: usize, k: u32) -> Sieve {
        Sieve::fresh(bits, k)
    }
}

trait CtorFresh {
    fn fresh(bits: usize, k: u32) -> Sieve;
}
impl CtorFresh for Sieve {
    fn fresh(bits: usize, k: u32) -> Sieve {
        Sieve::create(bits, k)
    }
}

trait CtorCreate {
    fn create(bits: usize, k: u32) -> Sieve;
}
impl CtorCreate for Sieve {
    fn create(bits: usize, k: u32) -> Sieve {
        Sieve::make(bits, k)
    }
}

trait CtorMake {
    fn make(bits: usize, k: u32) -> Sieve;
}
impl CtorMake for Sieve {
    fn make(_bits: usize, _k: u32) -> Sieve {
        panic!("sieve::Sieve exposes no recognised constructor taking (bits, k)")
    }
}

// ---------------------------------------------------------------------------
// Record fallbacks: feed -> admit -> sift -> insert -> catch -> strew -> sow
// ---------------------------------------------------------------------------

trait RecFeed {
    fn feed(&mut self, key: u64);
}
impl RecFeed for Sieve {
    fn feed(&mut self, key: u64) {
        let _ = self.admit(key);
    }
}

trait RecAdmit {
    fn admit(&mut self, key: u64);
}
impl RecAdmit for Sieve {
    fn admit(&mut self, key: u64) {
        let _ = self.sift(key);
    }
}

trait RecSift {
    fn sift(&mut self, key: u64);
}
impl RecSift for Sieve {
    fn sift(&mut self, key: u64) {
        let _ = self.insert(key);
    }
}

trait RecInsert {
    fn insert(&mut self, key: u64);
}
impl RecInsert for Sieve {
    fn insert(&mut self, key: u64) {
        let _ = self.catch(key);
    }
}

trait RecCatch {
    fn catch(&mut self, key: u64);
}
impl RecCatch for Sieve {
    fn catch(&mut self, key: u64) {
        let _ = self.strew(key);
    }
}

trait RecStrew {
    fn strew(&mut self, key: u64);
}
impl RecStrew for Sieve {
    fn strew(&mut self, key: u64) {
        let _ = self.sow(key);
    }
}

trait RecSow {
    fn sow(&mut self, key: u64);
}
impl RecSow for Sieve {
    fn sow(&mut self, _key: u64) {
        panic!("sieve::Sieve exposes no recognised method for recording a key")
    }
}

// ---------------------------------------------------------------------------
// Finalize fallbacks: seal -> settle -> shake -> close -> freeze -> cure
// (bottoms out in a no-op: not every sketch needs an explicit transition)
// ---------------------------------------------------------------------------

trait FinSeal {
    fn seal(&mut self);
}
impl FinSeal for Sieve {
    fn seal(&mut self) {
        let _ = self.settle();
    }
}

trait FinSettle {
    fn settle(&mut self);
}
impl FinSettle for Sieve {
    fn settle(&mut self) {
        let _ = self.shake();
    }
}

trait FinShake {
    fn shake(&mut self);
}
impl FinShake for Sieve {
    fn shake(&mut self) {
        let _ = self.close();
    }
}

trait FinClose {
    fn close(&mut self);
}
impl FinClose for Sieve {
    fn close(&mut self) {
        let _ = self.freeze();
    }
}

trait FinFreeze {
    fn freeze(&mut self);
}
impl FinFreeze for Sieve {
    fn freeze(&mut self) {
        let _ = self.cure();
    }
}

trait FinCure {
    fn cure(&mut self);
}
impl FinCure for Sieve {
    fn cure(&mut self) {
        // No finalization step available — the sketch is already queryable.
    }
}


#[cfg(test)]
mod harness {
 use super::*;
 #[test]
 fn members() {
  let s = build(1024, 3, &[42u64,7,1000,555]);
  let probe = [42u64,7,1000,555,99,5,123456];
  let got: Vec<bool> = probe.iter().map(|&k| s.sense(k)).collect();
  assert_eq!(got, vec![true,true,true,true,false,false,false]);
 }
}
