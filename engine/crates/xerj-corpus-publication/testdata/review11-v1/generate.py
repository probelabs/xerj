#!/usr/bin/env python3
"""Independent Review-11 protocol oracle.

This program deliberately does not import, execute, or parse output from the
Rust product crate.  It implements the published binary formulas directly,
uses Python's JSON/base64/SHA-256 implementations, and uses the system xxHash
library only for the separately specified XXH3-128 logical-edge identity.
"""

from __future__ import annotations

import argparse
import base64
import ctypes
import ctypes.util
import hashlib
import json
import struct
from pathlib import Path
from typing import Any


HERE = Path(__file__).resolve().parent
CATALOG_INDEX = ".xerj-autoindex-catalog-generations-v1"
NODE_INDEX = ".xerj-autoindex-graph-nodes-v1"


def u32(value: int) -> bytes:
    return struct.pack(">I", value)


def u64(value: int) -> bytes:
    return struct.pack(">Q", value)


def s(value: str | bytes) -> bytes:
    raw = value.encode() if isinstance(value, str) else value
    return u64(len(raw)) + raw


def a(values: list[bytes]) -> bytes:
    return u64(len(values)) + b"".join(values)


def raw_sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def rendered(prefix: str, preimage: bytes) -> str:
    return prefix + raw_sha256(preimage)


def canonical(value: Any) -> bytes:
    # The fixtures deliberately exercise objects, arrays, strings, integers,
    # booleans, and null.  They contain no binary64 formatting edge case.
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode()


def ordered_json(value: Any) -> bytes:
    return json.dumps(
        value,
        ensure_ascii=False,
        allow_nan=False,
        separators=(",", ":"),
        sort_keys=False,
    ).encode()


def vector(preimage: bytes, prefix: str | None = None) -> dict[str, Any]:
    result: dict[str, Any] = {
        "preimage_length": len(preimage),
        "preimage_base64": base64.b64encode(preimage).decode(),
        "sha256": raw_sha256(preimage),
    }
    if prefix is not None:
        result["rendered"] = prefix + result["sha256"]
    return result


def json_body(body: bytes) -> dict[str, Any]:
    return {
        "body_length": len(body),
        "body_base64": base64.b64encode(body).decode(),
        "canonical_json": body.decode(),
    }


def canonical_json_fields(body: bytes) -> dict[str, Any]:
    return {
        "canonical_json_length": len(body),
        "canonical_json_base64": base64.b64encode(body).decode(),
        "canonical_json": body.decode(),
    }


def xxh3_128_lower_hex(value: bytes) -> str:
    library = ctypes.util.find_library("xxhash")
    if library is None:
        raise RuntimeError(
            "independent oracle requires the system xxHash library for XXH3-128"
        )

    class Xxh128(ctypes.Structure):
        _fields_ = [("low64", ctypes.c_uint64), ("high64", ctypes.c_uint64)]

    lib = ctypes.CDLL(library)
    function = lib.XXH3_128bits
    function.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
    function.restype = Xxh128
    buffer = ctypes.create_string_buffer(value)
    digest = function(buffer, len(value))
    return f"{digest.high64:016x}{digest.low64:016x}"


def digest_vector(domain: bytes, fields: bytes, prefix: str) -> dict[str, Any]:
    return vector(domain + fields, prefix)


MANIFEST_VALUE = {
    "entries": [
        {"id": "doc-a", "path": "alpha.md"},
        {"id": "doc-b", "path": "beta.md"},
    ],
    "format_version": 1,
    "root_identity": "/r",
}
DATA_MAPPING_VALUE = {
    "properties": {"body": {"type": "text"}, "path": {"type": "keyword"}}
}
CATALOG_MAPPING_VALUE = {
    "properties": {"canonical": {"type": "keyword", "index": False}}
}
GRAPH_MAPPING_VALUE = {"properties": {"physical_id": {"type": "keyword"}}}
EXTRACTOR_CONFIG_VALUE: dict[str, Any] = {}
DATA_ROWS = [
    ("doc-a", {"path": "alpha.md", "body": "Alpha links [[beta]]."}),
    ("doc-b", {"path": "beta.md", "body": "Beta."}),
]
CATALOG_ROWS = [("wrap-1", {"path": "alpha.md", "kind": "file"})]
EDGE_ROWS = [
    {
        "src": "doc-a",
        "dst": "doc-b",
        "type": "wikilink",
        "weight": 1,
        "confidence": 1,
        "valid_at": 0,
        "created_at": 0,
        "detector": "wikilink@2",
        "schema_version": 1,
        "src_file": "alpha.md",
        "evidence": {"quote": "[[beta]]", "source": "alpha.md", "offset": 0},
    }
]
NODE_ROWS = [
    {
        "source_index": "life-docs",
        "logical_node_id": "doc-a",
        "title": "Alpha",
        "preview": None,
        "path": "alpha.md",
    },
    {
        "source_index": "life-docs",
        "logical_node_id": "doc-b",
        "title": None,
        "preview": "Beta.",
        "path": "beta.md",
    },
]


def artifact(kind: str, projection: str, resource: str, rows: int, body: bytes) -> dict[str, Any]:
    preimage = b"xerj-replay-artifact-v1\0" + s(kind) + u64(len(body)) + body
    return {
        "kind": kind,
        "projection_kind": projection,
        "resource_key": resource,
        "operation_count": rows,
        "byte_length": len(body),
        "bytes_base64": base64.b64encode(body).decode(),
        "raw_sha256": raw_sha256(body),
        "digest": vector(preimage, "xerra1-sha256-"),
    }


def replay_tuple(item: dict[str, Any]) -> bytes:
    return (
        s(item["kind"])
        + s(item["projection_kind"])
        + s(item["resource_key"])
        + u64(item["byte_length"])
        + u64(item["operation_count"])
        + s(item["digest"]["rendered"])
    )


def prepare_oracle() -> dict[str, Any]:
    manifest_json = canonical(MANIFEST_VALUE)
    manifest = digest_vector(
        b"xerj-autoindex-manifest-v1\0", s(manifest_json), "xerm1-sha256-"
    )
    owner = digest_vector(
        b"xerj-corpus-owner-v1\0", s("/r") + s("life"), "xercpo1-sha256-"
    )
    incarnation = digest_vector(
        b"xerj-corpus-incarnation-v1\0",
        s(owner["rendered"]) + bytes(range(0x00, 0x20)),
        "xercpi1-sha256-",
    )
    extractor_json = canonical(EXTRACTOR_CONFIG_VALUE)
    extractor_config = vector(extractor_json, "xerecfg1-sha256-")

    def mapping(value: Any) -> dict[str, Any]:
        body = canonical(value)
        result = digest_vector(b"xerj-mapping-v1\0", s(body), "xermap1-sha256-")
        result.update(canonical_json_fields(body))
        return result

    mappings = {
        "data": mapping(DATA_MAPPING_VALUE),
        "catalog": mapping(CATALOG_MAPPING_VALUE),
        "graph_edge": mapping(GRAPH_MAPPING_VALUE),
        "graph_node": mapping(GRAPH_MAPPING_VALUE),
    }

    data_rows = sorted((row_id, canonical(source)) for row_id, source in DATA_ROWS)
    data_ids_preimage = b"xerj-id-set-v1\0" + a([s(row_id) for row_id, _ in data_rows])
    data_content_preimage = b"xerj-data-content-v1\0" + a(
        [s(row_id) + s(source) for row_id, source in data_rows]
    )
    prepared_data_bytes = b"".join(
        ordered_json({"id": row_id}) + b"\n" + source + b"\n"
        for row_id, source in data_rows
    )
    prepared_data = artifact(
        "prepared-data-rows", "prepared", "not-a-replay-resource", 2, prepared_data_bytes
    )

    catalog_rows = sorted((row_id, canonical(source)) for row_id, source in CATALOG_ROWS)
    catalog_ids_preimage = b"xerj-catalog-id-set-v1\0" + a(
        [s(row_id) for row_id, _ in catalog_rows]
    )
    catalog_content_preimage = b"xerj-catalog-wrapper-set-v1\0" + a(
        [s(row_id) + s(source) for row_id, source in catalog_rows]
    )
    prepared_catalog_bytes = b"".join(
        ordered_json({"id": row_id}) + b"\n" + source + b"\n"
        for row_id, source in catalog_rows
    )
    prepared_catalog = artifact(
        "prepared-catalog-rows",
        "prepared",
        "not-a-replay-resource",
        1,
        prepared_catalog_bytes,
    )

    producer = digest_vector(
        b"xerj-autoindex-producer-v1\0",
        s(owner["rendered"])
        + s("life")
        + s("extractor@1")
        + s(extractor_config["rendered"]),
        "xerp1-sha256-",
    )
    edge_tuples = []
    for row in EDGE_ROWS:
        identity_input = (
            b"xg1\0"
            + row["src"].encode()
            + b"\0"
            + row["type"].encode()
            + b"\0"
            + row["dst"].encode()
            + b"\0"
            + str(row["valid_at"]).encode()
        )
        edge_tuples.append((xxh3_128_lower_hex(identity_input), canonical(row), identity_input))
    edge_tuples.sort(key=lambda item: (item[0].encode(), item[1]))
    logical_edges_preimage = b"xerj-graph-logical-edges-v1\0" + a(
        [s(edge_id) + s(row) for edge_id, row, _ in edge_tuples]
    )
    node_tuples = sorted(
        (
            row["source_index"],
            row["logical_node_id"],
            canonical(row),
        )
        for row in NODE_ROWS
    )
    logical_nodes_preimage = b"xerj-graph-logical-nodes-v1\0" + a(
        [s(source) + s(node_id) + s(row) for source, node_id, row in node_tuples]
    )
    values = {
        "manifest": {**manifest, **canonical_json_fields(manifest_json)},
        "owner": owner,
        "corpus_incarnation": incarnation,
        "extractor_config": {**extractor_config, **canonical_json_fields(extractor_json)},
        "mappings": mappings,
        "data_id_set": vector(data_ids_preimage, "xerids1-sha256-"),
        "data_content_set": vector(data_content_preimage, "xerdc1-sha256-"),
        "prepared_data_artifact": prepared_data,
        "catalog_id_set": vector(catalog_ids_preimage, "xercids1-sha256-"),
        "catalog_wrapper_set": vector(catalog_content_preimage, "xercws1-sha256-"),
        "prepared_catalog_artifact": prepared_catalog,
        "producer": producer,
        "logical_edge_id": {
            "input_base64": base64.b64encode(edge_tuples[0][2]).decode(),
            "rendered": edge_tuples[0][0],
        },
        "logical_edge_rows": [
            {
                "logical_edge_id": edge_id,
                "identity_input_base64": base64.b64encode(identity_input).decode(),
                **canonical_json_fields(row),
            }
            for edge_id, row, identity_input in edge_tuples
        ],
        "logical_node_rows": [
            {
                "source_index": source_index,
                "logical_node_id": logical_id,
                **canonical_json_fields(row),
            }
            for source_index, logical_id, row in node_tuples
        ],
        "logical_edge_set": vector(logical_edges_preimage, "xergle1-sha256-"),
        "logical_node_set": vector(logical_nodes_preimage, "xergln1-sha256-"),
    }
    core_body = (
        s("life")
        + s(owner["rendered"])
        + s(producer["rendered"])
        + u64(len(edge_tuples))
        + s(values["logical_edge_set"]["rendered"])
        + u64(len(node_tuples))
        + s(values["logical_node_set"]["rendered"])
    )
    values["graph_core"] = vector(
        b"xerj-graph-projection-core-v1\0" + core_body, "xergpc1-sha256-"
    )
    prepared_preimage = (
        b"xerj-prepared-input-v1\0"
        + u32(1)
        + s(owner["rendered"])
        + s(incarnation["rendered"])
        + s(manifest["rendered"])
        + a(
            [
                s("docs")
                + s(mappings["data"]["rendered"])
                + u64(len(data_rows))
                + s(values["data_id_set"]["rendered"])
                + s(values["data_content_set"]["rendered"])
                + s(prepared_data["digest"]["rendered"])
            ]
        )
        + u64(len(catalog_rows))
        + s(values["catalog_id_set"]["rendered"])
        + s(values["catalog_wrapper_set"]["rendered"])
        + s(prepared_catalog["digest"]["rendered"])
        + core_body
    )
    values["prepared_input"] = vector(prepared_preimage, "xerpdi1-sha256-")
    values["_data_rows"] = data_rows
    values["_catalog_rows"] = catalog_rows
    values["_edge_tuples"] = edge_tuples
    values["_node_tuples"] = node_tuples
    values["_core_body"] = core_body
    return values


