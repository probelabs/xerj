#!/usr/bin/env python3
"""Rust AST security substrate extractor (tree-sitter-rust).

Turns a Rust workspace into three NDJSON streams that a search engine can answer
whitebox-security questions against:

  functions.ndjson  one record per function/method, with the security-relevant
                    facts an auditor filters on (unsafe ops, panic sites, `as`
                    casts, allocation shapes, axum extractors, sinks, validators,
                    lock-across-await) plus the body text so an agent can read
                    the function without opening the file.
  calls.ndjson      caller -> callee edges, so interprocedural reach
                    (handler -> ... -> sink) is a query, not a guess.
  routes.ndjson     HTTP method + path pattern -> handler function, so
                    "network-reachable" is a fact rather than an assumption.

Unlike a grep-style scanner, the AST gives each fact a *scope*: "unvalidated
request parameter and a filesystem sink in the same function" is expressible.

Usage:
    python3 rust_ast_index.py <workspace-root> [--out DIR]

Requires: tree-sitter, tree-sitter-rust.
"""

import argparse
import json
import os
import sys
from collections import Counter

try:
    import tree_sitter_rust
    from tree_sitter import Language, Parser
except ImportError:  # pragma: no cover
    sys.exit("need: pip install tree-sitter==0.26.0 tree-sitter-rust==0.24.2")


# ── security signal vocabulary ────────────────────────────────────────────────
# Substring patterns matched against a function's own source text. The AST
# supplies the scoping and the call edges; these supply the signatures.

UNSAFE_OPS = {
    "transmute": ["transmute"],
    "from_raw": ["from_raw", "from_raw_parts"],
    "unchecked_index": ["get_unchecked", "get_unchecked_mut"],
    "set_len": ["set_len"],
    "assume_init": ["assume_init"],
    "static_mut": ["static mut"],
    "ffi_call": ["extern \"C\"", "libc::"],
    "raw_deref": ["*mut ", "*const "],
    "uninit": ["MaybeUninit", "mem::uninitialized", "zeroed()"],
    "ptr_arith": ["ptr::", ".offset(", ".add(", ".sub("],
    "ptr_reborrow": ["&mut *", "&*"],
    "mmap": ["Mmap::map", "MmapMut::map", "memmap2::"],
    "utf8_unchecked": ["from_utf8_unchecked"],
    "slice_raw_parts": ["slice::from_raw_parts", "from_raw_parts"],
}

PANIC_OPS = {
    "unwrap": [".unwrap()"],
    "expect": [".expect("],
    "panic": ["panic!("],
    "unreachable": ["unreachable!("],
    "todo": ["todo!(", "unimplemented!("],
    "assert": ["assert!(", "assert_eq!(", "assert_ne!("],
    "abort": ["process::abort", ".abort()", "process::exit"],
    "unwrap_unchecked": ["unwrap_unchecked"],
    "slice_from_str": ["from_utf8_unchecked"],
}

ALLOC_OPS = {
    "with_capacity": ["with_capacity("],
    "reserve": [".reserve(", ".reserve_exact("],
    "vec_repeat": ["vec!["],
    "repeat": [".repeat("],
    "collect": [".collect()", ".collect::<"],
    "extend": [".extend("],
    "to_vec": [".to_vec()"],
}

# axum / tower extractor types seen in handler signatures.
EXTRACTORS = [
    "Path<", "Query<", "Json<", "OptionalJson<", "State<", "Extension<",
    "Multipart", "Body", "Bytes", "HeaderMap", "TypedHeader", "Form<",
    "RawQuery", "ConnectInfo<", "WebSocketUpgrade", "Request<",
]

SINKS = {
    "fs_read": ["fs::read", "File::open", "read_to_string", "read_dir"],
    "fs_write": ["fs::write", "File::create", "create_dir_all", "OpenOptions"],
    "fs_delete": ["remove_dir_all", "remove_file", "remove_dir"],
    "fs_path_join": [".join(", "PathBuf::from", "Path::new"],
    "fs_rename": ["fs::rename", "hard_link", "symlink"],
    "process": ["Command::new", "process::Command"],
    "deserialize": [
        "serde_json::from_", "from_slice", "from_reader", "from_str::<",
        "bincode::", "rmp_serde", "serde_yaml::from_",
    ],
    "net_egress": ["reqwest::", "TcpStream::connect", "hyper::Client", "Client::new"],
    "spawn": ["tokio::spawn", "thread::spawn", "spawn_blocking"],
    "sql_ish": [".query(", ".execute("],
    "log_secret": ["api_key", "password", "secret", "token"],
}

