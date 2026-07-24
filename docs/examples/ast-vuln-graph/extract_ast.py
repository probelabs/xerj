#!/usr/bin/env python3
"""AST -> taint graph for PHP/WordPress. Emits one 'function fact' per function
(for XERJ) and the interprocedural source->sink findings a per-file scan misses.
"""
import glob, json, os, sys
from phply import phplex, phpast as ast
from phply.phpparse import make_parser

SRC   = {"$_GET","$_POST","$_REQUEST","$_COOKIE","$_SERVER","$_FILES"}
SQL   = {"query","get_results","get_var","get_row","get_col"}
DANGER= {"unserialize","eval","exec","system","shell_exec","passthru","assert","include","require"}
STATE = {"update_option","delete_option","add_option","wp_insert_post",
         "wp_update_post","wp_delete_post","update_user_meta","add_user_meta"}
SANIT = {"esc_html","esc_attr","esc_url","sanitize_text_field","sanitize_key","absint",
         "intval","wp_verify_nonce","check_admin_referer","wp_kses","prepare"}

def walk(n):
    if isinstance(n, ast.Node):
        yield n
        for f in n.fields:
            yield from walk(getattr(n, f))
    elif isinstance(n, (list, tuple)):
        for x in n: yield from walk(x)

def analyse_body(nodes):
    src, sinks, san, calls = set(), [], set(), set()
    for n in nodes:
        for m in walk(n):
            t = type(m).__name__
            if t == "Variable" and m.name in SRC: src.add(m.name)
            elif t == "MethodCall" and m.name in SQL: sinks.append(("sql", m.name))
            elif t == "Echo": sinks.append(("xss", "echo"))
            elif t == "Print": sinks.append(("xss", "print"))
            elif t == "Include": sinks.append(("lfi", "include"))
            elif t == "FunctionCall":
                if m.name in DANGER: sinks.append((m.name, m.name))
                if m.name in STATE:  sinks.append(("state", m.name))
                if m.name in SANIT:  san.add(m.name)
                calls.add(m.name)        # candidate call edge (user fn resolved later)
            elif t == "MethodCall" and m.name in SANIT: san.add(m.name)
    return src, sinks, san, calls

def main(root):
    parser = make_parser()
    funcs = {}          # name -> fact
    hooks = {}          # function_name -> {'entry':True,'nopriv':bool}
    for path in glob.glob(f"{root}/**/*.php", recursive=True):
        try:
            tree = parser.parse(open(path, encoding="utf-8", errors="replace").read(),
                                lexer=phplex.lexer.clone())
        except Exception:
            continue
        rel = os.path.relpath(path, root)
        for n in walk(tree):
            t = type(n).__name__
            if t == "Function":
                src, sinks, san, calls = analyse_body(n.nodes)
                funcs[n.name] = {
                    "file": rel, "func": n.name, "line": n.lineno,
                    "sources": sorted(src),
                    "sinks": sorted({s[0] for s in sinks}),
                    "sink_calls": sorted({s[1] for s in sinks}),
                    "sanitizers": sorted(san),
                    "calls": sorted(calls),
                    "params": [p.name for p in n.params],
                }
            elif t == "FunctionCall" and n.name in ("add_action","add_filter"):
                # add_action('hook','callback')
                args=[p.node for p in n.params if isinstance(p, ast.Parameter)]
                if len(args)>=2 and isinstance(args[0],str) and isinstance(args[1],str):
                    hooks[args[1]] = {"entry": True,
                                      "nopriv": "nopriv" in args[0],
                                      "admin": args[0].startswith(("admin_post","wp_ajax"))}
    # resolve call edges to user functions only
    for f in funcs.values():
        f["calls"] = [c for c in f["calls"] if c in funcs]
        f["entry"] = bool(hooks.get(f["func"], {}).get("entry"))
        f["nopriv"] = bool(hooks.get(f["func"], {}).get("nopriv"))

    # ── interprocedural taint: source -> (calls*) -> sink, no sanitizer on path ──
    findings = []
    def dfs(start):
        stack=[(start,[start],set(funcs[start]["sanitizers"]))]
        seen=set()
        while stack:
            cur,path,san=stack.pop()
            if cur in seen: continue
            seen.add(cur)
            f=funcs[cur]
            if f["sinks"] and not san:
                findings.append({"sink_type":f["sinks"], "sink_func":cur,
                                 "sink_file":f["file"], "path":path,
                                 "source_func":path[0],
                                 "unauth": funcs[path[0]].get("nopriv", False)})
            for cal in f["calls"]:
                stack.append((cal, path+[cal], san|set(funcs[cal]["sanitizers"])))
    for name,f in funcs.items():
        if f["sources"]:               # a taint source starts a path
            dfs(name)
    # ── CSRF: entry handler performs a state change with no nonce check ──
    def reachable(start):
        seen=set(); stack=[start]
        while stack:
            c=stack.pop()
            if c in seen or c not in funcs: continue
            seen.add(c); stack+=funcs[c]["calls"]
        return seen
    for name,f in funcs.items():
        if not f.get("entry"): continue
        sub=reachable(name)
        does_state = any("state" in funcs[c]["sinks"] for c in sub)
        has_nonce  = any(("wp_verify_nonce" in funcs[c]["sanitizers"]
                          or "check_admin_referer" in funcs[c]["sanitizers"]) for c in sub)
        if does_state and not has_nonce:
            findings.append({"sink_type":["csrf"], "sink_func":name, "sink_file":f["file"],
                             "path":[name], "source_func":name, "unauth":f.get("nopriv",False)})
    # dedupe findings by (source,sink)
    uniq={}
    for x in findings:
        uniq[(x["source_func"],x["sink_func"],tuple(x["sink_type"]))]=x
    findings=list(uniq.values())

    json.dump({"functions":list(funcs.values()),"findings":findings},
              open("ast_facts.json","w"), indent=2)
    print(f"parsed {len(funcs)} functions; {len(findings)} taint findings")
    return funcs, findings

if __name__=="__main__":
    funcs,findings=main(sys.argv[1] if len(sys.argv)>1 else "acme-plugin")
    print("\nINTERPROCEDURAL TAINT FINDINGS (AST graph, no LLM):")
    for x in findings:
        chain=" -> ".join(x["path"])
        tag=" [UNAUTH]" if x["unauth"] else ""
        print(f"  {'/'.join(x['sink_type'])}: {chain}  (sink in {x['sink_file']}){tag}")
