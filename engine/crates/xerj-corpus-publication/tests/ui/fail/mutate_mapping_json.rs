use xerj_corpus_publication::MappingJsonBytes;

fn mutate(bytes: &mut MappingJsonBytes) {
    bytes.canonical_json()[0] = b'!';
}

fn main() {}
