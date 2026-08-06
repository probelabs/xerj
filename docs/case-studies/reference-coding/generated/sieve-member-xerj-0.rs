pub fn build(bits: usize, k: u32, keys: &[u64]) -> sieve::Sieve {
    let mut s = sieve::Sieve::mesh(bits, k);
    for &key in keys {
        s.dust(key);
    }
    // `sense` panics unless the sieve has been sealed first.
    s.settle();
    s
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