def generation_oracle(prepared: dict[str, Any], expected: int, desired: int, generation: int) -> dict[str, Any]:
    owner = prepared["owner"]["rendered"]
    incarnation = prepared["corpus_incarnation"]["rendered"]
    manifest = prepared["manifest"]["rendered"]
    prepared_digest = prepared["prepared_input"]["rendered"]
    tx = digest_vector(
        b"xerj-autoindex-transaction-v1\0",
        s(owner)
        + s(incarnation)
        + u64(expected)
        + u64(desired)
        + s(manifest)
        + s(prepared_digest),
        "xertx1-sha256-",
    )
    generation_id = digest_vector(
        b"xerj-autoindex-generation-v1\0",
        s(owner) + s(incarnation) + u64(generation) + s(tx["rendered"]),
        "xerg1-sha256-",
    )
    owner_component_preimage = b"xerj-autoindex-physical-owner-v1\0" + s(owner)
    slug_component_preimage = b"xerj-autoindex-physical-slug-v1\0" + s("docs")
    stage_component_preimage = (
        b"xerj-autoindex-stage-identity-v1\0"
        + s(owner)
        + s(incarnation)
        + s(tx["rendered"])
        + s(manifest)
        + u64(generation)
        + s("docs")
    )
    name_components = {
        "owner": vector(owner_component_preimage),
        "slug": vector(slug_component_preimage),
        "stage": vector(stage_component_preimage),
    }
    physical = (
        f".xerj-aidx-d-{name_components['owner']['sha256']}-g{generation}"
        f"-s{name_components['slug']['sha256']}-t{name_components['stage']['sha256']}"
    )
    data_projection = digest_vector(
        b"xerj-data-projection-v1\0",
        u64(generation)
        + a(
            [
                s("docs")
                + s("life-docs")
                + s(physical)
                + s(prepared["mappings"]["data"]["rendered"])
                + u64(2)
                + s(prepared["data_id_set"]["rendered"])
                + s(prepared["data_content_set"]["rendered"])
            ]
        ),
        "xerd1-sha256-",
    )
    catalog_projection = digest_vector(
        b"xerj-catalog-projection-v1\0",
        s(owner)
        + s(incarnation)
        + u64(generation)
        + s(generation_id["rendered"])
        + u64(1)
        + s(prepared["catalog_wrapper_set"]["rendered"]),
        "xercatp1-sha256-",
    )
    catalog_incarnation = digest_vector(
        b"xerj-catalog-generation-incarnation-v1\0",
        s(owner)
        + s(incarnation)
        + u64(generation)
        + s(tx["rendered"])
        + s(catalog_projection["rendered"]),
        "xercati1-sha256-",
    )
    graph_token = digest_vector(
        b"xerj-autoindex-graph-token-v1\0",
        s(owner)
        + s(incarnation)
        + u64(generation)
        + s(tx["rendered"])
        + s(prepared["graph_core"]["rendered"]),
        "xergt1-sha256-",
    )
    edges = []
    for logical_id, row, _ in prepared["_edge_tuples"]:
        edge_id = digest_vector(
            b"xerj-graph-edge-physical-id-v1\0",
            s(owner)
            + s(incarnation)
            + u64(generation)
            + s(graph_token["rendered"])
            + s(logical_id),
            "xerge1-sha256-",
        )
        edges.append((logical_id, row, edge_id))
    nodes = []
    for source_index, logical_id, row in prepared["_node_tuples"]:
        node_id = digest_vector(
            b"xerj-graph-node-physical-id-v1\0",
            s(owner)
            + s(incarnation)
            + u64(generation)
            + s(graph_token["rendered"])
            + s(source_index)
            + s(logical_id),
            "xergn1-sha256-",
        )
        nodes.append((source_index, logical_id, row, node_id))
    edge_id_set = vector(
        b"xerj-graph-edge-physical-ids-v1\0"
        + a([s(value) for value in sorted(item[2]["rendered"] for item in edges)]),
        "xergepi1-sha256-",
    )
    node_id_set = vector(
        b"xerj-graph-node-physical-ids-v1\0"
        + a([s(value) for value in sorted(item[3]["rendered"] for item in nodes)]),
        "xergnpi1-sha256-",
    )
    graph_projection = digest_vector(
        b"xerj-graph-projection-v1\0",
        prepared["_core_body"]
        + s(prepared["graph_core"]["rendered"])
        + u64(generation)
        + s(graph_token["rendered"])
        + s(edge_id_set["rendered"])
        + s(node_id_set["rendered"]),
        "xergp1-sha256-",
    )

    data_body = b"".join(
        ordered_json({"index": {"_id": row_id, "_index": physical}})
        + b"\n"
        + source
        + b"\n"
        for row_id, source in prepared["_data_rows"]
    )
    data_resource = f"data/{physical}"
    artifacts = [artifact("data-bulk-ndjson", "data", data_resource, 2, data_body)]
    catalog_body = b"".join(
        ordered_json(
            {
                "index": {
                    "_id": row_id,
                    "_index": CATALOG_INDEX,
                    "generation": generation_id["rendered"],
                }
            }
        )
        + b"\n"
        + source
        + b"\n"
        for row_id, source in prepared["_catalog_rows"]
    )
    catalog_resource = f"catalog/{CATALOG_INDEX}/{generation_id['rendered']}"
    artifacts.append(
        artifact("catalog-bulk-ndjson", "catalog", catalog_resource, 1, catalog_body)
    )
    edges_index = ".xerj-memory-life-edges"
    edge_body = b""
    for logical_id, row_bytes, edge_id in edges:
        row = json.loads(row_bytes)
        row.update(
            {
                "corpus_incarnation": incarnation,
                "edge_scope": "generated",
                "graph_generation": generation,
                "graph_owner": owner,
                "graph_producer": prepared["producer"]["rendered"],
                "logical_edge_id": logical_id,
                "physical_id": edge_id["rendered"],
                "tx_id": tx["rendered"],
            }
        )
        edge_body += (
            ordered_json(
                {"index": {"_id": edge_id["rendered"], "_index": edges_index}}
            )
            + b"\n"
            + canonical(row)
            + b"\n"
        )
    edge_resource = f"graph-edge/{edges_index}/{graph_token['rendered']}"
    artifacts.append(
        artifact("graph-edge-bulk-ndjson", "graph-edge", edge_resource, len(edges), edge_body)
    )
    node_body = b""
    for _, _, row_bytes, node_id in nodes:
        row = json.loads(row_bytes)
        row.update(
            {
                "corpus_incarnation": incarnation,
                "doc_kind": "generated",
                "graph_generation": generation,
                "graph_owner": owner,
                "physical_id": node_id["rendered"],
                "tx_id": tx["rendered"],
            }
        )
        node_body += (
            ordered_json(
                {"index": {"_id": node_id["rendered"], "_index": NODE_INDEX}}
            )
            + b"\n"
            + canonical(row)
            + b"\n"
        )
    node_resource = f"graph-node/{NODE_INDEX}/{graph_token['rendered']}"
    artifacts.append(
        artifact("graph-node-bulk-ndjson", "graph-node", node_resource, len(nodes), node_body)
    )
    artifact_order = sorted(
        artifacts,
        key=lambda item: (
            item["projection_kind"].encode(),
            item["resource_key"].encode(),
            item["kind"].encode(),
            item["digest"]["rendered"].encode(),
        ),
    )
    replay_preimage = b"xerj-replay-set-v1\0" + a(
        [replay_tuple(item) for item in artifact_order]
    )
    replay_set = vector(replay_preimage, "xerrs1-sha256-")
    resource_keys = sorted(item["resource_key"] for item in artifacts)

    mapping_records = [
        ("data", data_resource, prepared["mappings"]["data"], canonical(DATA_MAPPING_VALUE)),
        (
            "catalog",
            catalog_resource,
            prepared["mappings"]["catalog"],
            canonical(CATALOG_MAPPING_VALUE),
        ),
        (
            "graph-edge",
            edge_resource,
            prepared["mappings"]["graph_edge"],
            canonical(GRAPH_MAPPING_VALUE),
        ),
        (
            "graph-node",
            node_resource,
            prepared["mappings"]["graph_node"],
            canonical(GRAPH_MAPPING_VALUE),
        ),
    ]
    mapping_records.sort(key=lambda item: (item[0].encode(), item[1].encode()))
    mapping_charge_bodies = [
        s(kind) + s(resource) + s(mapping["rendered"]) + s(mapping_json)
        for kind, resource, mapping, mapping_json in mapping_records
    ]
    quota = {
        "mapping": sum(map(len, mapping_charge_bodies)),
        "artifacts": sum(item["byte_length"] for item in artifacts),
        "operations": 64 * sum(item["operation_count"] for item in artifacts),
        "resources": 4096 * len(artifacts),
        "mapping_record_bodies_base64": [
            base64.b64encode(value).decode() for value in mapping_charge_bodies
        ],
    }
    quota["total"] = quota["mapping"] + quota["artifacts"] + quota["operations"] + quota["resources"]

    plan_preimage = (
        b"xerj-desired-publication-plan-v1\0"
        + u32(1)
        + s(owner)
        + s("life")
        + s("/r")
        + s(incarnation)
        + u64(expected)
        + u64(desired)
        + s(tx["rendered"])
        + s(manifest)
        + s(prepared_digest)
        + s(replay_set["rendered"])
        + u64(generation)
        + s(data_projection["rendered"])
        + a(
            [
                s("docs")
                + s("life-docs")
                + s(physical)
                + s(prepared["mappings"]["data"]["rendered"])
                + u64(2)
                + s(prepared["data_id_set"]["rendered"])
                + s(prepared["data_content_set"]["rendered"])
                + s(artifacts[0]["digest"]["rendered"])
            ]
        )
        + s(CATALOG_INDEX)
        + s(generation_id["rendered"])
        + s(catalog_incarnation["rendered"])
        + s(prepared["mappings"]["catalog"]["rendered"])
        + u64(1)
        + s(prepared["catalog_id_set"]["rendered"])
        + s(prepared["catalog_wrapper_set"]["rendered"])
        + s(catalog_projection["rendered"])
        + s(artifacts[1]["digest"]["rendered"])
        + s("life")
        + s(owner)
        + u64(generation)
        + s(prepared["producer"]["rendered"])
        + s(prepared["graph_core"]["rendered"])
        + s(graph_token["rendered"])
        + s(edges_index)
        + s(NODE_INDEX)
        + s(prepared["mappings"]["graph_edge"]["rendered"])
        + s(prepared["mappings"]["graph_node"]["rendered"])
        + u64(len(edges))
        + s(prepared["logical_edge_set"]["rendered"])
        + s(edge_id_set["rendered"])
        + u64(len(nodes))
        + s(prepared["logical_node_set"]["rendered"])
        + s(node_id_set["rendered"])
        + s(graph_projection["rendered"])
        + s(artifacts[2]["digest"]["rendered"])
        + s(artifacts[3]["digest"]["rendered"])
        + a(mapping_charge_bodies)
        + u64(quota["total"])
        + a([s(value) for value in resource_keys])
        + a([replay_tuple(item) for item in artifact_order])
    )
    plan = vector(plan_preimage, "xerdp1-sha256-")
    return {
        "expected_sequence": expected,
        "desired_sequence": desired,
        "generation": generation,
        "transaction": tx,
        "generation_id": generation_id,
        "name_components": name_components,
        "physical_data_name": physical,
        "data_projection": data_projection,
        "catalog_projection": catalog_projection,
        "catalog_generation_incarnation": catalog_incarnation,
        "graph_token": graph_token,
        "edge_physical_ids": [item[2] for item in edges],
        "node_physical_ids": [item[3] for item in nodes],
        "edge_physical_id_set": edge_id_set,
        "node_physical_id_set": node_id_set,
        "graph_projection": graph_projection,
        "artifacts": artifact_order,
        "replay_tuple_order": [
            {
                "position": position,
                "projection_kind": item["projection_kind"],
                "resource_key": item["resource_key"],
                "artifact_kind": item["kind"],
                "artifact_digest": item["digest"]["rendered"],
            }
            for position, item in enumerate(artifact_order)
        ],
        "replay_set": replay_set,
        "reserved_resource_keys": resource_keys,
        "mapping_reservations": [
            {
                "projection_kind": kind,
                "resource_key": resource,
                "mapping_digest": mapping["rendered"],
                **canonical_json_fields(mapping_json),
                "record": vector(body),
            }
            for (kind, resource, mapping, mapping_json), body in zip(
                mapping_records, mapping_charge_bodies
            )
        ],
        "mapping_reservation_array": vector(a(mapping_charge_bodies)),
        "quota": quota,
        "desired_plan": plan,
    }


def storage_incarnation(node_identity: str, kind: str, name: str, seed: bytes) -> dict[str, Any]:
    return digest_vector(
        b"xerj-storage-incarnation-v1\0",
        s(node_identity) + s(kind) + s(name) + seed,
        "xersi1-sha256-",
    )


def seal(
    domain: bytes,
    prefix: str,
    owner: str,
    incarnation: str,
    tx: str,
    generation: int,
    kind: str,
    storage_name: str,
    storage: str,
    final_sequence: int,
    mapping: str,
    count: int,
    ids: str,
    content: str,
    projection: str,
) -> dict[str, Any]:
    return digest_vector(
        domain,
        s(owner)
        + s(incarnation)
        + s(tx)
        + u64(generation)
        + s(kind)
        + s(storage_name)
        + s(storage)
        + u64(final_sequence)
        + s(mapping)
        + u64(count)
        + s(ids)
        + s(content)
        + s(projection),
        prefix,
    )


def publication_oracle(prepared: dict[str, Any], plan: dict[str, Any]) -> dict[str, Any]:
    owner = prepared["owner"]["rendered"]
    incarnation = prepared["corpus_incarnation"]["rendered"]
    tx = plan["transaction"]["rendered"]
    node_identity = digest_vector(
        b"xerj-node-identity-v1\0", bytes(range(0xA0, 0xC0)), "xerni1-sha256-"
    )
    storages = {
        "data": storage_incarnation(
            node_identity["rendered"],
            "data-index",
            plan["physical_data_name"],
            bytes(range(0x20, 0x40)),
        ),
        "catalog": storage_incarnation(
            node_identity["rendered"],
            "catalog-index",
            CATALOG_INDEX,
            bytes(range(0x40, 0x60)),
        ),
        "graph_edge": storage_incarnation(
            node_identity["rendered"],
            "graph-edge-index",
            ".xerj-memory-life-edges",
            bytes(range(0x60, 0x80)),
        ),
        "graph_node": storage_incarnation(
            node_identity["rendered"],
            "graph-node-index",
            NODE_INDEX,
            bytes(range(0x80, 0xA0)),
        ),
    }
    seals = {
        "data": seal(
            b"xerj-data-seal-v1\0",
            "xerds1-sha256-",
            owner,
            incarnation,
            tx,
            1,
            "data",
            plan["physical_data_name"],
            storages["data"]["rendered"],
            2,
            prepared["mappings"]["data"]["rendered"],
            2,
            prepared["data_id_set"]["rendered"],
            prepared["data_content_set"]["rendered"],
            plan["data_projection"]["rendered"],
        ),
        "catalog": seal(
            b"xerj-catalog-seal-v1\0",
            "xercs1-sha256-",
            owner,
            incarnation,
            tx,
            1,
            "catalog",
            CATALOG_INDEX,
            storages["catalog"]["rendered"],
            1,
            prepared["mappings"]["catalog"]["rendered"],
            1,
            prepared["catalog_id_set"]["rendered"],
            prepared["catalog_wrapper_set"]["rendered"],
            plan["catalog_projection"]["rendered"],
        ),
        "graph_edge": seal(
            b"xerj-graph-edge-seal-v1\0",
            "xerges1-sha256-",
            owner,
            incarnation,
            tx,
            1,
            "graph-edge",
            ".xerj-memory-life-edges",
            storages["graph_edge"]["rendered"],
            1,
            prepared["mappings"]["graph_edge"]["rendered"],
            1,
            plan["edge_physical_id_set"]["rendered"],
            prepared["logical_edge_set"]["rendered"],
            plan["graph_projection"]["rendered"],
        ),
        "graph_node": seal(
            b"xerj-graph-node-seal-v1\0",
            "xergns1-sha256-",
            owner,
            incarnation,
            tx,
            1,
            "graph-node",
            NODE_INDEX,
            storages["graph_node"]["rendered"],
            2,
            prepared["mappings"]["graph_node"]["rendered"],
            2,
            plan["node_physical_id_set"]["rendered"],
            prepared["logical_node_set"]["rendered"],
            plan["graph_projection"]["rendered"],
        ),
    }

    def seal_body(item: dict[str, Any], sequence: int) -> bytes:
        return u32(1) + u64(sequence) + s(item["rendered"])

    body = (
        b"xerj-corpus-publication-v1\0"
        + u32(1)
        + s(owner)
        + s("life")
        + s("/r")
        + s(incarnation)
        + u64(1)
        + s(tx)
        + s(prepared["manifest"]["rendered"])
        + s(plan["desired_plan"]["rendered"])
        + u64(1)
        + s(plan["data_projection"]["rendered"])
        + a(
            [
                s("docs")
                + s("life-docs")
                + s(plan["physical_data_name"])
                + s(storages["data"]["rendered"])
                + s(prepared["mappings"]["data"]["rendered"])
                + u64(2)
                + s(prepared["data_id_set"]["rendered"])
                + s(prepared["data_content_set"]["rendered"])
                + seal_body(seals["data"], 2)
            ]
        )
        + s(CATALOG_INDEX)
        + s(storages["catalog"]["rendered"])
        + s(plan["generation_id"]["rendered"])
        + s(plan["catalog_generation_incarnation"]["rendered"])
        + s(prepared["mappings"]["catalog"]["rendered"])
        + u64(1)
        + s(prepared["catalog_id_set"]["rendered"])
        + s(prepared["catalog_wrapper_set"]["rendered"])
        + s(plan["catalog_projection"]["rendered"])
        + seal_body(seals["catalog"], 1)
        + s("life")
        + s(owner)
        + u64(1)
        + s(prepared["producer"]["rendered"])
        + s(prepared["graph_core"]["rendered"])
        + s(plan["graph_token"]["rendered"])
        + s(".xerj-memory-life-edges")
        + s(storages["graph_edge"]["rendered"])
        + s(NODE_INDEX)
        + s(storages["graph_node"]["rendered"])
        + s(prepared["mappings"]["graph_edge"]["rendered"])
        + s(prepared["mappings"]["graph_node"]["rendered"])
        + u64(1)
        + s(prepared["logical_edge_set"]["rendered"])
        + s(plan["edge_physical_id_set"]["rendered"])
        + u64(2)
        + s(prepared["logical_node_set"]["rendered"])
        + s(plan["node_physical_id_set"]["rendered"])
        + s(plan["graph_projection"]["rendered"])
        + seal_body(seals["graph_edge"], 1)
        + seal_body(seals["graph_node"], 2)
    )
    publication = vector(body, "xercp1-sha256-")
    pub_value = {
        "format_version": 1,
        "owner": owner,
        "prefix": "life",
        "root_identity": "/r",
        "incarnation": incarnation,
        "sequence": 1,
        "tx_id": tx,
        "manifest_digest": prepared["manifest"]["rendered"],
        "plan_digest": plan["desired_plan"]["rendered"],
        "publication_digest": publication["rendered"],
        "data": {
            "generation": 1,
            "projection_digest": plan["data_projection"]["rendered"],
            "indices": [
                {
                    "slug": "docs",
                    "logical_index": "life-docs",
                    "physical_index": plan["physical_data_name"],
                    "physical_index_incarnation": storages["data"]["rendered"],
                    "mapping_digest": prepared["mappings"]["data"]["rendered"],
                    "document_count": 2,
                    "id_digest": prepared["data_id_set"]["rendered"],
                    "content_digest": prepared["data_content_set"]["rendered"],
                    "seal": {
                        "seal_version": 1,
                        "final_write_sequence": 2,
                        "seal_digest": seals["data"]["rendered"],
                    },
                }
            ],
        },
        "catalog": {
            "storage_index": CATALOG_INDEX,
            "storage_incarnation": storages["catalog"]["rendered"],
            "generation_id": plan["generation_id"]["rendered"],
            "incarnation": plan["catalog_generation_incarnation"]["rendered"],
            "mapping_digest": prepared["mappings"]["catalog"]["rendered"],
            "document_count": 1,
            "id_digest": prepared["catalog_id_set"]["rendered"],
            "content_digest": prepared["catalog_wrapper_set"]["rendered"],
            "projection_digest": plan["catalog_projection"]["rendered"],
            "seal": {
                "seal_version": 1,
                "final_write_sequence": 1,
                "seal_digest": seals["catalog"]["rendered"],
            },
        },
        "graph": {
            "brain": "life",
            "owner": owner,
            "generation": 1,
            "producer": prepared["producer"]["rendered"],
            "core_digest": prepared["graph_core"]["rendered"],
            "active_token": plan["graph_token"]["rendered"],
            "edges_index": ".xerj-memory-life-edges",
            "edges_index_incarnation": storages["graph_edge"]["rendered"],
            "nodes_index": NODE_INDEX,
            "nodes_index_incarnation": storages["graph_node"]["rendered"],
            "edge_mapping_digest": prepared["mappings"]["graph_edge"]["rendered"],
            "node_mapping_digest": prepared["mappings"]["graph_node"]["rendered"],
            "edge_count": 1,
            "logical_edge_digest": prepared["logical_edge_set"]["rendered"],
            "edge_physical_id_digest": plan["edge_physical_id_set"]["rendered"],
            "node_count": 2,
            "logical_node_digest": prepared["logical_node_set"]["rendered"],
            "node_physical_id_digest": plan["node_physical_id_set"]["rendered"],
            "projection_digest": plan["graph_projection"]["rendered"],
            "edge_seal": {
                "seal_version": 1,
                "final_write_sequence": 1,
                "seal_digest": seals["graph_edge"]["rendered"],
            },
            "node_seal": {
                "seal_version": 1,
                "final_write_sequence": 2,
                "seal_digest": seals["graph_node"]["rendered"],
            },
        },
    }
    pub_json = ordered_json(pub_value)
    return {
        "node_identity": node_identity,
        "storage_incarnations": storages,
        "seals": seals,
        "publication": {**publication, **json_body(pub_json)},
    }


def expectation(kind: str, owner: str, publication: dict[str, Any] | None = None) -> dict[str, Any]:
    if kind == "absent":
        body = u32(0) + s(owner) + u64(0)
        value = {"kind": "absent", "owner": owner, "sequence": 0}
    else:
        assert publication is not None
        body = (
            u32(1)
            + base64.b64decode(publication["preimage_base64"])
            + s(publication["rendered"])
        )
        value = {"kind": "present", "publication": json.loads(publication["canonical_json"])}
    digest = vector(b"xerj-expected-publication-v1\0" + body, "xerep1-sha256-")
    canonical_json = ordered_json(value)
    return {
        "kind": kind,
        "binary_body_length": len(body),
        "binary_body_base64": base64.b64encode(body).decode(),
        "digest": digest,
        **json_body(canonical_json),
    }


