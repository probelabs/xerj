fn needs_manifest(_: xerj_corpus_publication::ManifestDigest) {}
fn main() { let plan: xerj_corpus_publication::DesiredPlanDigest = "xerdp1-sha256-0000000000000000000000000000000000000000000000000000000000000000".parse().unwrap(); needs_manifest(plan); }
