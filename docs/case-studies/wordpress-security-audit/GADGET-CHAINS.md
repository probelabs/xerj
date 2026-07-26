# POP-gadget (deserialization) chain hunt

A dangerous `unserialize` is only exploitable if a **gadget chain** exists: a
class whose **magic method** — auto-invoked by unserialize (`__wakeup`,
`__unserialize`), by object destruction (`__destruct`), or by later use
(`__toString`, `__call`) — reaches a sink. This hunt enumerates every magic method
and traces it, interprocedurally, to any dangerous call.

## Method (XERJ data first)

`gadget_hunt.py` scrolls the `wpaudit` call graph, selects the 15 magic-method
names, and DFS-walks each (unambiguous call edges) to a **payoff**: a reached
function whose `sinks` are non-empty or whose `calls` include a high-impact
function (`exec`/`eval`/`unserialize`/`file_put_contents`/`unlink`/
`call_user_func`/`extractTo`/…). Flags the *auto-triggered* magic methods
separately — those are the live deserialization gadgets.

## Result on WordPress core

| | |
|---|--:|
| magic methods in core | 131 |
| **auto-triggered (`__wakeup`/`__unserialize`/`__destruct`/`__toString`) reaching a sink** | **0** |
| any magic method reaching a dangerous call | 2 |

The 2 are `__callStatic` (`pluggable-deprecated.php`, `AbstractEnum.php`), and both
are **false positives** on reading:
```php
public static function __callStatic($name, $arguments) {
    _deprecated_function(__CLASS__.'::'.$name, '3.5.0', '…');   // just a deprecation notice
}
```
The `call_user_func_array` at the end of the traced path
(`__callStatic → _deprecated_function → wp_trigger_error → do_action → _wp_call_all_hook → call_user_func_array`)
is WordPress's **generic hook dispatcher** firing registered callbacks — not
attacker-controlled from the magic method. And `__callStatic` is invoked by an
undefined *static* call, **not** by `unserialize`.

**Impact — a clean positive:** WordPress **core has no deserialization POP-gadget
chain.** Even given arbitrary object injection, no core magic method escalates it
to RCE/file-write/SSRF. The `__wakeup` methods that exist (e.g. SimplePie's
`FilteredIterator`) are inert or *defensive* (they throw to block deserialization).
Live gadget risk in the WP ecosystem comes from **vendored libraries and plugins**,
not core magic methods — point the same hunt at those.

## XERJ vs grep+context (measured)

The question — *"which magic methods reach a dangerous sink?"* — is inherently
**interprocedural**, so grep alone can't answer it; you must read the bodies and
hand-trace callees across files.

| approach | what it costs | tokens |
|---|---|--:|
| grep + read | read all **83 files** that define a magic method **and trace callees across files** | **~562,000** (> a context window → chunk → lose cross-file traces) |
| **XERJ** | query the 15 magic-method names + traverse the pre-built call graph → the answer | **~144** (2 candidates) |

**≈3,900× to the answer.** Quality is *higher*, not just cheaper: XERJ followed
6-hop cross-file chains reliably; a human/grep pass over 562k tokens would chunk
the code and is exactly where an interprocedural gadget chain slips through. The
call graph is built once and reused for every such query (gadgets, taint,
reachability).

## Reproduce
```bash
python3 gadget_hunt.py     # magic methods -> dangerous call, over the XERJ graph
```
Generalize by extending `MAGIC`/`DANGER`; point `wpaudit` at a plugin/vendored lib
to find the gadgets that core doesn't have.