def sync_begin(expected: dict[str, Any], plan: dict[str, Any], prepared_digest: str) -> dict[str, Any]:
    plan_bytes = base64.b64decode(plan["desired_plan"]["preimage_base64"])
    body = (
        u32(1)
        + base64.b64decode(expected["binary_body_base64"])
        + s(expected["digest"]["rendered"])
        + u64(len(plan_bytes))
        + plan_bytes
        + s(plan["desired_plan"]["rendered"])
        + s(prepared_digest)
        + s(plan["replay_set"]["rendered"])
    )
    digest = vector(b"xerj-sync-begin-v1\0" + body, "xersb1-sha256-")
    encoded_plan = base64.b64encode(plan_bytes).decode()
    value = {
        "format_version": 1,
        "expected_publication": json.loads(expected["canonical_json"]),
        "expected_publication_digest": expected["digest"]["rendered"],
        "canonical_plan_bytes": encoded_plan,
        "plan_digest": plan["desired_plan"]["rendered"],
        "prepared_input_digest": prepared_digest,
        "replay_set_digest": plan["replay_set"]["rendered"],
        "sync_begin_digest": digest["rendered"],
    }
    result = {
        "binary_body_length": len(body),
        "binary_body_base64": base64.b64encode(body).decode(),
        "digest": digest,
        "canonical_plan_base64": encoded_plan,
        **json_body(ordered_json(value)),
    }
    assert base64.b64encode(base64.b64decode(encoded_plan)).decode() == encoded_plan
    return result


def persisted_class(raw: bytes) -> dict[str, Any]:
    """Describe one exact future journal payload without blessing it as parsed."""
    return {
        "byte_length": len(raw),
        "bytes_base64": base64.b64encode(raw).decode(),
        "raw_sha256": raw_sha256(raw),
    }


def durable_bundle_oracle(
    prepared: dict[str, Any],
    plan: dict[str, Any],
    begin: dict[str, Any],
    expectation_oracle: dict[str, Any],
) -> dict[str, Any]:
    """Pin all four persisted classes and the expected shared-validator result.

    This is an independent byte oracle.  It records the relation the Rust
    runner must prove; it does not claim that rehydration was executed here.
    """
    prepared_raw = base64.b64decode(prepared["prepared_input"]["preimage_base64"])
    plan_raw = base64.b64decode(plan["desired_plan"]["preimage_base64"])
    begin_raw = begin["canonical_json"].encode()
    replay = []
    for position, item in enumerate(plan["artifacts"]):
        raw = base64.b64decode(item["bytes_base64"])
        replay.append(
            {
                "position": position,
                "projection_kind": item["projection_kind"],
                "resource_key": item["resource_key"],
                "artifact_kind": item["kind"],
                "artifact_digest": item["digest"]["rendered"],
                **persisted_class(raw),
            }
        )

    return {
        "persisted_classes": {
            "prepared_input": {
                "wrapper": "PersistedPreparedInputBytesV1",
                **persisted_class(prepared_raw),
            },
            "replay_artifacts": {
                "wrapper": "PersistedReplayArtifactBytesV1",
                "ordering": "desired-plan canonical replay-tuple order; item i pairs only with tuple i",
                "cardinality": len(replay),
                "items": replay,
            },
            "desired_plan": {
                "wrapper": "PersistedDesiredPlanBytesV1",
                **persisted_class(plan_raw),
            },
            "sync_begin": {
                "wrapper": "PersistedSyncBeginBytesV1",
                **persisted_class(begin_raw),
            },
        },
        "fresh_vs_rehydrate": {
            "status": "expected_relation_pending_rust_runner",
            "relation": "byte_and_getter_identical",
            "expectation_kind": expectation_oracle["kind"],
            "expected_identities": {
                "prepared_input_digest": prepared["prepared_input"]["rendered"],
                "desired_plan_digest": plan["desired_plan"]["rendered"],
                "replay_set_digest": plan["replay_set"]["rendered"],
                "sync_begin_digest": begin["digest"]["rendered"],
                "expected_publication_digest": expectation_oracle["digest"]["rendered"],
                "artifact_digests_in_position_order": [
                    item["artifact_digest"] for item in replay
                ],
                "mapping_reservations_in_position_order": [
                    [item["projection_kind"], item["resource_key"], item["mapping_digest"]]
                    for item in plan["mapping_reservations"]
                ],
            },
            "expected_equal_byte_classes": [
                "prepared_input",
                "replay_artifacts[*]",
                "desired_plan",
                "sync_begin",
            ],
            "expected_equal_getters": [
                "prepared_input.digest",
                "desired_plan.digest",
                "desired_plan.owner",
                "desired_plan.corpus_incarnation",
                "desired_plan.transaction_id",
                "desired_plan.generation",
                "desired_plan.prepared_input_digest",
                "desired_plan.replay_set_digest",
                "desired_plan.quota_charge",
                "desired_plan.mapping_reservations[*]",
                "desired_plan.reserved_resource_keys[*]",
                "replay_artifacts[*].kind",
                "replay_artifacts[*].projection_kind",
                "replay_artifacts[*].resource_key",
                "replay_artifacts[*].byte_length",
                "replay_artifacts[*].operation_count",
                "replay_artifacts[*].digest",
                "sync_begin.expected_publication",
                "sync_begin.plan_digest",
                "sync_begin.prepared_input_digest",
                "sync_begin.replay_set_digest",
                "sync_begin.digest",
            ],
        },
    }


def empty_vectors() -> dict[str, Any]:
    domains = {
        "data_id": (b"xerj-id-set-v1\0", "xerids1-sha256-"),
        "data_content": (b"xerj-data-content-v1\0", "xerdc1-sha256-"),
        "catalog_id": (b"xerj-catalog-id-set-v1\0", "xercids1-sha256-"),
        "catalog_wrapper": (b"xerj-catalog-wrapper-set-v1\0", "xercws1-sha256-"),
        "logical_edge": (b"xerj-graph-logical-edges-v1\0", "xergle1-sha256-"),
        "logical_node": (b"xerj-graph-logical-nodes-v1\0", "xergln1-sha256-"),
        "edge_physical_id": (b"xerj-graph-edge-physical-ids-v1\0", "xergepi1-sha256-"),
        "node_physical_id": (b"xerj-graph-node-physical-ids-v1\0", "xergnpi1-sha256-"),
    }
    result = {name: vector(domain + a([]), prefix) for name, (domain, prefix) in domains.items()}
    result["artifacts"] = {
        kind: vector(b"xerj-replay-artifact-v1\0" + s(kind) + u64(0), "xerra1-sha256-")
        for kind in (
            "data-bulk-ndjson",
            "catalog-bulk-ndjson",
            "graph-edge-bulk-ndjson",
            "graph-node-bulk-ndjson",
            "prepared-data-rows",
            "prepared-catalog-rows",
        )
    }
    return result


def two_empty_route_oracle() -> dict[str, Any]:
    """Complete absent-begin chain with two distinct empty data routes.

    The data artifacts have identical empty bytes and identical kind-bound
    digests.  Their resources and replay tuples remain distinct, and their
    journal association is therefore the canonical tuple position.
    """
    manifest_value = {"entries": [], "format_version": 1, "root_identity": "/r"}
    manifest_json = canonical(manifest_value)
    manifest = digest_vector(
        b"xerj-autoindex-manifest-v1\0", s(manifest_json), "xerm1-sha256-"
    )
    owner = digest_vector(
        b"xerj-corpus-owner-v1\0", s("/r") + s("life"), "xercpo1-sha256-"
    )
    incarnation = digest_vector(
        b"xerj-corpus-incarnation-v1\0",
        s(owner["rendered"]) + bytes(range(32)),
        "xercpi1-sha256-",
    )

    def mapping(value: Any) -> dict[str, Any]:
        body = canonical(value)
        return {
            **digest_vector(b"xerj-mapping-v1\0", s(body), "xermap1-sha256-"),
            **canonical_json_fields(body),
        }

    data_mapping = mapping({"properties": {"body": {"type": "text"}}})
    catalog_mapping = mapping({"properties": {}})
    edge_mapping = mapping({"properties": {}})
    node_mapping = mapping({"properties": {}})
    extractor_json = canonical({})
    extractor = {**vector(extractor_json, "xerecfg1-sha256-"), **canonical_json_fields(extractor_json)}

    empty_data_ids = vector(b"xerj-id-set-v1\0" + a([]), "xerids1-sha256-")
    empty_data_content = vector(
        b"xerj-data-content-v1\0" + a([]), "xerdc1-sha256-"
    )
    empty_prepared_data = artifact(
        "prepared-data-rows", "prepared", "not-a-replay-resource", 0, b""
    )
    data_routes = [
        {
            "slug": slug,
            "logical_index": logical_index,
            "mapping": data_mapping,
            "document_count": 0,
            "id_set": empty_data_ids,
            "content_set": empty_data_content,
            "prepared_artifact": empty_prepared_data,
        }
        for slug, logical_index in (("docs", "life-docs"), ("notes", "life-notes"))
    ]
    empty_catalog_ids = vector(
        b"xerj-catalog-id-set-v1\0" + a([]), "xercids1-sha256-"
    )
    empty_catalog_content = vector(
        b"xerj-catalog-wrapper-set-v1\0" + a([]), "xercws1-sha256-"
    )
    empty_prepared_catalog = artifact(
        "prepared-catalog-rows", "prepared", "not-a-replay-resource", 0, b""
    )
    producer = digest_vector(
        b"xerj-autoindex-producer-v1\0",
        s(owner["rendered"]) + s("life") + s("extractor@1") + s(extractor["rendered"]),
        "xerp1-sha256-",
    )
    empty_edges = vector(
        b"xerj-graph-logical-edges-v1\0" + a([]), "xergle1-sha256-"
    )
    empty_nodes = vector(
        b"xerj-graph-logical-nodes-v1\0" + a([]), "xergln1-sha256-"
    )
    core_body = (
        s("life")
        + s(owner["rendered"])
        + s(producer["rendered"])
        + u64(0)
        + s(empty_edges["rendered"])
        + u64(0)
        + s(empty_nodes["rendered"])
    )
    graph_core = vector(
        b"xerj-graph-projection-core-v1\0" + core_body, "xergpc1-sha256-"
    )
    prepared_preimage = (
        b"xerj-prepared-input-v1\0"
        + u32(1)
        + s(owner["rendered"])
        + s(incarnation["rendered"])
        + s(manifest["rendered"])
        + a(
            [
                s(item["slug"])
                + s(item["mapping"]["rendered"])
                + u64(0)
                + s(item["id_set"]["rendered"])
                + s(item["content_set"]["rendered"])
                + s(item["prepared_artifact"]["digest"]["rendered"])
                for item in data_routes
            ]
        )
        + u64(0)
        + s(empty_catalog_ids["rendered"])
        + s(empty_catalog_content["rendered"])
        + s(empty_prepared_catalog["digest"]["rendered"])
        + core_body
    )
    prepared = {
        "manifest": {**manifest, **canonical_json_fields(manifest_json)},
        "owner": owner,
        "corpus_incarnation": incarnation,
        "extractor_config": extractor,
        "data_routes": data_routes,
        "catalog_mapping": catalog_mapping,
        "catalog_id_set": empty_catalog_ids,
        "catalog_wrapper_set": empty_catalog_content,
        "prepared_catalog_artifact": empty_prepared_catalog,
        "producer": producer,
        "edge_mapping": edge_mapping,
        "node_mapping": node_mapping,
        "logical_edge_set": empty_edges,
        "logical_node_set": empty_nodes,
        "graph_core": graph_core,
        "prepared_input": vector(prepared_preimage, "xerpdi1-sha256-"),
    }

    expected_sequence, desired_sequence, generation = 0, 1, 1
    tx = digest_vector(
        b"xerj-autoindex-transaction-v1\0",
        s(owner["rendered"])
        + s(incarnation["rendered"])
        + u64(expected_sequence)
        + u64(desired_sequence)
        + s(manifest["rendered"])
        + s(prepared["prepared_input"]["rendered"]),
        "xertx1-sha256-",
    )
    generation_id = digest_vector(
        b"xerj-autoindex-generation-v1\0",
        s(owner["rendered"])
        + s(incarnation["rendered"])
        + u64(generation)
        + s(tx["rendered"]),
        "xerg1-sha256-",
    )
    owner_component = vector(
        b"xerj-autoindex-physical-owner-v1\0" + s(owner["rendered"])
    )
    data_entries = []
    generated_artifacts = []
    for route in data_routes:
        slug_component = vector(
            b"xerj-autoindex-physical-slug-v1\0" + s(route["slug"])
        )
        stage_component = vector(
            b"xerj-autoindex-stage-identity-v1\0"
            + s(owner["rendered"])
            + s(incarnation["rendered"])
            + s(tx["rendered"])
            + s(manifest["rendered"])
            + u64(generation)
            + s(route["slug"])
        )
        physical = (
            f".xerj-aidx-d-{owner_component['sha256']}-g{generation}"
            f"-s{slug_component['sha256']}-t{stage_component['sha256']}"
        )
        replay = artifact(
            "data-bulk-ndjson", "data", f"data/{physical}", 0, b""
        )
        generated_artifacts.append(replay)
        data_entries.append(
            {
                **route,
                "physical_index": physical,
                "artifact": replay,
                "slug_component": slug_component,
                "stage_component": stage_component,
            }
        )
    data_entries.sort(
        key=lambda item: (
            item["slug"].encode(),
            item["logical_index"].encode(),
            item["physical_index"].encode(),
        )
    )
    data_projection = digest_vector(
        b"xerj-data-projection-v1\0",
        u64(generation)
        + a(
            [
                s(item["slug"])
                + s(item["logical_index"])
                + s(item["physical_index"])
                + s(item["mapping"]["rendered"])
                + u64(0)
                + s(item["id_set"]["rendered"])
                + s(item["content_set"]["rendered"])
                for item in data_entries
            ]
        ),
        "xerd1-sha256-",
    )
    catalog_projection = digest_vector(
        b"xerj-catalog-projection-v1\0",
        s(owner["rendered"])
        + s(incarnation["rendered"])
        + u64(generation)
        + s(generation_id["rendered"])
        + u64(0)
        + s(empty_catalog_content["rendered"]),
        "xercatp1-sha256-",
    )
    catalog_incarnation = digest_vector(
        b"xerj-catalog-generation-incarnation-v1\0",
        s(owner["rendered"])
        + s(incarnation["rendered"])
        + u64(generation)
        + s(tx["rendered"])
        + s(catalog_projection["rendered"]),
        "xercati1-sha256-",
    )
    graph_token = digest_vector(
        b"xerj-autoindex-graph-token-v1\0",
        s(owner["rendered"])
        + s(incarnation["rendered"])
        + u64(generation)
        + s(tx["rendered"])
        + s(graph_core["rendered"]),
        "xergt1-sha256-",
    )
    empty_edge_physical = vector(
        b"xerj-graph-edge-physical-ids-v1\0" + a([]), "xergepi1-sha256-"
    )
    empty_node_physical = vector(
        b"xerj-graph-node-physical-ids-v1\0" + a([]), "xergnpi1-sha256-"
    )
    graph_projection = digest_vector(
        b"xerj-graph-projection-v1\0",
        core_body
        + s(graph_core["rendered"])
        + u64(generation)
        + s(graph_token["rendered"])
        + s(empty_edge_physical["rendered"])
        + s(empty_node_physical["rendered"]),
        "xergp1-sha256-",
    )
    catalog_resource = f"catalog/{CATALOG_INDEX}/{generation_id['rendered']}"
    generated_artifacts.append(
        artifact("catalog-bulk-ndjson", "catalog", catalog_resource, 0, b"")
    )
    edge_resource = f"graph-edge/.xerj-memory-life-edges/{graph_token['rendered']}"
    generated_artifacts.append(
        artifact("graph-edge-bulk-ndjson", "graph-edge", edge_resource, 0, b"")
    )
    node_resource = f"graph-node/{NODE_INDEX}/{graph_token['rendered']}"
    generated_artifacts.append(
        artifact("graph-node-bulk-ndjson", "graph-node", node_resource, 0, b"")
    )
    artifact_order = sorted(
        generated_artifacts,
        key=lambda item: (
            item["projection_kind"].encode(),
            item["resource_key"].encode(),
            item["kind"].encode(),
            item["digest"]["rendered"].encode(),
        ),
    )
    replay_set = vector(
        b"xerj-replay-set-v1\0" + a([replay_tuple(item) for item in artifact_order]),
        "xerrs1-sha256-",
    )
    resource_keys = sorted(item["resource_key"] for item in generated_artifacts)
    mapping_records = [
        ("data", item["artifact"]["resource_key"], data_mapping, canonical({"properties": {"body": {"type": "text"}}}))
        for item in data_entries
    ]
    mapping_records.extend(
        [
            ("catalog", catalog_resource, catalog_mapping, canonical({"properties": {}})),
            ("graph-edge", edge_resource, edge_mapping, canonical({"properties": {}})),
            ("graph-node", node_resource, node_mapping, canonical({"properties": {}})),
        ]
    )
    mapping_records.sort(key=lambda item: (item[0].encode(), item[1].encode()))
    mapping_bodies = [
        s(kind) + s(resource) + s(mapping_value["rendered"]) + s(mapping_json)
        for kind, resource, mapping_value, mapping_json in mapping_records
    ]
    quota = {
        "mapping": sum(map(len, mapping_bodies)),
        "artifacts": 0,
        "operations": 0,
        "resources": 4096 * len(generated_artifacts),
        "mapping_record_bodies_base64": [
            base64.b64encode(body).decode() for body in mapping_bodies
        ],
    }
    quota["total"] = sum(quota[key] for key in ("mapping", "artifacts", "operations", "resources"))
    data_plan_bodies = [
        s(item["slug"])
        + s(item["logical_index"])
        + s(item["physical_index"])
        + s(item["mapping"]["rendered"])
        + u64(0)
        + s(item["id_set"]["rendered"])
        + s(item["content_set"]["rendered"])
        + s(item["artifact"]["digest"]["rendered"])
        for item in data_entries
    ]
    artifact_by_projection = {
        item["projection_kind"]: item
        for item in generated_artifacts
        if item["projection_kind"] != "data"
    }
    plan_preimage = (
        b"xerj-desired-publication-plan-v1\0"
        + u32(1)
        + s(owner["rendered"])
        + s("life")
        + s("/r")
        + s(incarnation["rendered"])
        + u64(expected_sequence)
        + u64(desired_sequence)
        + s(tx["rendered"])
        + s(manifest["rendered"])
        + s(prepared["prepared_input"]["rendered"])
        + s(replay_set["rendered"])
        + u64(generation)
        + s(data_projection["rendered"])
        + a(data_plan_bodies)
        + s(CATALOG_INDEX)
        + s(generation_id["rendered"])
        + s(catalog_incarnation["rendered"])
        + s(catalog_mapping["rendered"])
        + u64(0)
        + s(empty_catalog_ids["rendered"])
        + s(empty_catalog_content["rendered"])
        + s(catalog_projection["rendered"])
        + s(artifact_by_projection["catalog"]["digest"]["rendered"])
        + s("life")
        + s(owner["rendered"])
        + u64(generation)
        + s(producer["rendered"])
        + s(graph_core["rendered"])
        + s(graph_token["rendered"])
        + s(".xerj-memory-life-edges")
        + s(NODE_INDEX)
        + s(edge_mapping["rendered"])
        + s(node_mapping["rendered"])
        + u64(0)
        + s(empty_edges["rendered"])
        + s(empty_edge_physical["rendered"])
        + u64(0)
        + s(empty_nodes["rendered"])
        + s(empty_node_physical["rendered"])
        + s(graph_projection["rendered"])
        + s(artifact_by_projection["graph-edge"]["digest"]["rendered"])
        + s(artifact_by_projection["graph-node"]["digest"]["rendered"])
        + a(mapping_bodies)
        + u64(quota["total"])
        + a([s(value) for value in resource_keys])
        + a([replay_tuple(item) for item in artifact_order])
    )
    plan = {
        "expected_sequence": expected_sequence,
        "desired_sequence": desired_sequence,
        "generation": generation,
        "transaction": tx,
        "generation_id": generation_id,
        "data_entries": data_entries,
        "data_projection": data_projection,
        "catalog_projection": catalog_projection,
        "catalog_generation_incarnation": catalog_incarnation,
        "graph_token": graph_token,
        "edge_physical_id_set": empty_edge_physical,
        "node_physical_id_set": empty_node_physical,
        "graph_projection": graph_projection,
        "artifacts": artifact_order,
        "replay_tuple_order": [
            {
                "position": position,
                "projection_kind": item["projection_kind"],
                "resource_key": item["resource_key"],
                "artifact_kind": item["kind"],
                "artifact_digest": item["digest"]["rendered"],
            }
            for position, item in enumerate(artifact_order)
        ],
        "replay_set": replay_set,
        "mapping_reservations": [
            {
                "projection_kind": kind,
                "resource_key": resource,
                "mapping_digest": mapping_value["rendered"],
                **canonical_json_fields(mapping_json),
                "record": vector(body),
            }
            for (kind, resource, mapping_value, mapping_json), body in zip(
                mapping_records, mapping_bodies
            )
        ],
        "mapping_reservation_array": vector(a(mapping_bodies)),
        "quota": quota,
        "reserved_resource_keys": resource_keys,
        "desired_plan": vector(plan_preimage, "xerdp1-sha256-"),
    }
    expected = expectation("absent", owner["rendered"])
    begin = sync_begin(expected, plan, prepared["prepared_input"]["rendered"])
    return {
        "prepared": prepared,
        "planned": plan,
        "expectation": expected,
        "sync_begin": begin,
        "bundle": durable_bundle_oracle(prepared, plan, begin, expected),
        "positional_assertions": {
            "data_route_count": 2,
            "data_artifact_positions": [
                index
                for index, item in enumerate(artifact_order)
                if item["projection_kind"] == "data"
            ],
            "data_artifact_bytes_identical": len(
                {
                    item["bytes_base64"]
                    for item in artifact_order
                    if item["projection_kind"] == "data"
                }
            )
            == 1,
            "data_artifact_digests_identical": len(
                {
                    item["digest"]["rendered"]
                    for item in artifact_order
                    if item["projection_kind"] == "data"
                }
            )
            == 1,
            "data_resources_distinct": len(
                {
                    item["resource_key"]
                    for item in artifact_order
                    if item["projection_kind"] == "data"
                }
            )
            == 2,
            "swapping_identical_empty_payloads": "observationally_identical",
            "omission_or_addition": "parse_error",
        },
    }