VALIDATORS = {
    "explicit_validate": ["validate", "is_valid", "check_"],
    "typed_name": ["IndexName::new", "IndexName::validate", "Namespace::new"],
    "checked_arith": ["checked_add", "checked_mul", "checked_sub", "checked_div"],
    "saturating": ["saturating_add", "saturating_mul", "saturating_sub"],
    "try_convert": ["try_from", "try_into", "TryFrom"],
    "containment": ["starts_with", "canonicalize", "contains(\"..\")"],
    "bounds": ["is_empty()", ".len() >", ".len() <", "min(", "max(", "clamp("],
    "auth": ["authorize", "authenticate", "require_", "has_permission", "verify_"],
}

CONCURRENCY = {
    "lock_acquire": [".lock()", ".read()", ".write()", ".lock().await",
                     ".read().await", ".write().await"],
    "relaxed_atomic": ["Ordering::Relaxed"],
    "spawn_blocking": ["spawn_blocking"],
    "block_on": ["block_on", "blocking_recv", "blocking_send"],
    "dashmap": ["DashMap", "dashmap::"],
}


def hits(text, table):
    """Which categories in `table` appear in `text`."""
    return sorted(k for k, pats in table.items() if any(p in text for p in pats))


class Extractor:
    def __init__(self, root):
        self.root = os.path.abspath(root)
        self.parser = Parser(Language(tree_sitter_rust.language()))
        self.functions = []
        self.calls = []
        self.routes = []
        self.stats = Counter()
        self.per_file = []
        self.error_files = []

    # ── traversal ────────────────────────────────────────────────────────────

    def run(self):
        for path in self.rust_files():
            self.parse_file(path)
        return self

    def rust_files(self):
        out = []
        for dirpath, dirnames, filenames in os.walk(self.root):
            dirnames[:] = [d for d in dirnames if d not in ("target", ".git")]
            for fn in filenames:
                if fn.endswith(".rs"):
                    out.append(os.path.join(dirpath, fn))
        return sorted(out)

    def parse_file(self, path):
        src = open(path, "rb").read()
        tree = self.parser.parse(src)
        rel = os.path.relpath(path, self.root)
        crate = self.crate_of(rel)

        err = self.count_errors(tree.root_node)
        total_fns = self.count_functions(tree.root_node)
        before = len(self.functions)

        self.walk(tree.root_node, src, rel, crate, mod_path=[], owner=None)
        self.scan_routes(src, rel, crate)

        emitted = len(self.functions) - before
        self.stats["files"] += 1
        self.stats["error_nodes"] += err
        self.stats["ast_functions"] += total_fns
        self.stats["emitted_functions"] += emitted
        if err:
            self.error_files.append({"file": rel, "error_nodes": err})
        self.per_file.append({
            "file": rel, "crate": crate, "bytes": len(src),
            "lines": src.count(b"\n") + 1,
            "ast_functions": total_fns, "emitted_functions": emitted,
            "error_nodes": err,
        })

    def crate_of(self, rel):
        parts = rel.split(os.sep)
        if "crates" in parts:
            return parts[parts.index("crates") + 1]
        return parts[0] if parts else "?"

    def count_errors(self, node):
        n = 0
        stack = [node]
        while stack:
            cur = stack.pop()
            if cur.type == "ERROR" or cur.is_missing:
                n += 1
            stack.extend(cur.children)
        return n

    def count_functions(self, node):
        n = 0
        stack = [node]
        while stack:
            cur = stack.pop()
            if cur.type == "function_item":
                n += 1
            stack.extend(cur.children)
        return n

    def walk(self, node, src, rel, crate, mod_path, owner):
        """Descend, tracking module path and impl/trait owner for scoping."""
        for child in node.children:
            t = child.type

            if t == "mod_item":
                name = self.field_text(child, "name", src)
                body = child.child_by_field_name("body")
                if body is not None:
                    self.walk(body, src, rel, crate, mod_path + [name or "?"], owner)
                continue

            if t in ("impl_item", "trait_item"):
                type_node = child.child_by_field_name("type")
                trait_node = child.child_by_field_name("trait")
                name = self.field_text(child, "name", src)
                owner_name = (
                    self.text(type_node, src) if type_node is not None
                    else (name or "?")
                )
                trait_name = self.text(trait_node, src) if trait_node is not None else None
                is_unsafe_impl = b"unsafe impl" in self.text_bytes(child, src)[:32]
                body = child.child_by_field_name("body")
                if body is not None:
                    self.walk(
                        body, src, rel, crate, mod_path,
                        {"owner": owner_name, "trait": trait_name,
                         "kind": t, "unsafe_impl": is_unsafe_impl},
                    )
                continue

            if t == "function_item":
                self.emit_function(child, src, rel, crate, mod_path, owner)
                # Descend into the body: Rust allows nested `fn` items inside a
                # function (helpers in tests, local recursion). Without this the
                # inner ones are invisible and coverage silently drops below 100%.
                body = child.child_by_field_name("body")
                if body is not None:
                    inner_name = self.field_text(child, "name", src) or "?"
                    self.walk(
                        body, src, rel, crate, mod_path + [f"fn:{inner_name}"], owner
                    )
                continue

            self.walk(child, src, rel, crate, mod_path, owner)

    # ── record building ──────────────────────────────────────────────────────

    def emit_function(self, node, src, rel, crate, mod_path, owner):
        name = self.field_text(node, "name", src) or "?"
        params_node = node.child_by_field_name("parameters")
        ret_node = node.child_by_field_name("return_type")
        body_node = node.child_by_field_name("body")

        header = self.text(node, src)[: node.start_byte and 400 or 400]
        decl = self.text(node, src).split("{", 1)[0]
        body = self.text(body_node, src) if body_node is not None else ""
        params = self.text(params_node, src) if params_node is not None else "()"
        ret = self.text(ret_node, src) if ret_node is not None else ""

        fn_id = f"{rel}:{name}:{node.start_point[0] + 1}"
        unsafe_blocks = self.unsafe_blocks(node, src)
        casts = self.casts(node, src)
        index_exprs = self.index_exprs(node, src)

        extractors = [e for e in EXTRACTORS if e in params]
        param_names = self.param_names(params_node, src) if params_node else []
        # Taint provenance is ONE HOP: a size or path is usually not the parameter
        # itself but a local derived from it (`let fields = params.get(...)` then
        # `with_capacity(fields.len() * like.len())`). Measured on the PR #69 test
        # set, tracking direct parameters only MISSED two of six known bugs.
        tainted = param_names + self.derived_locals(body, param_names)
        alloc_from_param = self.alloc_from_param(body, tainted)
        alloc_args_all = self.alloc_args_all(body)

        record = {
            "id": fn_id,
            "crate": crate,
            "file": rel,
            "line_start": node.start_point[0] + 1,
            "line_end": node.end_point[0] + 1,
            "fn_name": name,
            "module_path": "::".join(mod_path),
            "owner": (owner or {}).get("owner"),
            "trait_impl": (owner or {}).get("trait"),
            "in_unsafe_impl": bool((owner or {}).get("unsafe_impl")),
            "is_test": self.is_test(node, src) or self.in_test_scope(rel, mod_path),
            # signature facts
            "signature": decl.strip()[:600],
            "params": params[:1500],
            "param_names": param_names,
            "return_type": ret[:300],
            "is_async": "async" in decl,
            "is_unsafe_fn": decl.strip().startswith("unsafe ") or " unsafe fn " in decl,
            "is_pub": decl.strip().startswith("pub"),
            "loc": node.end_point[0] - node.start_point[0] + 1,
            # unsafe
            "has_unsafe_block": bool(unsafe_blocks),
            "unsafe_block_count": len(unsafe_blocks),
            "unsafe_block_lines": [b["line"] for b in unsafe_blocks],
            "unsafe_ops": hits(" ".join(b["text"] for b in unsafe_blocks), UNSAFE_OPS),
            "unsafe_any": bool(unsafe_blocks) or bool(
                decl.strip().startswith("unsafe ")
            ),
            # panic / abort surface
            "panic_ops": hits(body, PANIC_OPS),
            "panic_count": sum(body.count(p) for pats in PANIC_OPS.values() for p in pats),
            "index_expr_count": len(index_exprs),
            "index_exprs": index_exprs[:20],
            "cast_count": len(casts),
            "casts": casts[:25],
            "narrowing_casts": [c for c in casts if self.is_narrowing(c)],
            "narrowing_cast_count": sum(1 for c in casts if self.is_narrowing(c)),
            "has_narrowing_cast": any(self.is_narrowing(c) for c in casts),
            # allocation. The blunt "calls with_capacity" filter is far too broad
            # to be an audit signal (hundreds of hits). What discriminates a DoS
            # shape is ARGUMENT PROVENANCE: a size derived from request input,
            # and especially a PRODUCT of two request-derived counts (the
            # cross-product blowup shape). Those are separate fields so a query
            # can ask for them directly.
            "alloc_ops": hits(body, ALLOC_OPS),
            "alloc_from_param": alloc_from_param,
            "alloc_from_param_count": len(alloc_from_param),
            "alloc_args": [a["arg"] for a in alloc_from_param][:12],
            "alloc_param_names": sorted({a["param"] for a in alloc_from_param}),
            # The cross-product blowup shape is a MULTIPLICATION in an allocation
            # size, whether or not provenance resolves. Gating this on parameter
            # provenance hid PR #69's F6, whose factors are locals.
            "alloc_product": any("*" in a for a in alloc_args_all),
            "alloc_args_all": alloc_args_all[:16],
            # what gets path-joined — "join tainted input" is the traversal shape
            "path_join_args": self.join_args(body, tainted)[:12],
            "path_join_from_param": bool(self.join_args(body, tainted)),
            # ORDER MATTERS: a validator that runs AFTER the destructive op is not
            # a guard. PR #69's F2 called IndexName::new *after* remove_dir_all —
            # a presence-only validator signal scores that function as "guarded".
            "guard_after_destructive_op": self.guard_after_sink(body),
            # recursion + depth guarding (the stack-overflow shape)
            "calls_self": (name + "(") in body,
            "has_depth_guard": any(
                p in body for p in ("DepthGuard", "MAX_QUERY_DEPTH", "depth +",
                                    "depth_guard", "recursion_limit", "depth >")
            ),
            "reads_config_limit": any(
                p in body for p in ("limits.max", "config().limits", "max_fields",
                                    "max_actions", "max_result", "max_pending")
            ),
            # network reachability
            "extractors": extractors,
            "is_handler_shaped": bool(extractors) and "async" in decl,
            # sinks / validators / concurrency
            "sinks": hits(body, SINKS),
            "validators": hits(body, VALIDATORS),
            "concurrency": hits(body, CONCURRENCY),
            "lock_across_await": self.lock_across_await(body),
            # readable body for the agent. Some functions in this codebase are
            # >1800 lines, so the flag tells an auditor when the indexed body is
            # partial and the real file must be opened before drawing conclusions.
            "body": body[:12000],
            "body_truncated": len(body) > 12000,
            "body_chars": len(body),
            "body_lines": body.count("\n") + 1,
        }
        self.functions.append(record)
        self.emit_calls(node, src, rel, fn_id, name)

    def emit_calls(self, node, src, rel, fn_id, caller):
        stack = [node]
        seen = set()
        while stack:
            cur = stack.pop()
            if cur.type == "call_expression":
                fn_node = cur.child_by_field_name("function")
                if fn_node is not None:
                    callee = self.text(fn_node, src).strip()
                    short = callee.split("::")[-1].split(".")[-1]
                    key = (callee, cur.start_point[0])
                    if key not in seen:
                        seen.add(key)
                        # A method call (`x.len()`) cannot be resolved to a
                        # definition without type inference: the receiver's type
                        # is unknown here. A free or path-qualified call
                        # (`foo()`, `Type::foo()`) can be. Callers that build a
                        # call graph MUST distinguish these — resolving `.len()`
                        # by name alone wires every `len` in the tree together
                        # and collapses the graph into one giant component.
                        self.calls.append({
                            "caller_id": fn_id, "caller": caller, "file": rel,
                            "line": cur.start_point[0] + 1,
                            "callee_path": callee[:200], "callee": short[:120],
                            "kind": "call",
                            "is_method": "." in callee,
                            "resolvable": "." not in callee,
                        })
            elif cur.type == "macro_invocation":
                m = cur.child_by_field_name("macro")
                if m is not None:
                    name = self.text(m, src)
                    key = (name, cur.start_point[0])
                    if key not in seen:
                        seen.add(key)
                        self.calls.append({
                            "caller_id": fn_id, "caller": caller, "file": rel,
                            "line": cur.start_point[0] + 1,
                            "callee_path": name + "!", "callee": name,
                            "kind": "macro", "is_method": False,
                            "resolvable": False,
                        })
            stack.extend(cur.children)

    def scan_routes(self, src, rel, crate):
        """Extract axum route registrations: .route("/p", get(handler))."""
        text = src.decode("utf8", "replace")
        idx = 0
        while True:
            i = text.find(".route(", idx)
            if i < 0:
                break
            idx = i + 7
            seg = text[i : i + 400]
            q1 = seg.find('"')
            if q1 < 0:
                continue
            q2 = seg.find('"', q1 + 1)
            if q2 < 0:
                continue
            path_pat = seg[q1 + 1 : q2]
            rest = seg[q2 + 1 : q2 + 260]
            for method in ("get", "post", "put", "delete", "patch", "head", "any"):
                token = method + "("
                mi = rest.find(token)
                if mi < 0:
                    continue
                handler = rest[mi + len(token) :]
                handler = handler.split(")")[0].split(",")[0].strip()
                if not handler:
                    continue
                self.routes.append({
                    "file": rel, "crate": crate,
                    "line": text.count("\n", 0, i) + 1,
                    "method": method.upper(), "path": path_pat,
                    "handler": handler.split("::")[-1][:120],
                    "handler_path": handler[:200],
                    "unauth_looking": "_cluster" not in path_pat,
                })

    # ── small helpers ────────────────────────────────────────────────────────

    def text(self, node, src):
        if node is None:
            return ""
        return src[node.start_byte : node.end_byte].decode("utf8", "replace")

    def text_bytes(self, node, src):
        return src[node.start_byte : node.end_byte]

    def field_text(self, node, field, src):
        n = node.child_by_field_name(field)
        return self.text(n, src) if n is not None else None

    def is_test(self, node, src):
        # attributes precede the function_item as siblings inside the parent.
        prev = node.prev_sibling
        seen = 0
        while prev is not None and seen < 4:
            if prev.type == "attribute_item":
                a = self.text(prev, src)
                if "test" in a or "cfg(test)" in a:
                    return True
            elif prev.type not in ("line_comment", "block_comment", "attribute_item"):
                break
            prev = prev.prev_sibling
            seen += 1
        return False

    def in_test_scope(self, rel, mod_path):
        """Integration-test files and any `mod tests` ancestor are test scope.

        Audit queries filter test code out: a panic in a test is not a DoS.
        """
        if "/tests/" in rel or rel.startswith("tests/") or "_tests.rs" in rel:
            return True
        return any(m in ("tests", "test") or m.endswith("_tests") for m in mod_path)

    def unsafe_blocks(self, node, src):
        out = []
        stack = [node]
        while stack:
            cur = stack.pop()
            if cur.type == "unsafe_block":
                out.append({
                    "line": cur.start_point[0] + 1,
                    "text": self.text(cur, src)[:2000],
                })
            stack.extend(cur.children)
        return sorted(out, key=lambda b: b["line"])

    def casts(self, node, src):
        out = []
        stack = [node]
        while stack:
            cur = stack.pop()
            if cur.type == "type_cast_expression":
                out.append(self.text(cur, src).replace("\n", " ")[:160])
            stack.extend(cur.children)
        return out

    NARROW = ("as u8", "as u16", "as u32", "as i8", "as i16", "as i32",
              "as usize", "as isize", "as f32")

    def is_narrowing(self, cast_text):
        return any(cast_text.rstrip().endswith(n) for n in self.NARROW)

    def index_exprs(self, node, src):
        out = []
        stack = [node]
        while stack:
            cur = stack.pop()
            if cur.type == "index_expression":
                out.append(self.text(cur, src).replace("\n", " ")[:120])
            stack.extend(cur.children)
        return out

    def param_names(self, params_node, src):
        names = []
        for child in params_node.children:
            if child.type == "parameter":
                pat = child.child_by_field_name("pattern")
                if pat is not None:
                    names.append(self.text(pat, src))
            elif child.type == "self_parameter":
                names.append("self")
        return names

    def alloc_from_param(self, body, param_names):
        """with_capacity/reserve whose argument mentions a parameter name."""
        out = []
        for pat in ("with_capacity(", ".reserve(", "vec![", ".repeat("):
            start = 0
            while True:
                i = body.find(pat, start)
                if i < 0:
                    break
                start = i + len(pat)
                arg = body[i + len(pat) : i + len(pat) + 120]
                arg = arg.split(")")[0]
                for p in param_names:
                    if p and p != "self" and p in arg:
                        out.append({"call": pat.strip("."), "arg": arg[:100],
                                    "param": p})
                        break
        return out

    def derived_locals(self, body, param_names):
        """Locals bound from an expression mentioning a parameter (one hop).

        `let fields = params.get("fields")` makes `fields` carry `params`' taint.
        Without this hop, an allocation sized by `fields.len()` reads as
        untainted — which is how PR #69's F6 and F8 escaped detection.
        """
        if not param_names:
            return []
        real = [p for p in param_names if p and p != "self"]
        if not real:
            return []
        out = []
        for line in body.split("\n"):
            s = line.strip()
            if not s.startswith("let "):
                continue
            if "=" not in s:
                continue
            lhs, rhs = s[4:].split("=", 1)
            if not any(p in rhs for p in real):
                continue
            name = lhs.replace("mut ", "").split(":")[0].strip()
            if name and name.replace("_", "").isalnum():
                out.append(name)
        return out

    @staticmethod
    def balanced_arg(body, open_at, opener="(", closer=")"):
        """Text of the argument list starting after `open_at`, paren-balanced.

        Splitting on the first `)` truncates `with_capacity(a.len() * b.len())`
        to `a.len(` — which silently loses the multiplication that IS the
        cross-product signal. Measured: that truncation hid PR #69's F6.
        """
        depth = 1
        i = open_at
        n = len(body)
        while i < n and depth > 0:
            c = body[i]
            if c == opener:
                depth += 1
            elif c == closer:
                depth -= 1
                if depth == 0:
                    return body[open_at:i]
            i += 1
        return body[open_at : min(n, open_at + 200)]

    def alloc_args_all(self, body):
        """Every allocation-size argument, regardless of provenance."""
        out = []
        for pat, closer in (("with_capacity(", ")"), (".reserve(", ")"),
                            ("vec![", "]"), (".repeat(", ")")):
            start = 0
            while True:
                i = body.find(pat, start)
                if i < 0:
                    break
                start = i + len(pat)
                opener = "[" if closer == "]" else "("
                arg = self.balanced_arg(body, i + len(pat), opener, closer)
                out.append(arg.strip()[:160])
        return out

    DESTRUCTIVE = ("remove_dir_all(", "remove_file(", "remove_dir(", "create_dir_all(",
                   "File::create(", "fs::write(", "fs::rename(")
    # Only STRONG name/path validators count for the ordering test. Generic
    # helpers like `starts_with(` appear all over a large function for unrelated
    # reasons (glob matching), and taking the earliest of those masks a genuinely
    # late path validator — that is what hid PR #69's F2 on the first pass.
    PATH_GUARDS = ("IndexName::new", "IndexName::validate", "Namespace::new",
                   "validate_snapshot", "canonicalize(", 'contains("..")')

    def guard_after_sink(self, body):
        """True when a path/name validator only appears AFTER a destructive op.

        Presence of a validator says nothing about whether it protects anything —
        ordering does. This is the exact shape of PR #69's F2: `IndexName::new`
        ran *after* `remove_dir_all` had already deleted the directory.
        """
        sink_at = min((body.find(p) for p in self.DESTRUCTIVE if p in body),
                      default=-1)
        if sink_at < 0:
            return False
        positions = [body.find(g) for g in self.PATH_GUARDS if g in body]
        if not positions:
            return False
        # unguarded-before-the-sink: no strong validator runs before it, but one
        # does run after — the validation is decorative.
        return min(positions) > sink_at

    def join_args(self, body, param_names):
        """`.join(x)` arguments that mention a parameter name.

        `dst.join(user_input)` is the path-traversal shape; `dir.join("fixed")`
        is not. Without the argument text the two are indistinguishable.
        """
        out = []
        start = 0
        while True:
            i = body.find(".join(", start)
            if i < 0:
                break
            start = i + 6
            arg = body[i + 6 : i + 6 + 120].split(")")[0]
            for p in param_names:
                if p and p != "self" and p in arg:
                    out.append(arg.strip()[:100])
                    break
        return out

    def lock_across_await(self, body):
        """Heuristic: a lock guard bound to a name, then an .await before scope end."""
        flags = []
        for i, line in enumerate(body.split("\n")):
            if any(p in line for p in (".lock()", ".read()", ".write()")) and "let " in line:
                if ".await" not in line:
                    flags.append(i + 1)
        if not flags:
            return False
        return ".await" in body

    # ── output ───────────────────────────────────────────────────────────────

    def write(self, outdir):
        os.makedirs(outdir, exist_ok=True)
        for name, rows in (("functions", self.functions),
                           ("calls", self.calls),
                           ("routes", self.routes)):
            with open(os.path.join(outdir, f"{name}.ndjson"), "w") as fh:
                for r in rows:
                    fh.write(json.dumps(r, separators=(",", ":")) + "\n")

        # A nested `fn` lives inside its parent's body, so the parent's body scan
        # sees the child's unsafe blocks too. For a *site* inventory that is
        # double-counting: attribute each (file, line) to the INNERMOST enclosing
        # function, i.e. the candidate with the greatest line_start.
        site_by_loc = {}
        for f in self.functions:
            for ln in f["unsafe_block_lines"]:
                key = (f["file"], ln)
                prev = site_by_loc.get(key)
                if prev is None or f["line_start"] > prev["_fn_line"]:
                    site_by_loc[key] = {
                        "file": f["file"], "fn": f["fn_name"], "line": ln,
                        "ops": f["unsafe_ops"], "is_unsafe_fn": f["is_unsafe_fn"],
                        "crate": f["crate"], "_fn_line": f["line_start"],
                    }
        unsafe_sites = sorted(
            ({k: v for k, v in s.items() if k != "_fn_line"}
             for s in site_by_loc.values()),
            key=lambda s: (s["file"], s["line"]),
        )
        unsafe_fns = [
            {"file": f["file"], "fn": f["fn_name"], "line": f["line_start"],
             "crate": f["crate"]}
            for f in self.functions if f["is_unsafe_fn"]
        ]

        coverage = {
            "root": self.root,
            "files_total": self.stats["files"],
            "files_parsed": self.stats["files"] - len(self.error_files),
            "files_with_error_nodes": len(self.error_files),
            "error_files": self.error_files,
            "error_nodes_total": self.stats["error_nodes"],
            "ast_function_nodes": self.stats["ast_functions"],
            "emitted_function_records": self.stats["emitted_functions"],
            "function_coverage_pct": round(
                100.0 * self.stats["emitted_functions"]
                / max(1, self.stats["ast_functions"]), 4),
            "call_edges": len(self.calls),
            "routes": len(self.routes),
            "unsafe_block_sites": len(unsafe_sites),
            "unsafe_fn_declarations": len(unsafe_fns),
            "functions_with_unsafe": sum(1 for f in self.functions if f["unsafe_any"]),
            "handler_shaped_functions": sum(
                1 for f in self.functions if f["is_handler_shaped"]),
            "functions_with_narrowing_casts": sum(
                1 for f in self.functions if f["narrowing_casts"]),
            "functions_with_alloc_from_param": sum(
                1 for f in self.functions if f["alloc_from_param"]),
            "per_file": self.per_file,
        }
        with open(os.path.join(outdir, "coverage.json"), "w") as fh:
            json.dump(coverage, fh, indent=2)
        with open(os.path.join(outdir, "unsafe_inventory.json"), "w") as fh:
            json.dump({"unsafe_blocks": unsafe_sites,
                       "unsafe_fns": unsafe_fns}, fh, indent=2)
        return coverage


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("--out", default="./ast-out")
    args = ap.parse_args()

    ex = Extractor(args.root).run()
    cov = ex.write(args.out)
    print(json.dumps({k: v for k, v in cov.items() if k != "per_file"}, indent=2))


if __name__ == "__main__":
    main()
