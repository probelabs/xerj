fn needs_generation(_: xerj_corpus_publication::Generation) {}

fn main() {
    needs_generation(xerj_corpus_publication::Sequence::new(1));
}