def ordering_prepare_oracle() -> dict[str, Any]:
    """Build the complete, deliberately reversed two-distinct-row input chain."""
    manifest_value = {
        "entries": [
            {"id": "doc-a", "path": "a.md"},
            {"id": "doc-b", "path": "b.md"},
            {"id": "doc-z", "path": "z.md"},
        ],
        "format_version": 1,
        "root_identity": "/ordering",
    }
    route_inputs = [
        {
            "slug": "zeta",
            "logical_index": "life-zeta",
            "mapping_value": {"enabled": True},
            "rows": [("doc-z", {"path": "z.md", "rank": 26})],
        },
        {
            "slug": "alpha",
            "logical_index": "life-alpha",
            "mapping_value": {"z": {"type": "keyword"}, "a": {"type": "text"}},
            "rows": [
                ("doc-b", {"path": "b.md", "rank": 2}),
                ("doc-a", {"rank": 1, "path": "a.md"}),
            ],
        },
    ]
    catalog_rows_input = [
        ("wrap-z", {"canonical": "z"}),
        ("wrap-a", {"canonical": "a"}),
    ]
    edge_rows_input = [
        {
            "src": "doc-z", "dst": "doc-a", "type": "zeta", "weight": 2,
            "confidence": 1, "valid_at": 1, "created_at": 1,
            "detector": "ordering@1", "schema_version": 1, "src_file": "z.md",
            "evidence": {"quote": "z to a", "source": "z.md", "offset": 1},
        },
        {
            "src": "doc-a", "dst": "doc-b", "type": "alpha", "weight": 1,
            "confidence": 0.5, "valid_at": 0, "created_at": 0,
            "detector": "ordering@1", "schema_version": 1, "src_file": "a.md",
            "evidence": {"quote": "a to b", "source": "a.md", "offset": 0},
        },
    ]
    node_rows_input = [
        {"source_index": "life-zeta", "logical_node_id": "doc-z", "title": "Z", "preview": None, "path": "z.md"},
        {"source_index": "life-alpha", "logical_node_id": "doc-b", "title": "B", "preview": None, "path": "b.md"},
        {"source_index": "life-alpha", "logical_node_id": "doc-a", "title": "A", "preview": None, "path": "a.md"},
    ]

    manifest_json = canonical(manifest_value)
    manifest = digest_vector(b"xerj-autoindex-manifest-v1\0", s(manifest_json), "xerm1-sha256-")
    owner = digest_vector(
        b"xerj-corpus-owner-v1\0", s("/ordering") + s("life"), "xercpo1-sha256-"
    )
    incarnation = digest_vector(
        b"xerj-corpus-incarnation-v1\0",
        s(owner["rendered"]) + bytes(255 - i for i in range(32)),
        "xercpi1-sha256-",
    )

    def mapping(value: Any) -> dict[str, Any]:
        body = canonical(value)
        return {
            **digest_vector(b"xerj-mapping-v1\0", s(body), "xermap1-sha256-"),
            **canonical_json_fields(body),
        }

    routes = []
    for item in route_inputs:
        rows = sorted((row_id, canonical(source)) for row_id, source in item["rows"])
        ids = vector(
            b"xerj-id-set-v1\0" + a([s(row_id) for row_id, _ in rows]),
            "xerids1-sha256-",
        )
        content = vector(
            b"xerj-data-content-v1\0" + a([s(row_id) + s(source) for row_id, source in rows]),
            "xerdc1-sha256-",
        )
        prepared_bytes = b"".join(
            ordered_json({"id": row_id}) + b"\n" + source + b"\n" for row_id, source in rows
        )
        routes.append(
            {
                "slug": item["slug"],
                "logical_index": item["logical_index"],
                "mapping": mapping(item["mapping_value"]),
                "rows": rows,
                "id_set": ids,
                "content_set": content,
                "prepared_artifact": artifact(
                    "prepared-data-rows", "prepared", "not-a-replay-resource",
                    len(rows), prepared_bytes,
                ),
            }
        )
    routes.sort(key=lambda item: item["slug"].encode())

    catalog_mapping = mapping({"enabled": False})
    catalog_rows = sorted((row_id, canonical(source)) for row_id, source in catalog_rows_input)
    catalog_ids = vector(
        b"xerj-catalog-id-set-v1\0" + a([s(row_id) for row_id, _ in catalog_rows]),
        "xercids1-sha256-",
    )
    catalog_wrappers = vector(
        b"xerj-catalog-wrapper-set-v1\0"
        + a([s(row_id) + s(source) for row_id, source in catalog_rows]),
        "xercws1-sha256-",
    )
    prepared_catalog_bytes = b"".join(
        ordered_json({"id": row_id}) + b"\n" + source + b"\n"
        for row_id, source in catalog_rows
    )
    prepared_catalog = artifact(
        "prepared-catalog-rows", "prepared", "not-a-replay-resource",
        len(catalog_rows), prepared_catalog_bytes,
    )

    extractor_json = canonical({"z": 0, "a": 1})
    extractor_config = vector(extractor_json, "xerecfg1-sha256-")
    producer = digest_vector(
        b"xerj-autoindex-producer-v1\0",
        s(owner["rendered"]) + s("life") + s("ordering@1") + s(extractor_config["rendered"]),
        "xerp1-sha256-",
    )
    edge_tuples = []
    for row in edge_rows_input:
        identity_input = (
            b"xg1\0" + row["src"].encode() + b"\0" + row["type"].encode()
            + b"\0" + row["dst"].encode() + b"\0" + str(row["valid_at"]).encode()
        )
        edge_tuples.append((xxh3_128_lower_hex(identity_input), canonical(row), identity_input))
    edge_tuples.sort(key=lambda item: (item[0].encode(), item[1]))
    logical_edges = vector(
        b"xerj-graph-logical-edges-v1\0"
        + a([s(edge_id) + s(row) for edge_id, row, _ in edge_tuples]),
        "xergle1-sha256-",
    )
    node_tuples = sorted(
        (row["source_index"], row["logical_node_id"], canonical(row)) for row in node_rows_input
    )
    logical_nodes = vector(
        b"xerj-graph-logical-nodes-v1\0"
        + a([s(source) + s(node_id) + s(row) for source, node_id, row in node_tuples]),
        "xergln1-sha256-",
    )
    core_body = (
        s("life") + s(owner["rendered"]) + s(producer["rendered"])
        + u64(len(edge_tuples)) + s(logical_edges["rendered"])
        + u64(len(node_tuples)) + s(logical_nodes["rendered"])
    )
    graph_core = vector(b"xerj-graph-projection-core-v1\0" + core_body, "xergpc1-sha256-")
    prepared_preimage = (
        b"xerj-prepared-input-v1\0" + u32(1) + s(owner["rendered"])
        + s(incarnation["rendered"]) + s(manifest["rendered"])
        + a([
            s(item["slug"]) + s(item["mapping"]["rendered"]) + u64(len(item["rows"]))
            + s(item["id_set"]["rendered"]) + s(item["content_set"]["rendered"])
            + s(item["prepared_artifact"]["digest"]["rendered"])
            for item in routes
        ])
        + u64(len(catalog_rows)) + s(catalog_ids["rendered"])
        + s(catalog_wrappers["rendered"]) + s(prepared_catalog["digest"]["rendered"])
        + core_body
    )
    return {
        "manifest": {**manifest, **canonical_json_fields(manifest_json)},
        "owner": owner,
        "corpus_incarnation": incarnation,
        "extractor_config": {**extractor_config, **canonical_json_fields(extractor_json)},
        "data_routes": routes,
        "catalog_mapping": catalog_mapping,
        "catalog_id_set": catalog_ids,
        "catalog_wrapper_set": catalog_wrappers,
        "prepared_catalog_artifact": prepared_catalog,
        "producer": producer,
        "edge_mapping": mapping({"z": {}, "a": {}}),
        "node_mapping": mapping({"z": {}, "a": {}}),
        "logical_edge_set": logical_edges,
        "logical_node_set": logical_nodes,
        "logical_edge_rows": [
            {
                "logical_edge_id": edge_id,
                "identity_input_base64": base64.b64encode(identity_input).decode(),
                **canonical_json_fields(row),
            }
            for edge_id, row, identity_input in edge_tuples
        ],
        "logical_node_rows": [
            {
                "source_index": source_index,
                "logical_node_id": logical_id,
                **canonical_json_fields(row),
            }
            for source_index, logical_id, row in node_tuples
        ],
        "graph_core": graph_core,
        "prepared_input": vector(prepared_preimage, "xerpdi1-sha256-"),
        "_catalog_rows": catalog_rows,
        "_edge_tuples": edge_tuples,
        "_node_tuples": node_tuples,
        "_core_body": core_body,
    }


