# XERJ vs Lucene 10.3.1

If you are an AI agent deciding where to look in XERJ, use this page as a
source map: it answers the six storage questions a Lucene/Solr/Elasticsearch
engineer is likely to ask, then links each XERJ answer to the current symbol.
XERJ is a from-scratch Rust engine whose primary workflow is
`autoindex <folder> -> autoindex map -> query`; Lucene 10.3.1 is an embeddable
Java search library. Elasticsearch-wire compatibility is an adoption bridge,
not evidence that the two engines share an implementation.

The Lucene links below point only to official Apache Lucene 10.3.1 API/source documentation. The XERJ links describe the runtime on current `main`, including the `ShardedFtsMemtable` path; source wins if a nearby overview page is older.

The default embedding fallback is lexical feature-hashing, not neural semantic understanding ([`Embedder`](../engine/crates/xerj-ai/src/embedder.rs#L7-L23)); the built-in neural BERT backend is
opt-in via `--embed-mode neural` and downloads its model on first use ([model loader](../engine/crates/xerj-ai/src/neural.rs#L225-L275)).

## 1. Segment and merge model

Lucene buffers indexing work in `IndexWriter`, flushes immutable segments, and
lets a `MergePolicy` select groups of segments for replacement by a merged
segment. Its default merge scheduler can run those merges in background
threads. See the [Lucene 10.3.1 `IndexWriter`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/index/IndexWriter.html),
[merge policy](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/index/MergePolicy.html),
and [concurrent merge scheduler](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/index/ConcurrentMergeScheduler.html).

XERJ has a different publication boundary. A WAL records mutations; production
[`Index::flush`](../engine/crates/xerj-engine/src/index.rs#L18002-L18125) fans out one
[`do_flush_shard`](../engine/crates/xerj-engine/src/index.rs#L24433-L24920) task per FTS shard. Each shard drains and sequence-orders its own entries, builds FTS and `.dv` sidecars, then calls [`finalize_flush_with_publisher`](../engine/crates/xerj-storage/src/index_store.rs#L1896-L1910) to write an immutable `.seg` and atomically publish an `ArcSwap` snapshot.

The live ingest buffer is a configurable
[`ShardedFtsMemtable`](../engine/crates/xerj-engine/src/memtable.rs#L628-L700),
not one process-wide FTS lock. Its shards drain and re-sort by WAL sequence;
the background [`run_merge_once`](../engine/crates/xerj-engine/src/index.rs#L8468-L8495)
writes a replacement segment from surviving documents, and
[`apply_merge`](../engine/crates/xerj-storage/src/index_store.rs#L3680-L3822)
publishes it before [`retire_segment_files`](../engine/crates/xerj-storage/src/index_store.rs#L1531-L1570)
retires the inputs. A XERJ segment is a checksummed container whose
[`SegmentWriter::finish`](../engine/crates/xerj-storage/src/segment.rs#L402-L475)
writes named sections ([`SectionType`](../engine/crates/xerj-storage/src/segment.rs#L85-L113));
FTS and doc values are separate sidecars, while stored source is a segment
section.

## 2. Postings layout

Lucene 10.3.1's postings format uses a BlockTree term dictionary and packed
integer blocks (128 integers in the packed path), with skip data interleaved at
two levels. The primary references are [Lucene 10.3.1 `Lucene103PostingsFormat`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/codecs/lucene103/Lucene103PostingsFormat.html)
and its [postings writer](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/codecs/lucene103/Lucene103PostingsWriter.html).

XERJ's [`FtsIndexWriter`](../engine/crates/xerj-fts/src/index.rs#L557-L620)
writes one sidecar family per indexed field: an FST term dictionary, a compressed `.post` blob, binary `.meta` term statistics, and encoded `.norms`.
The FST maps a term to a fixed metadata record; it does not point directly to
a Lucene segment file. The format and reader are documented in
[`FtsIndexReader`](../engine/crates/xerj-fts/src/index.rs#L1150-L1228).

[`PostingsWriter::encode_term`](../engine/crates/xerj-fts/src/postings.rs#L260-L331)
delta-encodes doc IDs in 128-doc PFOR blocks through its
[packed-block helpers](../engine/crates/xerj-fts/src/postings.rs#L333-L421);
positioned fields add packed term frequencies and variable-byte positions,
while exact fields can omit both. Residual postings use variable-byte encoding.
The [module format notes](../engine/crates/xerj-fts/src/postings.rs#L1-L40) make
the boundary explicit: this shared block size is not Lucene codec compatibility.
XERJ's `SkipEntry` is currently in-memory scaffolding and is not serialized, so
[`advance_to`](../engine/crates/xerj-fts/src/postings.rs#L702-L708) scans linearly.
Do not infer Lucene's two-level skip behavior from the shared block size.

## 3. Doc-values equivalent

Lucene exposes typed, column-oriented DocValues alongside postings; its codec chooses
on-disk representations for numeric, sorted, sorted-set, and related types. The
10.3.1 reference describes `.dvd` and `.dvm` files in
[`Lucene90DocValuesFormat`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/codecs/lucene90/Lucene90DocValuesFormat.html), while
[`SortedSetDocValuesField`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/document/SortedSetDocValuesField.html)
is useful for a multi-valued sorted field.

XERJ's closest equivalent is a per-segment `{segment_id}.dv` sidecar. The
[`Column` enum](../engine/crates/xerj-storage/src/doc_values.rs#L560-L590)
currently has numeric and keyword columns, each with a missing-value bitmap;
keyword columns use sorted terms plus ordinals, and numeric columns retain a
sorted `(value, doc_id)` index for range work. The binary envelope is produced
by [`encode_columns`](../engine/crates/xerj-storage/src/doc_values.rs#L620-L735)
and loaded through the engine's `.dv` cache.

The columns feed XERJ sorting, filters, ranges, and aggregation fast paths without reparsing `_source`. They are an analogous column store, not a
drop-in Lucene DocValues codec: field eligibility, array handling, compression,
and the public mapping contract are XERJ behavior. The segment's stored source
remains the fallback and hydration authority.

## 4. Analysis chain

Lucene's [`Analyzer`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/analysis/Analyzer.html)
creates a reusable token stream for a field, and
[`StandardAnalyzer`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/analysis/standard/StandardAnalyzer.html)
is one concrete preset. Analyzer output is part of the index contract: changing
analysis changes the terms that postings contain.

XERJ models the same conceptual stages in [`AnalyzerPipeline::analyze`](../engine/crates/xerj-fts/src/analyzer.rs#L69-L126): char filters, one tokenizer, then ordered token filters.
[`AnalyzerRegistry`](../engine/crates/xerj-fts/src/analyzer.rs#L937-L1100) starts with built-ins; supported custom settings are validated and installed at index creation ([create-time gate](../engine/crates/xerj-engine/src/index.rs#L5922-L5940)).
The FTS mapping builder chooses `standard` for `text` and `keyword` for other mapped field types ([`build_fts_field_configs`](../engine/crates/xerj-engine/src/index.rs#L19383-L19407)); its `standard` preset is Unicode word splitting plus lowercase, with no stop-word removal or stemming. Source wins over older architecture prose.

Query analysis is shape-specific, not general index/query custom-analyzer symmetry:
`match` uses the requested built-in analyzer or `standard` when omitted; unknown/custom
names decline to stored evaluation in this default registry ([`match` projection](../engine/crates/xerj-engine/src/index.rs#L37926-L37975)). Keyword matches use whole-value terms.
On keyword/exact fields, `match_phrase` uses the whole-value term path. On analyzed text fields, its FTS phrase path requires `standard` and zero slop; other analyzed shapes fall back to stored evaluation ([`match_phrase` implementation](../engine/crates/xerj-engine/src/index.rs#L38447-L38507)). Index-creation validation may accept supported custom settings,
but this projection does not promise general index/query custom-analyzer symmetry.

## 5. Update and delete semantics

Lucene documents are immutable once in a segment. In Lucene's own API,
`updateDocument` is expressed as delete-then-add; buffered deletes and live
document state are reconciled as segments are flushed and merged. The core
lifecycle is stable, but concrete segment, live-docs, and DocValues files are
codec/package details that can vary by Lucene commit; see the official
[10.3.1 index package](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/index/package-summary.html)
and [`IndexWriter`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/index/IndexWriter.html),
not universal filenames. Live-doc encoding is defined by the [codec live-docs
contract](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/codecs/LiveDocsFormat.html).
Lucene core accepts `IndexableField` objects, not JSON. Its
`updateNumericDocValue`/`updateBinaryDocValue` APIs only update existing fields
that were indexed with the corresponding DocValues type and DocValues only;
they cannot add arbitrary fields and are not a general JSON partial-update API.

XERJ also does not edit an already-published segment in place. A document
update appends a new WAL/index version under the same `_id`; the
[`VersionMap`](../engine/crates/xerj-storage/src/version_map.rs#L45-L118)
tracks the newest sequence number and whether the current version is deleted.
Old physical copies are ghosts until merge filtering removes them. Deletes
are WAL tombstones and can be carried in a segment's sequence-aware tombstone
section; the latest-version checks in merge and recovery keep an older copy
from reappearing.

The ES-shaped partial-update surface is real: [`update_document`](../engine/crates/xerj-engine/src/index.rs#L13144-L13210)
reads the current source, overlays top-level fields, and reindexes the merged
document; the API exposes `/_update/{id}` in [`update_doc`](../engine/crates/xerj-api/src/es_compat.rs#L17362-L17480)
and `/_update_by_query` in [`update_by_query`](../engine/crates/xerj-api/src/es_compat.rs#L24244-L24281).
That query-by-query handler hard-caps its fetch at 10,000 hits (`size: 10000`),
so one request is not a full-corpus traversal.
Updates are therefore logical replacement plus versioning, not byte-level
mutation. A delete also tombstones the old HNSW node when one exists, so a
vector result cannot outlive the document ([`delete_document_versioned`](../engine/crates/xerj-engine/src/index.rs#L12929-L13140)).

## 6. Vector index placement

Lucene treats a vector field as a first-class per-segment vector value and
delegates nearest-neighbor search to a `KnnVectorsFormat` reader. A
[`KnnFloatVectorField`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/document/KnnFloatVectorField.html)
creates that field, while [`KnnFloatVectorQuery`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/search/KnnFloatVectorQuery.html)
executes it; the codec boundary is [`KnnVectorsFormat`](https://lucene.apache.org/core/10_3_1/core/org/apache/lucene/codecs/KnnVectorsFormat.html).
The vector index is separate from the inverted term postings even though both
belong to the same Lucene segment lifecycle.

XERJ keeps vectors out of the lexical FTS term dictionary. A mapped
`dense_vector` is stored in `_source` for hydration and maintained by
[`index_vectors`](../engine/crates/xerj-engine/src/index.rs#L9939-L10070) in an
index-level persisted HNSW artifact (`hnsw/`; [`save_hnsw_to_disk`](../engine/crates/xerj-engine/src/index.rs#L12274-L12335)).
Graph serialization is [`HnswIndex::save_to`](../engine/crates/xerj-vector/src/hnsw.rs#L963-L1008);
the doc-ID maps are persisted alongside it.
That graph is not a postings list and is not an active FTS field sidecar.

Unfiltered top-level cosine kNN may use HNSW only after field-identity,
coverage, freshness, and size checks; candidates are fetched and exact
rescored. Exact rescoring does not make candidate selection exact: HNSW remains
approximate, so candidate-set recall can differ from brute force. Filters,
nested/passage shapes, unsupported metrics, small or stale graphs, and any
failed eligibility check use the exact brute-force path. The dispatch and
fallback rules are explicit in [`search_at_generation`](../engine/crates/xerj-engine/src/index.rs#L13844-L13930)
and [`run_knn_hnsw`](../engine/crates/xerj-engine/src/index.rs#L10470-L10530).

Non-claims: this page does not claim byte-compatible indexes, identical merge
policies, Lucene codec compatibility, complete analyzer parity, identical
vector recall or latency, or that an Elasticsearch API name implies Lucene
internals. It is a current-source orientation guide; re-check the linked
symbols when changing the engine.