def ordering_generation_oracle(prepared: dict[str, Any]) -> dict[str, Any]:
    owner = prepared["owner"]["rendered"]
    incarnation = prepared["corpus_incarnation"]["rendered"]
    manifest = prepared["manifest"]["rendered"]
    prepared_digest = prepared["prepared_input"]["rendered"]
    expected, desired, generation = 8, 9, 42
    tx = digest_vector(
        b"xerj-autoindex-transaction-v1\0",
        s(owner) + s(incarnation) + u64(expected) + u64(desired)
        + s(manifest) + s(prepared_digest),
        "xertx1-sha256-",
    )
    generation_id = digest_vector(
        b"xerj-autoindex-generation-v1\0",
        s(owner) + s(incarnation) + u64(generation) + s(tx["rendered"]),
        "xerg1-sha256-",
    )
    owner_component = vector(b"xerj-autoindex-physical-owner-v1\0" + s(owner))
    data_entries = []
    artifacts = []
    for route in prepared["data_routes"]:
        slug_component = vector(b"xerj-autoindex-physical-slug-v1\0" + s(route["slug"]))
        stage_component = vector(
            b"xerj-autoindex-stage-identity-v1\0" + s(owner) + s(incarnation)
            + s(tx["rendered"]) + s(manifest) + u64(generation) + s(route["slug"])
        )
        physical = (
            f".xerj-aidx-d-{owner_component['sha256']}-g{generation}"
            f"-s{slug_component['sha256']}-t{stage_component['sha256']}"
        )
        body = b"".join(
            ordered_json({"index": {"_id": row_id, "_index": physical}})
            + b"\n" + source + b"\n" for row_id, source in route["rows"]
        )
        replay = artifact(
            "data-bulk-ndjson", "data", f"data/{physical}", len(route["rows"]), body
        )
        artifacts.append(replay)
        data_entries.append(
            {
                "slug": route["slug"], "logical_index": route["logical_index"],
                "physical_index": physical, "mapping": route["mapping"],
                "document_count": len(route["rows"]), "id_set": route["id_set"],
                "content_set": route["content_set"], "artifact": replay,
                "slug_component": slug_component, "stage_component": stage_component,
            }
        )
    data_entries.sort(key=lambda item: (item["slug"].encode(), item["logical_index"].encode(), item["physical_index"].encode()))
    data_projection = digest_vector(
        b"xerj-data-projection-v1\0",
        u64(generation) + a([
            s(item["slug"]) + s(item["logical_index"]) + s(item["physical_index"])
            + s(item["mapping"]["rendered"]) + u64(item["document_count"])
            + s(item["id_set"]["rendered"]) + s(item["content_set"]["rendered"])
            for item in data_entries
        ]),
        "xerd1-sha256-",
    )
    catalog_projection = digest_vector(
        b"xerj-catalog-projection-v1\0",
        s(owner) + s(incarnation) + u64(generation) + s(generation_id["rendered"])
        + u64(len(prepared["_catalog_rows"])) + s(prepared["catalog_wrapper_set"]["rendered"]),
        "xercatp1-sha256-",
    )
    catalog_incarnation = digest_vector(
        b"xerj-catalog-generation-incarnation-v1\0",
        s(owner) + s(incarnation) + u64(generation) + s(tx["rendered"])
        + s(catalog_projection["rendered"]),
        "xercati1-sha256-",
    )
    graph_token = digest_vector(
        b"xerj-autoindex-graph-token-v1\0",
        s(owner) + s(incarnation) + u64(generation) + s(tx["rendered"])
        + s(prepared["graph_core"]["rendered"]),
        "xergt1-sha256-",
    )
    edges = []
    for logical_id, row, _ in prepared["_edge_tuples"]:
        physical_id = digest_vector(
            b"xerj-graph-edge-physical-id-v1\0",
            s(owner) + s(incarnation) + u64(generation) + s(graph_token["rendered"])
            + s(logical_id),
            "xerge1-sha256-",
        )
        edges.append((logical_id, row, physical_id))
    nodes = []
    for source_index, logical_id, row in prepared["_node_tuples"]:
        physical_id = digest_vector(
            b"xerj-graph-node-physical-id-v1\0",
            s(owner) + s(incarnation) + u64(generation) + s(graph_token["rendered"])
            + s(source_index) + s(logical_id),
            "xergn1-sha256-",
        )
        nodes.append((source_index, logical_id, row, physical_id))
    edge_id_set = vector(
        b"xerj-graph-edge-physical-ids-v1\0"
        + a([s(value) for value in sorted(item[2]["rendered"] for item in edges)]),
        "xergepi1-sha256-",
    )
    node_id_set = vector(
        b"xerj-graph-node-physical-ids-v1\0"
        + a([s(value) for value in sorted(item[3]["rendered"] for item in nodes)]),
        "xergnpi1-sha256-",
    )
    graph_projection = digest_vector(
        b"xerj-graph-projection-v1\0",
        prepared["_core_body"] + s(prepared["graph_core"]["rendered"])
        + u64(generation) + s(graph_token["rendered"])
        + s(edge_id_set["rendered"]) + s(node_id_set["rendered"]),
        "xergp1-sha256-",
    )
    catalog_body = b"".join(
        ordered_json({"index": {"_id": row_id, "_index": CATALOG_INDEX, "generation": generation_id["rendered"]}})
        + b"\n" + source + b"\n" for row_id, source in prepared["_catalog_rows"]
    )
    catalog_resource = f"catalog/{CATALOG_INDEX}/{generation_id['rendered']}"
    catalog_artifact = artifact(
        "catalog-bulk-ndjson", "catalog", catalog_resource,
        len(prepared["_catalog_rows"]), catalog_body,
    )
    artifacts.append(catalog_artifact)
    edges_index = ".xerj-memory-life-edges"
    edge_body = b""
    for logical_id, row_bytes, physical_id in edges:
        row = json.loads(row_bytes)
        row.update({
            "corpus_incarnation": incarnation, "edge_scope": "generated",
            "graph_generation": generation, "graph_owner": owner,
            "graph_producer": prepared["producer"]["rendered"],
            "logical_edge_id": logical_id, "physical_id": physical_id["rendered"],
            "tx_id": tx["rendered"],
        })
        edge_body += ordered_json({"index": {"_id": physical_id["rendered"], "_index": edges_index}}) + b"\n" + canonical(row) + b"\n"
    edge_resource = f"graph-edge/{edges_index}/{graph_token['rendered']}"
    edge_artifact = artifact("graph-edge-bulk-ndjson", "graph-edge", edge_resource, len(edges), edge_body)
    artifacts.append(edge_artifact)
    node_body = b""
    for _, _, row_bytes, physical_id in nodes:
        row = json.loads(row_bytes)
        row.update({
            "corpus_incarnation": incarnation, "doc_kind": "generated",
            "graph_generation": generation, "graph_owner": owner,
            "physical_id": physical_id["rendered"], "tx_id": tx["rendered"],
        })
        node_body += ordered_json({"index": {"_id": physical_id["rendered"], "_index": NODE_INDEX}}) + b"\n" + canonical(row) + b"\n"
    node_resource = f"graph-node/{NODE_INDEX}/{graph_token['rendered']}"
    node_artifact = artifact("graph-node-bulk-ndjson", "graph-node", node_resource, len(nodes), node_body)
    artifacts.append(node_artifact)

    artifact_order = sorted(
        artifacts,
        key=lambda item: (item["projection_kind"].encode(), item["resource_key"].encode(), item["kind"].encode(), item["digest"]["rendered"].encode()),
    )
    replay_set = vector(
        b"xerj-replay-set-v1\0" + a([replay_tuple(item) for item in artifact_order]),
        "xerrs1-sha256-",
    )
    resource_keys = sorted(item["resource_key"] for item in artifacts)
    mapping_records = []
    for item in data_entries:
        mapping_records.append(("data", item["artifact"]["resource_key"], item["mapping"], item["mapping"]["canonical_json"].encode()))
    mapping_records.extend([
        ("catalog", catalog_resource, prepared["catalog_mapping"], prepared["catalog_mapping"]["canonical_json"].encode()),
        ("graph-edge", edge_resource, prepared["edge_mapping"], prepared["edge_mapping"]["canonical_json"].encode()),
        ("graph-node", node_resource, prepared["node_mapping"], prepared["node_mapping"]["canonical_json"].encode()),
    ])
    mapping_records.sort(key=lambda item: (item[0].encode(), item[1].encode()))
    mapping_bodies = [s(kind) + s(resource) + s(mapping["rendered"]) + s(body) for kind, resource, mapping, body in mapping_records]
    quota = {
        "mapping": sum(map(len, mapping_bodies)),
        "artifacts": sum(item["byte_length"] for item in artifacts),
        "operations": 64 * sum(item["operation_count"] for item in artifacts),
        "resources": 4096 * len(artifacts),
        "mapping_record_bodies_base64": [base64.b64encode(body).decode() for body in mapping_bodies],
    }
    quota["total"] = quota["mapping"] + quota["artifacts"] + quota["operations"] + quota["resources"]
    data_plan_bodies = [
        s(item["slug"]) + s(item["logical_index"]) + s(item["physical_index"])
        + s(item["mapping"]["rendered"]) + u64(item["document_count"])
        + s(item["id_set"]["rendered"]) + s(item["content_set"]["rendered"])
        + s(item["artifact"]["digest"]["rendered"])
        for item in data_entries
    ]
    plan_preimage = (
        b"xerj-desired-publication-plan-v1\0" + u32(1) + s(owner) + s("life")
        + s("/ordering") + s(incarnation) + u64(expected) + u64(desired)
        + s(tx["rendered"]) + s(manifest) + s(prepared_digest) + s(replay_set["rendered"])
        + u64(generation) + s(data_projection["rendered"]) + a(data_plan_bodies)
        + s(CATALOG_INDEX) + s(generation_id["rendered"]) + s(catalog_incarnation["rendered"])
        + s(prepared["catalog_mapping"]["rendered"]) + u64(len(prepared["_catalog_rows"]))
        + s(prepared["catalog_id_set"]["rendered"]) + s(prepared["catalog_wrapper_set"]["rendered"])
        + s(catalog_projection["rendered"]) + s(catalog_artifact["digest"]["rendered"])
        + s("life") + s(owner) + u64(generation) + s(prepared["producer"]["rendered"])
        + s(prepared["graph_core"]["rendered"]) + s(graph_token["rendered"])
        + s(edges_index) + s(NODE_INDEX) + s(prepared["edge_mapping"]["rendered"])
        + s(prepared["node_mapping"]["rendered"]) + u64(len(edges))
        + s(prepared["logical_edge_set"]["rendered"]) + s(edge_id_set["rendered"])
        + u64(len(nodes)) + s(prepared["logical_node_set"]["rendered"])
        + s(node_id_set["rendered"]) + s(graph_projection["rendered"])
        + s(edge_artifact["digest"]["rendered"]) + s(node_artifact["digest"]["rendered"])
        + a(mapping_bodies) + u64(quota["total"])
        + a([s(value) for value in resource_keys])
        + a([replay_tuple(item) for item in artifact_order])
    )
    return {
        "expected_sequence": expected, "desired_sequence": desired, "generation": generation,
        "transaction": tx, "generation_id": generation_id, "owner_name_component": owner_component,
        "data_entries": data_entries, "data_projection": data_projection,
        "catalog_projection": catalog_projection, "catalog_generation_incarnation": catalog_incarnation,
        "graph_token": graph_token, "edge_physical_ids": [item[2] for item in edges],
        "node_physical_ids": [item[3] for item in nodes], "edge_physical_id_set": edge_id_set,
        "node_physical_id_set": node_id_set, "graph_projection": graph_projection,
        "artifacts": artifact_order,
        "artifact_tuple_order": [
            {
                "position": position,
                "projection_kind": item["projection_kind"],
                "resource_key": item["resource_key"],
                "artifact_kind": item["kind"],
                "artifact_digest": item["digest"]["rendered"],
            }
            for position, item in enumerate(artifact_order)
        ],
        "reserved_resource_keys": resource_keys, "mapping_record_order": [[item[0], item[1]] for item in mapping_records],
        "mapping_reservations": [
            {
                "projection_kind": kind,
                "resource_key": resource,
                "mapping_digest": mapping["rendered"],
                **canonical_json_fields(mapping_json),
                "record": vector(body),
            }
            for (kind, resource, mapping, mapping_json), body in zip(
                mapping_records, mapping_bodies
            )
        ],
        "mapping_reservation_array": vector(a(mapping_bodies)),
        "quota": quota, "replay_set": replay_set,
        "data_plan_array": vector(a(data_plan_bodies)),
        "reserved_resource_array": vector(a([s(value) for value in resource_keys])),
        "replay_tuple_array": vector(a([replay_tuple(item) for item in artifact_order])),
        "desired_plan": vector(plan_preimage, "xerdp1-sha256-"),
    }


def ordering_publication_oracle(
    prepared: dict[str, Any],
    plan: dict[str, Any],
    *,
    sequence_override: int | None = None,
) -> dict[str, Any]:
    """Generate a complete parseable publication with two sorted data entries."""
    owner = prepared["owner"]["rendered"]
    incarnation = prepared["corpus_incarnation"]["rendered"]
    tx = plan["transaction"]["rendered"]
    generation = plan["generation"]
    sequence = (
        plan["desired_sequence"] if sequence_override is None else sequence_override
    )
    node_identity = digest_vector(
        b"xerj-node-identity-v1\0", bytes(range(0xA0, 0xC0)), "xerni1-sha256-"
    )
    data_storages = []
    for index, entry in enumerate(plan["data_entries"]):
        seed_start = 0x20 if index == 0 else 0xC0
        data_storages.append(
            storage_incarnation(
                node_identity["rendered"], "data-index", entry["physical_index"],
                bytes(range(seed_start, seed_start + 0x20)),
            )
        )
    catalog_storage = storage_incarnation(
        node_identity["rendered"], "catalog-index", CATALOG_INDEX, bytes(range(0x40, 0x60))
    )
    edge_storage = storage_incarnation(
        node_identity["rendered"], "graph-edge-index", ".xerj-memory-life-edges",
        bytes(range(0x60, 0x80)),
    )
    node_storage = storage_incarnation(
        node_identity["rendered"], "graph-node-index", NODE_INDEX, bytes(range(0x80, 0xA0))
    )
    data_seals = []
    for entry, storage in zip(plan["data_entries"], data_storages):
        data_seals.append(
            seal(
                b"xerj-data-seal-v1\0", "xerds1-sha256-", owner, incarnation, tx,
                generation, "data", entry["physical_index"], storage["rendered"],
                entry["document_count"], entry["mapping"]["rendered"],
                entry["document_count"], entry["id_set"]["rendered"],
                entry["content_set"]["rendered"], plan["data_projection"]["rendered"],
            )
        )
    catalog_seal = seal(
        b"xerj-catalog-seal-v1\0", "xercs1-sha256-", owner, incarnation, tx,
        generation, "catalog", CATALOG_INDEX, catalog_storage["rendered"],
        len(prepared["_catalog_rows"]), prepared["catalog_mapping"]["rendered"],
        len(prepared["_catalog_rows"]), prepared["catalog_id_set"]["rendered"],
        prepared["catalog_wrapper_set"]["rendered"], plan["catalog_projection"]["rendered"],
    )
    edge_seal = seal(
        b"xerj-graph-edge-seal-v1\0", "xerges1-sha256-", owner, incarnation, tx,
        generation, "graph-edge", ".xerj-memory-life-edges", edge_storage["rendered"],
        len(prepared["_edge_tuples"]), prepared["edge_mapping"]["rendered"],
        len(prepared["_edge_tuples"]), plan["edge_physical_id_set"]["rendered"],
        prepared["logical_edge_set"]["rendered"], plan["graph_projection"]["rendered"],
    )
    node_seal = seal(
        b"xerj-graph-node-seal-v1\0", "xergns1-sha256-", owner, incarnation, tx,
        generation, "graph-node", NODE_INDEX, node_storage["rendered"],
        len(prepared["_node_tuples"]), prepared["node_mapping"]["rendered"],
        len(prepared["_node_tuples"]), plan["node_physical_id_set"]["rendered"],
        prepared["logical_node_set"]["rendered"], plan["graph_projection"]["rendered"],
    )

    def seal_body(item: dict[str, Any], final_write_sequence: int) -> bytes:
        return u32(1) + u64(final_write_sequence) + s(item["rendered"])

    data_bodies = []
    data_json = []
    for entry, storage, data_seal in zip(plan["data_entries"], data_storages, data_seals):
        final_write_sequence = entry["document_count"]
        data_bodies.append(
            s(entry["slug"]) + s(entry["logical_index"]) + s(entry["physical_index"])
            + s(storage["rendered"]) + s(entry["mapping"]["rendered"])
            + u64(entry["document_count"]) + s(entry["id_set"]["rendered"])
            + s(entry["content_set"]["rendered"]) + seal_body(data_seal, final_write_sequence)
        )
        data_json.append(
            {
                "slug": entry["slug"], "logical_index": entry["logical_index"],
                "physical_index": entry["physical_index"],
                "physical_index_incarnation": storage["rendered"],
                "mapping_digest": entry["mapping"]["rendered"],
                "document_count": entry["document_count"], "id_digest": entry["id_set"]["rendered"],
                "content_digest": entry["content_set"]["rendered"],
                "seal": {"seal_version": 1, "final_write_sequence": final_write_sequence, "seal_digest": data_seal["rendered"]},
            }
        )
    body = (
        b"xerj-corpus-publication-v1\0" + u32(1) + s(owner) + s("life") + s("/ordering")
        + s(incarnation) + u64(sequence) + s(tx) + s(prepared["manifest"]["rendered"])
        + s(plan["desired_plan"]["rendered"]) + u64(generation)
        + s(plan["data_projection"]["rendered"]) + a(data_bodies)
        + s(CATALOG_INDEX) + s(catalog_storage["rendered"]) + s(plan["generation_id"]["rendered"])
        + s(plan["catalog_generation_incarnation"]["rendered"])
        + s(prepared["catalog_mapping"]["rendered"]) + u64(len(prepared["_catalog_rows"]))
        + s(prepared["catalog_id_set"]["rendered"]) + s(prepared["catalog_wrapper_set"]["rendered"])
        + s(plan["catalog_projection"]["rendered"]) + seal_body(catalog_seal, len(prepared["_catalog_rows"]))
        + s("life") + s(owner) + u64(generation) + s(prepared["producer"]["rendered"])
        + s(prepared["graph_core"]["rendered"]) + s(plan["graph_token"]["rendered"])
        + s(".xerj-memory-life-edges") + s(edge_storage["rendered"]) + s(NODE_INDEX)
        + s(node_storage["rendered"]) + s(prepared["edge_mapping"]["rendered"])
        + s(prepared["node_mapping"]["rendered"]) + u64(len(prepared["_edge_tuples"]))
        + s(prepared["logical_edge_set"]["rendered"]) + s(plan["edge_physical_id_set"]["rendered"])
        + u64(len(prepared["_node_tuples"])) + s(prepared["logical_node_set"]["rendered"])
        + s(plan["node_physical_id_set"]["rendered"]) + s(plan["graph_projection"]["rendered"])
        + seal_body(edge_seal, len(prepared["_edge_tuples"]))
        + seal_body(node_seal, len(prepared["_node_tuples"]))
    )
    publication = vector(body, "xercp1-sha256-")
    value = {
        "format_version": 1, "owner": owner, "prefix": "life", "root_identity": "/ordering",
        "incarnation": incarnation, "sequence": sequence, "tx_id": tx,
        "manifest_digest": prepared["manifest"]["rendered"],
        "plan_digest": plan["desired_plan"]["rendered"], "publication_digest": publication["rendered"],
        "data": {"generation": generation, "projection_digest": plan["data_projection"]["rendered"], "indices": data_json},
        "catalog": {
            "storage_index": CATALOG_INDEX, "storage_incarnation": catalog_storage["rendered"],
            "generation_id": plan["generation_id"]["rendered"],
            "incarnation": plan["catalog_generation_incarnation"]["rendered"],
            "mapping_digest": prepared["catalog_mapping"]["rendered"],
            "document_count": len(prepared["_catalog_rows"]),
            "id_digest": prepared["catalog_id_set"]["rendered"],
            "content_digest": prepared["catalog_wrapper_set"]["rendered"],
            "projection_digest": plan["catalog_projection"]["rendered"],
            "seal": {"seal_version": 1, "final_write_sequence": len(prepared["_catalog_rows"]), "seal_digest": catalog_seal["rendered"]},
        },
        "graph": {
            "brain": "life", "owner": owner, "generation": generation,
            "producer": prepared["producer"]["rendered"], "core_digest": prepared["graph_core"]["rendered"],
            "active_token": plan["graph_token"]["rendered"], "edges_index": ".xerj-memory-life-edges",
            "edges_index_incarnation": edge_storage["rendered"], "nodes_index": NODE_INDEX,
            "nodes_index_incarnation": node_storage["rendered"],
            "edge_mapping_digest": prepared["edge_mapping"]["rendered"],
            "node_mapping_digest": prepared["node_mapping"]["rendered"],
            "edge_count": len(prepared["_edge_tuples"]),
            "logical_edge_digest": prepared["logical_edge_set"]["rendered"],
            "edge_physical_id_digest": plan["edge_physical_id_set"]["rendered"],
            "node_count": len(prepared["_node_tuples"]),
            "logical_node_digest": prepared["logical_node_set"]["rendered"],
            "node_physical_id_digest": plan["node_physical_id_set"]["rendered"],
            "projection_digest": plan["graph_projection"]["rendered"],
            "edge_seal": {"seal_version": 1, "final_write_sequence": len(prepared["_edge_tuples"]), "seal_digest": edge_seal["rendered"]},
            "node_seal": {"seal_version": 1, "final_write_sequence": len(prepared["_node_tuples"]), "seal_digest": node_seal["rendered"]},
        },
    }
    publication_json = ordered_json(value)
    return {
        "node_identity": node_identity, "data_storage_incarnations": data_storages,
        "catalog_storage_incarnation": catalog_storage,
        "edge_storage_incarnation": edge_storage, "node_storage_incarnation": node_storage,
        "data_seals": data_seals, "catalog_seal": catalog_seal,
        "edge_seal": edge_seal, "node_seal": node_seal,
        "data_body_array": vector(a(data_bodies)),
        "publication": {**publication, **json_body(publication_json)},
    }


def strip_private(value: Any) -> Any:
    if isinstance(value, dict):
        return {key: strip_private(item) for key, item in value.items() if not key.startswith("_")}
    if isinstance(value, list):
        return [strip_private(item) for item in value]
    if isinstance(value, tuple):
        return [strip_private(item) for item in value]
    if isinstance(value, bytes):
        return base64.b64encode(value).decode()
    return value


def json_object_mutation_cases(value: Any, path: str = "$") -> list[str]:
    """Enumerate the closed-object unknown/missing/null/duplicate cases."""
    cases: list[str] = []
    if isinstance(value, dict):
        cases.append(f"{path}:unknown-member")
        for key, child in sorted(value.items()):
            member_path = f"{path}.{key}"
            cases.append(f"{member_path}:missing")
            # Logical-node title/preview are nullable by contract.  Replacing
            # a non-null optional value with null is a valid fresh-chain
            # change, while replacing an existing null with null is no change;
            # neither is a parse-error mutation.
            if child is not None and key not in {"title", "preview"}:
                cases.append(f"{member_path}:null")
            cases.append(f"{member_path}:duplicate")
            cases.extend(json_object_mutation_cases(child, member_path))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            cases.extend(json_object_mutation_cases(child, f"{path}[{index}]"))
    return cases


def mutation_ledger(goldens: dict[str, Any]) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []

    def add(
        row_id: str,
        tier: str,
        runner: str,
        baseline: str,
        mutation: str,
        outcome: str,
        cases: list[str],
        *,
        case_ranges: list[dict[str, Any]] | None = None,
        case_expectations: dict[str, dict[str, str]] | None = None,
    ) -> None:
        row: dict[str, Any] = {
            "id": row_id,
            "tier": (
                "integration"
                if runner.startswith(("mutations::", "resource_key_nul::"))
                else "compile"
                if runner.startswith("ui::")
                else "unit"
            ),
            "semantic_tier": tier,
            "runner": runner,
            "baseline": baseline,
            "mutation": mutation,
            "outcome": outcome,
            "case_count": len(cases)
            + sum(item["count"] for item in (case_ranges or [])),
            "cases": cases,
        }
        if case_ranges:
            row["case_ranges"] = case_ranges
        if case_expectations:
            unknown_cases = sorted(set(case_expectations) - set(cases))
            if unknown_cases:
                raise AssertionError(
                    f"{row_id}: expectations name undeclared cases: {unknown_cases}"
                )
            row["case_expectations"] = case_expectations
        rows.append(row)

    digest_baselines = [
        "CorpusOwnerId",
        "CorpusIncarnationId",
        "ManifestDigest",
        "ExtractorConfigDigest",
        "MappingDigest",
        "PreparedInputDigest",
        "TransactionId",
        "ReplayArtifactDigest",
        "ReplaySetDigest",
        "DesiredPlanDigest",
        "PublicationDigest",
        "ExpectedPublicationDigest",
        "SyncBeginDigest",
    ]
    digest_mutations = [
        "wrong-prefix",
        "wrong-algorithm",
        "short-hex",
        "uppercase-hex",
        "leading-space",
        "trailing-space",
        "terminal-nul",
        "cross-domain-prefix",
    ]
    add(
        "digest-rendering-cartesian",
        "public_parser",
        "mutations::rendered_digest_cartesian",
        "all 13 crate-root rendered digest types",
        "one exact spelling mutation per digest type and mutation name",
        "parse_error",
        [f"{digest}:{mutation}" for digest in digest_baselines for mutation in digest_mutations],
    )

    closed_baselines: list[tuple[str, str]] = [
        ("manifest", goldens["prepared"]["manifest"]["canonical_json"]),
        *[
            (f"logical-edge-{index}", row["canonical_json"])
            for index, row in enumerate(goldens["prepared"]["logical_edge_rows"])
        ],
        *[
            (f"logical-node-{index}", row["canonical_json"])
            for index, row in enumerate(goldens["prepared"]["logical_node_rows"])
        ],
        ("prior-publication", goldens["prior_publication"]["publication"]["canonical_json"]),
        ("absent-expectation", goldens["expectations"]["absent"]["canonical_json"]),
        ("present-expectation", goldens["expectations"]["present"]["canonical_json"]),
        ("absent-sync-begin", goldens["sync_begins"]["absent"]["canonical_json"]),
        ("present-sync-begin", goldens["sync_begins"]["present"]["canonical_json"]),
    ]
    closed_cases = []
    for name, body in closed_baselines:
        closed_cases.extend(
            f"{name}:{case}"
            for case in json_object_mutation_cases(json.loads(body))
        )
    add(
        "closed-json-cartesian",
        "public_parser",
        "mutations::closed_json_cartesian",
        "all closed JSON parser fixtures",
        "unknown at every object and missing/null/duplicate for every required member",
        "parse_error",
        closed_cases,
    )

    add(
        "open-payload-fresh-derivations",
        "fresh_derivation",
        "mutations::open_payload_member_cartesian_changes_chain",
        "fresh canonical logical inputs",
        "add one retained nested custom member and rebuild the whole chain",
        "changed_chain",
        [
            "data-mapping:add-custom-nested",
            "catalog-mapping:add-custom-nested",
            "graph-edge-mapping:add-custom-nested",
            "graph-node-mapping:add-custom-nested",
            "extractor-config:add-custom-nested",
            "data-source:add-custom-nested",
            "catalog-source:add-custom-nested",
        ],
    )
    add(
        "rfc8785-member-reordering",
        "fresh_derivation",
        "mutations::rfc8785_member_reordering_is_unchanged",
        "open canonical JSON inputs",
        "reverse z/a insertion order without changing the JSON value",
        "unchanged",
        [
            "data-mapping:z-a-to-a-z",
            "catalog-mapping:z-a-to-a-z",
            "graph-edge-mapping:z-a-to-a-z",
            "graph-node-mapping:z-a-to-a-z",
            "extractor-config:z-a-to-a-z",
            "data-source:z-a-to-a-z",
            "catalog-source:z-a-to-a-z",
        ],
    )
    add(
        "rfc8785-invalid-numbers",
        "public_parser",
        "mutations::rfc8785_invalid_number_matrix",
        "each JSON parser family",
        "supply a non-finite spelling, or a fractional value only in an actual closed integral field",
        "parse_error",
        [
            f"{parser}:{number}"
            for parser in [
                "manifest",
                "data-mapping",
                "catalog-mapping",
                "graph-edge-mapping",
                "graph-node-mapping",
                "extractor-config",
                "data-source",
                "catalog-source",
                "logical-edge",
                "logical-node",
                "publication",
                "expectation",
                "sync-begin",
            ]
            for number in ["NaN", "+Infinity", "-Infinity"]
        ]
        + [
            "manifest:format_version=1.5",
            "logical-edge:schema_version=1.5",
            "publication:sequence=1.5",
            "expectation:absent.sequence=0.5",
            "sync-begin:format_version=1.5",
        ],
    )

    prepared_length = goldens["prepared"]["prepared_input"]["preimage_length"]
    plan_length = goldens["generation_1"]["desired_plan"]["preimage_length"]
    add(
        "standalone-binary-byte-flips",
        "standalone_binary_parser",
        "mutations::complete_binary_byte_flip_cartesian",
        "primary prepared-input and desired-plan preimages",
        "xor 0x01 at each byte offset independently",
        "parse_error_or_changed_chain",
        [],
        case_ranges=[
            {"baseline": "prepared", "first_offset": 0, "last_offset": prepared_length - 1, "count": prepared_length},
            {"baseline": "desired-plan", "first_offset": 0, "last_offset": plan_length - 1, "count": plan_length},
        ],
    )
    persisted_lengths = {
        "prepared-input": prepared_length,
        "desired-plan": plan_length,
        "sync-begin": goldens["sync_begins"]["absent"]["body_length"],
    }
    add(
        "persisted-class-byte-flips",
        "persisted_bundle",
        "mutations::persisted_bundle_byte_flip_cartesian",
        "primary four-class durable bundle",
        "xor 0x01 at each non-replay persisted byte and rehydrate against the other held classes",
        "parse_error",
        [],
        case_ranges=[
            {
                "baseline": baseline,
                "first_offset": 0,
                "last_offset": length - 1,
                "count": length,
            }
            for baseline, length in persisted_lengths.items()
        ],
    )
    replay_ranges = []
    for position, item in enumerate(goldens["generation_1"]["artifacts"]):
        length = item["byte_length"]
        if length:
            replay_ranges.append(
                {
                    "baseline": f"replay[{position}]/{item['projection_kind']}",
                    "first_offset": 0,
                    "last_offset": length - 1,
                    "count": length,
                }
            )
    add(
        "persisted-replay-byte-flips",
        "persisted_bundle",
        "mutations::persisted_replay_byte_flip_cartesian",
        "primary replay vector in canonical tuple order",
        "xor 0x01 at each byte offset and rehydrate against held prepared/plan/begin bytes",
        "parse_error",
        [],
        case_ranges=replay_ranges,
    )

    add(
        "fresh-generation-seven-chain",
        "fresh_derivation",
        "mutations::generation_only_changed_chain",
        "generation_1 versus generation_7",
        "change only explicit Generation from 1 to 7 and rebuild",
        "changed_chain",
        [
            "generation-id:changed",
            "physical-data-name:changed",
            "catalog-incarnation:changed",
            "graph-token:changed",
            "edge-physical-id:changed",
            "node-physical-ids:changed",
            "edge-physical-set:changed",
            "node-physical-set:changed",
            "data-projection:changed",
            "catalog-projection:changed",
            "graph-projection:changed",
            "replay-artifacts:changed",
            "replay-set:changed",
            "mapping-resources:changed",
            "reserved-resources:changed",
            "desired-plan:changed",
            "sync-begin:changed",
        ],
    )
    add(
        "fresh-generation-seven-transaction-control",
        "fresh_derivation",
        "mutations::generation_only_transaction_is_unchanged",
        "generation_1 versus generation_7",
        "change only explicit Generation from 1 to 7 and compare transaction bytes",
        "unchanged",
        ["transaction:byte-identical"],
    )
    add(
        "mapping-reservation-persisted-matrix",
        "persisted_bundle",
        "mutations::persisted_mapping_reservation_matrix",
        "primary desired-plan mapping reservation array",
        "apply one named false persisted mapping state while holding/recomputing outer attachments as the case states",
        "parse_error",
        [
            "omit-record",
            "add-internally-valid-extra-record",
            "duplicate-record",
            "reverse-record-order",
            "wrong-projection-kind",
            "wrong-resource-key",
            "wrong-rendered-mapping-digest",
            "mapping-json-byte-change-with-old-digest",
            "mapping-json-byte-change-with-recomputed-record-digest-only",
            "noncanonical-mapping-json-member-order",
            "duplicate-key-mapping-json",
            "mapping-json-not-object",
            "projection-repeated-digest-mismatch",
            "reserved-resource-set-mismatch",
            "replay-tuple-resource-mismatch",
            "replay-tuple-projection-mismatch",
            "mapping-record-count-less-than-resource-count",
            "mapping-record-count-greater-than-resource-count",
        ],
    )
    add(
        "quota-persisted-matrix",
        "persisted_bundle",
        "mutations::persisted_quota_matrix",
        "primary desired-plan quota and its exact mapping/replay/resource inputs",
        "change one persisted component/total or trigger one checked arithmetic edge",
        "parse_error",
        [
            "mapping-record-body-length-minus-one",
            "mapping-record-body-length-plus-one",
            "artifact-length-minus-one",
            "artifact-length-plus-one",
            "operation-count-minus-one",
            "operation-count-plus-one",
            "resource-count-minus-one",
            "resource-count-plus-one",
            "encoded-total-minus-one",
            "encoded-total-plus-one",
            "omit-S-prefix-from-mapping-charge",
            "add-record-envelope-length-to-mapping-charge",
            "artifact-sum-overflow",
            "operation-sum-overflow",
            "operation-multiply-64-overflow",
            "stage-add-mapping-artifact-overflow",
            "stage-add-operation-overflow",
        ],
    )
    add(
        "replay-vector-positional-matrix",
        "persisted_bundle",
        "mutations::persisted_replay_vector_matrix",
        "primary, two-empty, and two-nonempty replay vectors",
        "change cardinality or positional association",
        "parse_error",
        [
            "primary:omit-item",
            "primary:add-item",
            "primary:duplicate-item",
            "primary:reverse-distinct-items",
            "two-empty:omit-first-data-item",
            "two-empty:add-empty-data-item",
            "two-nonempty:swap-data-items",
            "two-nonempty:omit-first-data-item",
            "two-nonempty:add-first-data-item-copy",
        ],
    )
    add(
        "replay-vector-identical-empty-swap",
        "persisted_bundle",
        "mutations::identical_empty_payload_swap_is_observationally_identical",
        "two-empty positional fixture data positions",
        "swap two byte-identical empty payload values without moving tuple positions",
        "unchanged",
        ["swap-empty-data-payload-values-at-positions-1-and-2"],
    )
    add(
        "strict-replay-boundary-matrix",
        "persisted_bundle",
        "mutations::strict_replay_boundary_matrix",
        "primary nonempty replay artifacts",
        "apply one exact NDJSON boundary or JSON canonicality defect",
        "parse_error",
        [
            "data:odd-line-count",
            "data:blank-action-line",
            "data:blank-source-line",
            "data:missing-source-line",
            "data:extra-source-line",
            "data:missing-final-lf",
            "data:extra-final-lf",
            "data:crlf",
            "data:embedded-cr",
            "data:trailing-space-after-final-lf",
            "data:noncanonical-action-json",
            "data:noncanonical-source-json",
            "data:duplicate-action-json-key",
            "data:duplicate-source-json-key",
            "catalog:missing-final-lf",
            "catalog:duplicate-action-json-key",
            "graph-edge:missing-final-lf",
            "graph-edge:duplicate-source-json-key",
            "graph-node:missing-final-lf",
            "graph-node:noncanonical-source-json",
        ],
    )
    add(
        "strict-replay-action-matrix",
        "persisted_bundle",
        "mutations::strict_replay_action_matrix",
        "primary replay action objects",
        "apply one exact closed action metadata defect",
        "parse_error",
        [
            f"{kind}:{case}"
            for kind in ["data", "catalog", "graph-edge", "graph-node"]
            for case in [
                "verb-create",
                "verb-update",
                "verb-delete",
                "missing-index-action",
                "null-index-action",
                "unknown-top-level-action-member",
                "missing-_id",
                "null-_id",
                "nonstring-_id",
                "missing-_index",
                "null-_index",
                "nonstring-_index",
                "unknown-index-metadata-member",
                "wrong-target",
            ]
        ]
        + [
            "catalog:missing-generation",
            "catalog:null-generation",
            "catalog:wrong-generation",
            "catalog:unknown-generation-sibling",
            "data:forbidden-generation",
            "graph-edge:forbidden-generation",
            "graph-node:forbidden-generation",
            "data:mixed-targets-between-operations",
        ],
    )
    add(
        "strict-replay-content-join-matrix",
        "persisted_bundle",
        "mutations::strict_replay_content_join_matrix",
        "primary and comparator prepared summaries, plan projections, and replay rows",
        "change one persisted row/content/generated field while other classes remain held or are recomputed only as named",
        "parse_error",
        [
            "data:duplicate-id-row",
            "data:reverse-row-order",
            "data:changed-source-with-old-content-digest",
            "data:changed-path-with-old-content-and-artifact-digests",
            "data:changed-id-with-old-id-set",
            "data:prepared-payload-digest-mismatch",
            "catalog:duplicate-wrapper-row",
            "catalog:reverse-row-order-control-with-two-row-fixture",
            "catalog:changed-source-with-old-wrapper-digest",
            "catalog:changed-id-with-old-id-set",
            "catalog:prepared-payload-digest-mismatch",
            "graph-edge:duplicate-logical-row",
            "comparator-graph-edge:reverse-logical-row-order",
            "graph-edge:logical-id-mismatch",
            "graph-edge:physical-id-mismatch",
            "graph-edge:owner-mismatch",
            "graph-edge:incarnation-mismatch",
            "graph-edge:transaction-mismatch",
            "graph-edge:generation-mismatch",
            "graph-edge:producer-mismatch",
            "graph-edge:scope-mismatch",
            "graph-node:duplicate-logical-row",
            "graph-node:reverse-logical-row-order",
            "graph-node:physical-id-mismatch",
            "graph-node:owner-mismatch",
            "graph-node:incarnation-mismatch",
            "graph-node:transaction-mismatch",
            "graph-node:generation-mismatch",
            "graph-node:doc-kind-mismatch",
            "graph:logical-edge-set-mismatch",
            "graph:logical-node-set-mismatch",
            "graph:edge-physical-set-mismatch",
            "graph:node-physical-set-mismatch",
            "graph:core-mismatch",
            "graph:token-mismatch",
            "graph:projection-mismatch",
        ],
        case_expectations={
            "data:changed-source-with-old-content-digest": {
                "expected_error_kind": "CrossFieldMismatch",
                "expected_reason_contains": (
                    "data replay content differs from prepared input or desired plan"
                ),
            },
            "data:changed-path-with-old-content-and-artifact-digests": {
                "expected_error_kind": "CrossFieldMismatch",
                "expected_reason_contains": (
                    "replay artifact digest differs from desired-plan tuple"
                ),
            },
        },
    )
    add(
        "replay-tuple-persisted-matrix",
        "persisted_bundle",
        "mutations::persisted_replay_tuple_matrix",
        "primary desired-plan replay tuples",
        "change one exact tuple/set/count/order field",
        "parse_error",
        [
            "wrong-artifact-kind",
            "wrong-projection-kind",
            "wrong-resource-key",
            "wrong-byte-length",
            "wrong-operation-count",
            "wrong-artifact-digest",
            "duplicate-tuple",
            "omit-tuple",
            "add-internally-valid-extra-tuple",
            "reverse-tuple-order",
            "same-cardinality-tuple-substitution",
            "projection-repeated-artifact-digest-mismatch",
            "projection-declared-count-mismatch",
            "replay-set-digest-mismatch",
        ],
    )
    add(
        "persisted-cross-file-join-matrix",
        "persisted_bundle",
        "mutations::persisted_cross_file_join_matrix",
        "all four primary persisted classes",
        "substitute or mutate one repeated cross-file identity while holding the rest",
        "parse_error",
        [
            "standalone-plan-bytes-from-generation-7-with-generation-1-begin",
            "standalone-plan-byte-change-with-begin-held",
            "embedded-plan-byte-change-with-standalone-held",
            "prepared-digest-mismatch-plan",
            "prepared-digest-mismatch-begin",
            "replay-set-digest-mismatch-plan",
            "replay-set-digest-mismatch-begin",
            "plan-digest-mismatch-begin",
            "owner-mismatch-prepared-plan",
            "incarnation-mismatch-prepared-plan",
            "manifest-mismatch-prepared-plan",
            "data-count-mismatch-prepared-plan-replay",
            "catalog-count-mismatch-prepared-plan-replay",
            "graph-edge-count-mismatch-prepared-plan-replay",
            "graph-node-count-mismatch-prepared-plan-replay",
            "data-id-digest-mismatch-prepared-plan-replay",
            "data-content-digest-mismatch-prepared-plan-replay",
            "catalog-id-digest-mismatch-prepared-plan-replay",
            "catalog-content-digest-mismatch-prepared-plan-replay",
            "graph-core-mismatch-prepared-plan-replay",
            "begin-expected-owner-mismatch-plan-owner",
            "begin-expected-sequence-mismatch-plan-predecessor",
            "begin-publication-root-changed-owner-root-prefix-join-rejects",
            "begin-publication-prefix-changed-owner-root-prefix-join-rejects",
            "begin-publication-incarnation-changed-data-route-name-join-rejects",
        ],
        case_expectations={
            "begin-expected-owner-mismatch-plan-owner": {
                "expected_error_kind": "CrossFieldMismatch",
                "expected_reason_contains": (
                    "expected publication owner/sequence mismatch"
                ),
            },
            "begin-expected-sequence-mismatch-plan-predecessor": {
                "expected_error_kind": "CrossFieldMismatch",
                "expected_reason_contains": (
                    "expected publication owner/sequence mismatch"
                ),
            },
            "begin-publication-root-changed-owner-root-prefix-join-rejects": {
                "expected_error_kind": "CrossFieldMismatch",
                "expected_reason_contains": (
                    "publication owner does not match root/prefix"
                ),
            },
            "begin-publication-prefix-changed-owner-root-prefix-join-rejects": {
                "expected_error_kind": "CrossFieldMismatch",
                "expected_reason_contains": (
                    "publication owner does not match root/prefix"
                ),
            },
            "begin-publication-incarnation-changed-data-route-name-join-rejects": {
                "expected_error_kind": "CrossFieldMismatch",
                "expected_reason_contains": "publication data route/name mismatch",
            },
        },
    )
    add(
        "binary-framing-persisted-matrix",
        "persisted_bundle",
        "mutations::persisted_binary_framing_matrix",
        "prepared and desired-plan canonical preimages",
        "apply one exact domain/width/field/count framing defect",
        "parse_error",
        [
            f"{baseline}:{case}"
            for baseline in ["prepared", "desired-plan"]
            for case in [
                "delete-domain-terminal-nul",
                "move-domain-terminal-nul",
                "wrong-format-version",
                "u32-little-endian",
                "u64-little-endian",
                "u32-wrong-width",
                "u64-wrong-width",
                "omit-field",
                "swap-adjacent-fields",
                "collection-count-minus-one",
                "collection-count-plus-one",
                "collection-count-encoded-twice",
                "trailing-byte",
            ]
        ],
    )
    add(
        "ordering-persisted-matrix",
        "persisted_bundle",
        "mutations::persisted_ordering_matrix",
        "comparator fixture complete arrays",
        "encode one collection in reverse of its normative comparator",
        "parse_error",
        [
            "prepared-data-routes:zeta-before-alpha",
            "data-rows:doc-z-before-doc-a",
            "catalog-rows:wrap-z-before-wrap-a",
            "logical-edges:larger-id-before-smaller-id",
            "logical-nodes:z-index-before-a-index",
            "data-plan:zeta-before-alpha",
            "replay-tuples:descending-comparator",
            "reserved-resource-keys:descending-raw-utf8",
            "mapping-reservations:descending-projection-resource",
            "publication-data:zeta-before-alpha",
        ],
    )
    add(
        "publication-expectation-begin-matrix",
        "persisted_bundle",
        "mutations::publication_expectation_begin_matrix",
        "prior publication, absent/present expectations, absent/present sync begin",
        "change one exact closed attachment or predecessor relation",
        "parse_error",
        [
            "publication:storage-incarnation",
            "publication:data-projection",
            "publication:catalog-projection",
            "publication:graph-projection",
            "publication:data-seal",
            "publication:catalog-seal",
            "publication:edge-seal",
            "publication:node-seal",
            "publication:document-count",
            "publication:final-write-sequence",
            "publication:attached-digest",
            "expectation:absent-sequence-nonzero",
            "expectation:present-publication-body",
            "expectation:present-publication-digest",
            "expectation:present-owner",
            "expectation:present-incarnation",
            "expectation:present-seal",
            "begin:plan-base64-extra-padding",
            "begin:plan-base64-wrong-decoded-length",
            "begin:plan-digest",
            "begin:prepared-input-digest",
            "begin:replay-set-digest",
            "begin:attached-self-digest",
        ],
    )
    add(
        "fresh-input-duplicates",
        "fresh_derivation",
        "mutations::fresh_duplicate_input_matrix",
        "typed logical input constructors",
        "insert one duplicate primary identity before fresh preparation",
        "parse_error",
        [
            "duplicate-document-id",
            "duplicate-wrapper-id",
            "duplicate-data-slug",
            "duplicate-logical-edge-row",
            "duplicate-logical-node-key",
        ],
    )
    add(
        "fresh-logical-content-changes",
        "fresh_derivation",
        "mutations::fresh_logical_content_changed_chain",
        "valid primary logical inputs",
        "change one valid logical value, or one explicitly coordinated logical identity, and rebuild all descendants",
        "changed_chain",
        [
            "document-source-byte",
            "document-and-manifest-id",
            "catalog-source-byte",
            "wrapper-id",
            "logical-edge-row",
            "logical-node-row",
            "data-mapping-byte",
            "catalog-mapping-byte",
            "edge-mapping-byte",
            "node-mapping-byte",
            "extractor-config-byte",
            "generation-1-to-7",
        ],
    )
    add(
        "compile-time-semantic-swaps",
        "compile_time",
        "ui::public_surface_and_privacy_contract",
        "crate-root typed API",
        "attempt one forbidden type substitution or private construction/access",
        "compile_error",
        [
            "root-for-prefix",
            "prefix-for-root",
            "slug-for-logical-index",
            "logical-index-for-slug",
            "data-mapping-for-catalog-mapping",
            "catalog-mapping-for-data-mapping",
            "document-id-for-wrapper-id",
            "wrapper-id-for-document-id",
            "brain-for-extractor-identity",
            "extractor-identity-for-brain",
            "sequence-for-generation",
            "generation-for-sequence",
            "later-target-value-in-prepare",
            "later-plan-value-in-transaction",
            "construct-controlled-byte-wrapper",
            "construct-mapping-reservation",
            "mutate-mapping-json",
            "swap-persisted-prepared-and-plan-wrappers",
            "inspect-persisted-wrapper-bytes",
            "access-private-codec",
            "access-raw-digest-bytes",
            "cross-digest-domain-substitution",
            "serde-digest",
            "construct-private-scalar-field",
        ],
    )
    add(
        "checked-arithmetic-private-matrix",
        "private_unit",
        "codec::tests::checked_arithmetic_matrix",
        "test-only checked length and quota helpers",
        "exercise exact success boundary and each named overflow edge",
        "parse_error",
        [
            "u128-length-u64-max-plus-one",
            "mapping-sum-u64-overflow",
            "artifact-sum-u64-overflow",
            "operation-sum-u64-overflow",
            "operation-times-64-overflow",
            "resource-times-4096-overflow",
            "stage-add-first-overflow",
            "stage-add-second-overflow",
            "stage-add-third-overflow",
        ],
    )
    add(
        "u64-max-length-control",
        "private_unit",
        "codec::tests::checked_arithmetic_matrix",
        "test-only checked length helper",
        "convert exactly u64::MAX",
        "success",
        ["u128-length-u64-max"],
    )
    add(
        "u64-max-generation-control",
        "fresh_derivation",
        "mutations::u64_max_generation_succeeds",
        "valid primary logical input",
        "plan with Generation::new(u64::MAX)",
        "success",
        ["generation-u64-max-with-231-byte-bounded-name"],
    )
    add(
        "physical-name-bound",
        "fresh_derivation",
        "mutations::physical_name_bound_matrix",
        "generated physical data name",
        "cross one exact scalar/name bound",
        "parse_error",
        [
            "rendered-name-232-bytes",
            "invalid-hidden-name-grammar",
            "resource-key-1025-bytes",
        ],
    )
    resource_key_length = len(goldens["generation_1"]["artifacts"][0]["resource_key"].encode())
    add(
        "resource-key-nul-position-cartesian",
        "public_parser",
        "resource_key_nul::embedded_nul_is_rejected_at_every_position",
        "generation_1.artifacts[0].resource_key",
        "insert one NUL at each UTF-8 byte boundary including both ends",
        "parse_error",
        [],
        case_ranges=[
            {
                "baseline": "catalog-resource-key",
                "first_offset": 0,
                "last_offset": resource_key_length,
                "count": resource_key_length + 1,
            }
        ],
    )

    tier_counts: dict[str, dict[str, int]] = {}
    runner_counts: dict[str, dict[str, Any]] = {}
    for row in rows:
        tier = tier_counts.setdefault(
            row["semantic_tier"], {"rows": 0, "cases": 0}
        )
        tier["rows"] += 1
        tier["cases"] += row["case_count"]
        runner = runner_counts.setdefault(
            row["runner"], {"rows": 0, "cases": 0, "outcomes": []}
        )
        runner["rows"] += 1
        runner["cases"] += row["case_count"]
        if row["outcome"] not in runner["outcomes"]:
            runner["outcomes"].append(row["outcome"])

    declared_row_count = len(rows)
    declared_case_count = sum(row["case_count"] for row in rows)
    integration_rows = [row for row in rows if row["runner"].startswith("mutations::")]
    compile_rows = [row for row in rows if row["runner"].startswith("ui::")]
    unit_rows = [row for row in rows if row["runner"].startswith("codec::")]
    nul_rows = [
        row for row in rows if row["runner"].startswith("resource_key_nul::")
    ]

    # These are recorded observations from the named Rust commands, not
    # expectations manufactured from the ledger. The gate checks below derive
    # completion by comparing them to the current rows. If a row or case is
    # added, removed, or renamed without a new matching run, completion becomes
    # false instead of silently following the edited declaration.
    observations = {
        "integration": {
            "command": "cargo test -p xerj-corpus-publication --test mutations --no-fail-fast",
            "passed_test_functions": 27,
            "failed_test_functions": 0,
            "registry_rows": 30,
            "registry_cases": 28797,
            "registry_exact": True,
        },
        "unit": {
            "command": "cargo test -p xerj-corpus-publication --lib codec::tests::checked_arithmetic_matrix",
            "passed_cases": 10,
            "failed_cases": 0,
        },
        "compile": {
            "command": "cargo test -p xerj-corpus-publication --test ui public_surface_and_privacy_contract",
            "passed_fixtures": 24,
            "failed_fixtures": 0,
        },
        "resource_key_nul": {
            "command": "cargo test -p xerj-corpus-publication --test resource_key_nul embedded_nul_is_rejected_at_every_position",
            "passed_cases": 125,
            "failed_cases": 0,
        },
    }

    expectations = {
        "integration": {
            "runner_functions": len(integration_rows),
            "test_functions_including_registry": len(integration_rows) + 1,
            "runner_cases": sum(row["case_count"] for row in integration_rows),
            "registry_rows": declared_row_count,
            "registry_cases": declared_case_count,
        },
        "unit": {
            "rows": len(unit_rows),
            "cases": sum(row["case_count"] for row in unit_rows),
        },
        "compile": {
            "rows": len(compile_rows),
            "fixtures": sum(row["case_count"] for row in compile_rows),
        },
        "resource_key_nul": {
            "rows": len(nul_rows),
            "cases": sum(row["case_count"] for row in nul_rows),
        },
    }
    gate_passed = {
        "integration": (
            observations["integration"]["failed_test_functions"] == 0
            and observations["integration"]["passed_test_functions"]
            == expectations["integration"]["test_functions_including_registry"]
            and observations["integration"]["registry_exact"]
            and observations["integration"]["registry_rows"]
            == expectations["integration"]["registry_rows"]
            and observations["integration"]["registry_cases"]
            == expectations["integration"]["registry_cases"]
        ),
        "unit": (
            observations["unit"]["failed_cases"] == 0
            and observations["unit"]["passed_cases"] == expectations["unit"]["cases"]
        ),
        "compile": (
            observations["compile"]["failed_fixtures"] == 0
            and observations["compile"]["passed_fixtures"]
            == expectations["compile"]["fixtures"]
        ),
        "resource_key_nul": (
            observations["resource_key_nul"]["failed_cases"] == 0
            and observations["resource_key_nul"]["passed_cases"]
            == expectations["resource_key_nul"]["cases"]
        ),
    }

    def execution_gate(row: dict[str, Any]) -> str:
        if row["runner"].startswith("mutations::"):
            return "integration"
        if row["runner"].startswith("ui::"):
            return "compile"
        if row["runner"].startswith("codec::"):
            return "unit"
        if row["runner"].startswith("resource_key_nul::"):
            return "resource_key_nul"
        raise AssertionError(f"unclassified mutation runner: {row['runner']}")

    for row in rows:
        gate = execution_gate(row)
        row["execution_gate"] = gate
        row["execution_status"] = (
            "executed_passed" if gate_passed[gate] else "incomplete"
        )

    execution_complete = all(gate_passed.values()) and all(
        row["execution_status"] == "executed_passed" for row in rows
    )
    failed_gates = [gate for gate, passed in gate_passed.items() if not passed]
    gates = {
        gate: {
            "status": "passed" if gate_passed[gate] else "incomplete",
            "expected": expectations[gate],
            "observed": observations[gate],
        }
        for gate in ["integration", "unit", "compile", "resource_key_nul"]
    }

    return {
        "format_version": 2,
        "contract": (
            "Every row has an exact tier, Rust runner, baseline, mutation rule, "
            "closed outcome, concrete cases or bounded offset ranges, and derived case_count. "
            "Persisted false quota/tuple/cross-file/artifact states require parse_error; "
            "valid fresh logical changes require changed_chain."
        ),
        "execution_complete": execution_complete,
        "execution_blocker": None if execution_complete else failed_gates,
        "execution_evidence": {
            "status": "complete" if execution_complete else "incomplete",
            "derivation": (
                "Completion is true only when each recorded observation exactly matches "
                "the row/case expectations derived from this ledger and every row's "
                "classified execution gate passes."
            ),
            "declared_row_count": declared_row_count,
            "declared_case_count": declared_case_count,
            "gates": gates,
            "generator_role": "records independently completed Rust execution; does not invoke the Rust crate",
        },
        "execution_policy": {
            "persisted_false_state": "parse_error",
            "valid_fresh_rebuild": "changed_chain",
            "canonical_member_reordering": "unchanged",
            "standalone_unjoined_binary_flip": "parse_error_or_changed_chain",
            "compile_time_brand_violation": "compile_error",
            "valid_boundary_control": "success",
            "completion_rule": "execution_complete is derived by exact equality between current ledger expectations and recorded integration-registry, unit, compile-fixture, and NUL observations",
        },
        "summary": {
            "row_count": declared_row_count,
            "case_count": declared_case_count,
            "tiers": tier_counts,
            "runners": runner_counts,
        },
        "rows": rows,
    }


def provenance(
    generator_bytes: bytes, mutation_result: dict[str, Any]
) -> dict[str, Any]:
    reference_path = HERE.parent.parent / "tests" / "support" / "reference_codec.rs"
    reference_bytes = reference_path.read_bytes()
    return {
        "format_version": 2,
        "authority_plan": {
            "path": "benchmarks/wordpress-autoindex-2026-08-10/CORPUS_PUBLICATION_AUTHORITY_PLAN.md",
            "bytes": 220497,
            "sha256": "bacf93cbaa1d522b1292d895d306b17edf26d4f950f6c71a6422a6abd01996bf",
        },
        "first_slice_plan": {
            "path": "benchmarks/wordpress-autoindex-2026-08-10/PUBLICATION_AUTHORITY_FIRST_SLICE_PLAN.md",
            "bytes": 66522,
            "sha256": "c408734596551662255b7615046c61b044978e01116d0ed5324ef703eb391cb6",
        },
        "implementation_remediation_ledger": {
            "path": "benchmarks/wordpress-autoindex-2026-08-10/PUBLICATION_AUTHORITY_FIRST_SLICE_IMPLEMENTATION_REMEDIATION.md",
            "bytes": 24231,
            "sha256": "2e035b2a1ae998f87876e46a3a7f100d4b64375b72d8276abdb0611eba1618b0",
        },
        "review_11": {
            "path": "benchmarks/wordpress-autoindex-2026-08-10/CORPUS_PUBLICATION_AUTHORITY_REVIEW_11.md",
            "bytes": 16338,
            "sha256": "e397e80588fd7242a98bca73708d7344311c39e6c5f4c1e7fb6b4b8693593201",
        },
        "reviewed_source": {
            "commit": "18f85755b2f25d2a4eefb6b7539a129f017ef6b9",
            "tree": "20a6a8c450ba7aa86979d78bb3a90c0fdef783df",
        },
        "candidate_source": {
            "status": "attested_preceding_source_commit",
            "base_commit": "aa142d6772a046baa9d5728328737020d3d05818",
            "base_tree": "2f9e469b1f1e12ab9005e0f666ddb1ff2cd680b9",
            "candidate_commit": "a214f1587df2d39f38e1017b4ebe4766715e3716",
            "candidate_tree": "3eae5b0efbac8598d9939c76272e585874124a4a",
            "claim": (
                "This attestation pins the exact preceding source commit and tree. The final "
                "provenance-attestation commit changes evidence metadata and oracle assertions "
                "only; it intentionally does not and cannot self-pin its own commit or tree."
            ),
        },
        "attestation_commit": {
            "changes_evidence_metadata_only": True,
            "self_pins": False,
            "pinned_preceding_commit": "a214f1587df2d39f38e1017b4ebe4766715e3716",
            "pinned_preceding_tree": "3eae5b0efbac8598d9939c76272e585874124a4a",
            "statement": (
                "The attestation commit is deliberately outside the pinned source identity. "
                "Its only purpose is to record that already-committed source identity and "
                "update the assertions that verify this evidence."
            ),
        },
        "fence_prerequisite": "d8b243023f3c325c3c433cd384ad74ca4e12af51",
        "oracle_sections": [
            {
                "heading": "Normative binary encoding and widths",
                "authority_plan_lines": "1374-1516",
                "use": "binary framing, publication/prepared/replay/desired-plan formulas",
            },
            {
                "heading": "Normative component identities and projection preimages",
                "authority_plan_lines": "1518-1713",
                "use": "identity domains, artifacts, projections, storage incarnations and seals",
            },
            {
                "heading": "Checked-in golden bytes and digests",
                "authority_plan_lines": "1715-1873",
                "use": "published primary preimages and rendered digests",
            },
            {
                "heading": "Review-5 target-bearing artifact oracles",
                "authority_plan_lines": "1875-1921",
                "use": "published primary NDJSON bytes and typed empty vectors",
            },
        ],
        "first_slice_sections": [
            {
                "heading": "Implementation-adjudication closure",
                "first_slice_plan_lines": "48-172",
            },
            {
                "heading": "Golden and provenance contract",
                "first_slice_plan_lines": "957-991",
            },
            {
                "heading": "Required complete goldens",
                "first_slice_plan_lines": "992-1059",
            },
            {
                "heading": "Two-row ordering oracle",
                "first_slice_plan_lines": "1061-1073",
            },
            {
                "heading": "Comparator-complete two-distinct-row matrix",
                "first_slice_plan_lines": "1075-1102",
            },
            {
                "heading": "Mutation matrix",
                "first_slice_plan_lines": "1104-1157",
            },
        ],
        "independent_generator": {
            "id": "independent-python-review11-v2",
            "path": "generate.py",
            "bytes": len(generator_bytes),
            "sha256": raw_sha256(generator_bytes),
            "hash_methodology": (
                "SHA-256 over the exact checked-in generate.py bytes read before output "
                "serialization; generator output files are not inputs to this hash."
            ),
        },
        "reference_encoder": {
            "path": "../../tests/support/reference_codec.rs",
            "bytes": len(reference_bytes),
            "sha256": raw_sha256(reference_bytes),
            "status": "pinned_in_preceding_source_commit; unchanged_by_attestation",
        },
        "generated_at": "2026-08-12T00:10:39+02:00",
        "toolchain": (
            "Python 3.13.5 hashlib/json/base64/ctypes plus system libxxhash.so.0 "
            "for independently specified XXH3-128"
        ),
        "generation_policy": (
            "The generator derives every expectation from literal protocol domains and "
            "independent U32/U64/S/A encoders. It never imports, calls, or harvests the Rust crate."
        ),
        "generator_scope": [
            "primary, generation-7, present-transition and comparator complete chains",
            "mapping reservations, mapping array, exact quota and revised desired plans",
            "canonical catalog/data/graph-edge/graph-node replay order",
            "all four persisted byte classes and fresh-versus-rehydrate expected identities",
            "two-empty same-kind and two-nonempty positional fixtures",
            "concrete tiered mutation ledger with derived runner/case totals and gate-derived Rust execution evidence",
        ],
        "generated_extensions": True,
        "plan_published_vectors_copied_exactly": True,
        "mutation_execution": {
            "status": mutation_result["execution_evidence"]["status"],
            "execution_complete": mutation_result["execution_complete"],
            "ledger_rows": mutation_result["summary"]["row_count"],
            "ledger_cases": mutation_result["summary"]["case_count"],
            "derivation": mutation_result["execution_evidence"]["derivation"],
            "gates": mutation_result["execution_evidence"]["gates"],
            "generator_boundary": "The Python generator records this Rust evidence but still never imports or invokes the production crate.",
        },
        "incomplete_scope": [],
    }


def generate() -> tuple[dict[str, Any], bytes]:
    prepared = prepare_oracle()
    gen1 = generation_oracle(prepared, 0, 1, 1)
    gen7 = generation_oracle(prepared, 0, 1, 7)
    present_plan = generation_oracle(prepared, 1, 2, 2)
    publication = publication_oracle(prepared, gen1)
    ordering_prepared = ordering_prepare_oracle()
    ordering_plan = ordering_generation_oracle(ordering_prepared)
    ordering_publication = ordering_publication_oracle(ordering_prepared, ordering_plan)
    ordering_predecessor = ordering_publication_oracle(
        ordering_prepared,
        ordering_plan,
        sequence_override=ordering_plan["expected_sequence"],
    )
    absent_expected = expectation("absent", prepared["owner"]["rendered"])
    present_expected = expectation(
        "present", prepared["owner"]["rendered"], publication["publication"]
    )
    ordering_expected = expectation(
        "present",
        ordering_prepared["owner"]["rendered"],
        ordering_predecessor["publication"],
    )
    gen1_begin = sync_begin(
        absent_expected, gen1, prepared["prepared_input"]["rendered"]
    )
    gen7_begin = sync_begin(
        absent_expected, gen7, prepared["prepared_input"]["rendered"]
    )
    present_begin = sync_begin(
        present_expected, present_plan, prepared["prepared_input"]["rendered"]
    )
    ordering_begin = sync_begin(
        ordering_expected,
        ordering_plan,
        ordering_prepared["prepared_input"]["rendered"],
    )
    two_empty = two_empty_route_oracle()
    ordering_data_artifacts = [
        (position, item)
        for position, item in enumerate(ordering_plan["artifacts"])
        if item["projection_kind"] == "data"
    ]
    two_nonempty = {
        "prepared": strip_private(ordering_prepared),
        "planned": ordering_plan,
        "predecessor_publication": ordering_predecessor,
        "expectation": ordering_expected,
        "sync_begin": ordering_begin,
        "bundle": durable_bundle_oracle(
            ordering_prepared, ordering_plan, ordering_begin, ordering_expected
        ),
        "positional_assertions": {
            "data_route_count": 2,
            "data_artifact_positions": [
                position for position, _ in ordering_data_artifacts
            ],
            "data_artifact_bytes_distinct": len(
                {item["bytes_base64"] for _, item in ordering_data_artifacts}
            )
            == 2,
            "data_artifact_digests_distinct": len(
                {
                    item["digest"]["rendered"]
                    for _, item in ordering_data_artifacts
                }
            )
            == 2,
            "data_resources_distinct": len(
                {item["resource_key"] for _, item in ordering_data_artifacts}
            )
            == 2,
            "swapping_distinct_payloads": "parse_error",
            "omission_or_addition": "parse_error",
        },
    }
    result = {
        "format_version": 2,
        "generator": "independent-python-review11-v2",
        "prepared": strip_private(prepared),
        "generation_1": gen1,
        "generation_7": gen7,
        "generation_7_invariants": {
            "transaction_byte_identical": gen1["transaction"] == gen7["transaction"],
            "prepared_input_unchanged": True,
            "changed_descendants": [
                key
                for key in (
                    "generation_id", "physical_data_name", "catalog_generation_incarnation",
                    "graph_token", "edge_physical_ids", "node_physical_ids",
                    "edge_physical_id_set", "node_physical_id_set", "data_projection",
                    "catalog_projection", "graph_projection", "artifacts", "replay_set",
                    "reserved_resource_keys", "desired_plan",
                )
                if gen1[key] != gen7[key]
            ],
            "descendant_assertions": [
                {"path": "transaction", "relation": "byte_identical", "satisfied": gen1["transaction"] == gen7["transaction"]},
                {"path": "name_components.owner", "relation": "byte_identical", "satisfied": gen1["name_components"]["owner"] == gen7["name_components"]["owner"]},
                {"path": "name_components.slug", "relation": "byte_identical", "satisfied": gen1["name_components"]["slug"] == gen7["name_components"]["slug"]},
                {"path": "name_components.stage", "relation": "changed", "satisfied": gen1["name_components"]["stage"] != gen7["name_components"]["stage"]},
                *[
                    {"path": key, "relation": "changed", "satisfied": gen1[key] != gen7[key]}
                    for key in (
                        "generation_id", "physical_data_name", "catalog_generation_incarnation",
                        "graph_token", "edge_physical_ids", "node_physical_ids",
                        "edge_physical_id_set", "node_physical_id_set", "data_projection",
                        "catalog_projection", "graph_projection", "artifacts", "replay_set",
                        "reserved_resource_keys", "desired_plan",
                    )
                ],
                {"path": "quota.mapping_record_bodies_base64", "relation": "changed", "satisfied": gen1["quota"]["mapping_record_bodies_base64"] != gen7["quota"]["mapping_record_bodies_base64"]},
                {"path": "quota.total", "relation": "byte_identical", "satisfied": gen1["quota"]["total"] == gen7["quota"]["total"]},
            ],
        },
        "present_transition": present_plan,
        "prior_publication": publication,
        "expectations": {"absent": absent_expected, "present": present_expected},
        "sync_begins": {"absent": gen1_begin, "present": present_begin},
        "durable_bundles": {
            "primary_generation_1_absent": durable_bundle_oracle(
                prepared, gen1, gen1_begin, absent_expected
            ),
            "generation_7_absent": durable_bundle_oracle(
                prepared, gen7, gen7_begin, absent_expected
            ),
            "present_transition": durable_bundle_oracle(
                prepared, present_plan, present_begin, present_expected
            ),
            "comparator_present": durable_bundle_oracle(
                ordering_prepared, ordering_plan, ordering_begin, ordering_expected
            ),
        },
        "positional_fixtures": {
            "two_empty_same_kind": two_empty,
            "two_nonempty_distinct": two_nonempty,
        },
        "empty_vectors": empty_vectors(),
        "ordering_matrix": {
            "reverse_input": {
                "data_routes": ["zeta", "alpha"],
                "alpha_document_ids": ["doc-b", "doc-a"],
                "catalog_wrapper_ids": ["wrap-z", "wrap-a"],
                "logical_edge_ids": [
                    item[0] for item in reversed(ordering_prepared["_edge_tuples"])
                ],
                "logical_nodes": [
                    [item[0], item[1]] for item in reversed(ordering_prepared["_node_tuples"])
                ],
            },
            "canonical_order": {
                "data_routes": [item["slug"] for item in ordering_prepared["data_routes"]],
                "data_document_ids": [
                    [item["slug"], [row[0] for row in item["rows"]]]
                    for item in ordering_prepared["data_routes"]
                ],
                "catalog_wrapper_ids": [row[0] for row in ordering_prepared["_catalog_rows"]],
                "logical_edge_ids": [item[0] for item in ordering_prepared["_edge_tuples"]],
                "logical_nodes": [[item[0], item[1]] for item in ordering_prepared["_node_tuples"]],
                "data_plan_entries": [item["slug"] for item in ordering_plan["data_entries"]],
                "edge_physical_ids": sorted(item["rendered"] for item in ordering_plan["edge_physical_ids"]),
                "node_physical_ids": sorted(item["rendered"] for item in ordering_plan["node_physical_ids"]),
                "replay_tuples": ordering_plan["artifact_tuple_order"],
                "reserved_resource_keys": ordering_plan["reserved_resource_keys"],
                "quota_mapping_records": ordering_plan["mapping_record_order"],
            },
            "prepared": strip_private(ordering_prepared),
            "planned": ordering_plan,
            "prior_publication": ordering_publication,
            "predecessor_publication": ordering_predecessor,
            "expectation": ordering_expected,
            "sync_begin": ordering_begin,
            "rfc8785": {
                "mapping": ordering_prepared["data_routes"][0]["mapping"],
                "document_source": {
                    **canonical_json_fields(ordering_prepared["data_routes"][0]["rows"][0][1]),
                    "downstream_content_set": ordering_prepared["data_routes"][0]["content_set"],
                },
                "catalog_source": {
                    **canonical_json_fields(ordering_prepared["_catalog_rows"][0][1]),
                    "downstream_wrapper_set": ordering_prepared["catalog_wrapper_set"],
                },
                "extractor_config": ordering_prepared["extractor_config"],
            },
        },
        "coverage": {
            "complete": [
                "primary prepared identity graph and complete preimages",
                "generation-1 desired graph, artifacts, replay set, quota, plan",
                "generation-7 complete descendants with invariant transaction",
                "generation-2 present-predecessor plan",
                "storage incarnations, seals, publication body and closed JSON",
                "absent/present expectation bodies, JSON and digests",
                "absent/present sync-begin bodies, padded plan base64, JSON and digests",
                "typed empty sets and empty artifacts",
                "primary node logical-order versus rendered-ID-set order",
                "complete two-distinct comparator prepared/target/replay/quota/plan/publication chain",
                "all four persisted byte classes for primary, generation-7, present, and comparator bundles",
                "fresh-versus-rehydrate expected byte/getter identities for every complete bundle",
                "complete two-empty same-kind and two-nonempty positional bundle fixtures",
            ],
            "not_complete": [],
            "not_in_slice": [
                {
                    "requirement": "authority vectors and remote lifecycle",
                    "reason": "Explicitly deferred by the approved first-slice plan; this leaf is I/O-free.",
                }
            ],
        },
    }
    pub_json = publication["publication"]["canonical_json"].encode() + b"\n"
    return result, pub_json


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if checked-in outputs differ")
    args = parser.parse_args()
    result, publication = generate()
    generator_bytes = Path(__file__).read_bytes()
    mutation_result = mutation_ledger(result)
    provenance_result = provenance(generator_bytes, mutation_result)
    golden_bytes = (json.dumps(result, indent=2, sort_keys=True) + "\n").encode()
    mutation_bytes = (
        json.dumps(mutation_result, indent=2, sort_keys=True) + "\n"
    ).encode()
    provenance_bytes = (
        json.dumps(provenance_result, indent=2, sort_keys=True) + "\n"
    ).encode()
    golden_path = HERE / "goldens.json"
    publication_path = HERE / "publication.json"
    mutation_path = HERE / "mutations.json"
    provenance_path = HERE / "provenance.json"
    if args.check:
        failures = []
        if golden_path.read_bytes() != golden_bytes:
            failures.append(str(golden_path))
        if publication_path.read_bytes() != publication:
            failures.append(str(publication_path))
        if mutation_path.read_bytes() != mutation_bytes:
            failures.append(str(mutation_path))
        if provenance_path.read_bytes() != provenance_bytes:
            failures.append(str(provenance_path))
        if failures:
            raise SystemExit("generated outputs differ: " + ", ".join(failures))
    else:
        golden_path.write_bytes(golden_bytes)
        publication_path.write_bytes(publication)
        mutation_path.write_bytes(mutation_bytes)
        provenance_path.write_bytes(provenance_bytes)
    print(
        json.dumps(
            {
                "goldens_sha256": raw_sha256(golden_bytes),
                "mutations_sha256": raw_sha256(mutation_bytes),
                "publication_sha256": raw_sha256(publication.rstrip(b"\n")),
                "provenance_sha256": raw_sha256(provenance_bytes),
                "coverage_complete": len(result["coverage"]["complete"]),
                "coverage_not_complete": len(result["coverage"]["not_complete"]),
                "mutation_rows": mutation_result["summary"]["row_count"],
                "mutation_cases": mutation_result["summary"]["case_count"],
                "mutation_execution_complete": mutation_result["execution_complete"],
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
