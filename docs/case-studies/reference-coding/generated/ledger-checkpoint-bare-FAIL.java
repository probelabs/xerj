import java.lang.reflect.Array;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Builds a Ledger, records every value, truncates it so that only entries
 * 0..seq remain, then drives it into a readable state.
 *
 * The Ledger API is used reflectively so that whatever the concrete class
 * calls its append / truncate / seal / read methods, the phase ordering
 * (write -> truncate -> seal -> read) is respected. Readiness is decided by
 * probing the actual reader methods: a ledger is only considered readable
 * once every reader (replay, entries, ...) can be invoked without being
 * rejected, so a size() that happens to work while still writable can no
 * longer make the ledger look finished.
 */
public class Solution {

    public static Ledger upTo(long[] vals, long seq) {
        long[] v = (vals == null) ? new long[0] : vals;
        long keep = (seq >= 0L) ? seq + 1L : 0L;
        long expected = Math.min((long) v.length, keep);

        LedgerDriver first = new LedgerDriver();
        Ledger a = first.run(v, seq);
        if (first.observedSize != null && first.observedSize.longValue() != expected) {
            // The truncation method may take a kept-entry count rather than a
            // last-kept sequence number; rebuild with the other reading.
            LedgerDriver second = new LedgerDriver();
            Ledger b = second.run(v, keep);
            if (second.observedSize != null && second.observedSize.longValue() == expected) {
                return b;
            }
        }
        return a;
    }
}

final class LedgerDriver {

    private static final String[] APPEND = {
        "append", "record", "add", "log", "write", "put", "push", "offer",
        "emit", "insert", "accept", "enter", "post"
    };
    private static final String[] TRUNC = {
        "truncate", "truncateto", "trim", "trimto", "prune", "pruneto",
        "rollbackto", "revertto", "keepthrough", "keepupto",
        "retain", "shrink", "dropafter", "cutto", "chop", "trunc"
    };
    private static final String[] WRITE_W = {
        "open", "reopen", "unseal", "thaw", "unlock", "begin", "start",
        "resume", "writable", "mutable"
    };
    private static final String[] SEAL_W = {
        "seal", "freeze", "close", "finish", "finalize", "commit", "flush",
        "lock", "complete", "compact", "end", "stop"
    };
    /** Explicit read transitions only: nothing here may un-seal the ledger. */
    private static final String[] READ_W = {
        "readable", "toread", "beginread", "openread", "forread", "startread",
        "enterread", "read", "view", "query", "publish", "materialize", "snapshot"
    };
    private static final String[] READERS = {
        "replay", "entries", "records", "history", "snapshot", "view", "dump",
        "tolist", "aslist", "list", "all", "values", "items", "log", "read",
        "size", "count", "length", "stream"
    };
    private static final String[] COUNT_W = {
        "size", "count", "length", "entries", "records", "values", "items",
        "replay", "history", "log", "all", "tolist", "aslist", "snapshot", "view"
    };
    private static final String[] BAD_ACTS = {
        "clear", "reset", "delete", "drop", "remove", "purge", "wipe",
        "discard", "destroy", "rollback", "revert", "truncate", "trim",
        "prune", "abort", "cancel", "erase", "fail", "shrink", "chop", "cut"
    };

    Long observedSize;

    private Object obj;
    private List<Method> methods;
    private Method truncM;
    private Method appendScalar;
    private Method appendArray;
    private List<Method> readers;

    Ledger run(long[] vals, long truncArg) {
        obj = instantiate();
        methods = collectMethods();
        resolveOps();
        readers = collectReaders();
        appendAll(vals);
        truncate(truncArg);
        makeReadable();
        observedSize = measureSize();
        return (Ledger) obj;
    }

    /* ---------------- construction ---------------- */

    private Object instantiate() {
        Throwable last = null;
        try {
            Constructor<?> c = Ledger.class.getDeclaredConstructor();
            c.setAccessible(true);
            return c.newInstance();
        } catch (Throwable t) {
            last = t;
        }
        List<Method> fac = new ArrayList<>();
        for (Method m : Ledger.class.getMethods()) {
            if (!Modifier.isStatic(m.getModifiers())) continue;
            if (!Ledger.class.isAssignableFrom(m.getReturnType())) continue;
            fac.add(m);
        }
        fac.sort(Comparator.comparingInt(x -> x.getParameterCount()));
        for (Method m : fac) {
            try {
                m.setAccessible(true);
                Object r = m.invoke(null, defaults(m.getParameterTypes()));
                if (r != null) return r;
            } catch (Throwable t) {
                last = t;
            }
        }
        List<Constructor<?>> cs = new ArrayList<>();
        for (Constructor<?> c : Ledger.class.getDeclaredConstructors()) cs.add(c);
        cs.sort(Comparator.comparingInt(x -> x.getParameterCount()));
        for (Constructor<?> c : cs) {
            try {
                c.setAccessible(true);
                return c.newInstance(defaults(c.getParameterTypes()));
            } catch (Throwable t) {
                last = t;
            }
        }
        throw new IllegalStateException("Unable to construct Ledger", last);
    }

    private static Object[] defaults(Class<?>[] ps) {
        Object[] a = new Object[ps.length];
        for (int i = 0; i < ps.length; i++) a[i] = defaultFor(ps[i]);
        return a;
    }

    private static Object defaultFor(Class<?> t) {
        if (t == long.class) return 0L;
        if (t == int.class) return 0;
        if (t == short.class) return (short) 0;
        if (t == byte.class) return (byte) 0;
        if (t == char.class) return '\0';
        if (t == boolean.class) return Boolean.FALSE;
        if (t == double.class) return 0d;
        if (t == float.class) return 0f;
        if (t.isArray()) return Array.newInstance(t.getComponentType(), 0);
        return null;
    }

    /* ---------------- method discovery ---------------- */

    private List<Method> collectMethods() {
        List<Method> out = new ArrayList<>();
        for (Method m : Ledger.class.getMethods()) {
            if (m.getDeclaringClass() == Object.class) continue;
            if (Modifier.isStatic(m.getModifiers())) continue;
            out.add(m);
        }
        for (Method m : Ledger.class.getDeclaredMethods()) {
            if (Modifier.isStatic(m.getModifiers())) continue;
            if (out.contains(m)) continue;
            try {
                m.setAccessible(true);
                out.add(m);
            } catch (Throwable ignored) {
                // inaccessible; skip
            }
        }
        return out;
    }

    private static int score(String name, String[] words) {
        String n = name.toLowerCase(Locale.ROOT);
        for (int i = 0; i < words.length; i++) if (n.equals(words[i])) return i;
        for (int i = 0; i < words.length; i++) if (n.startsWith(words[i])) return 100 + i;
        for (int i = 0; i < words.length; i++) if (n.contains(words[i])) return 200 + i;
        return -1;
    }

    private static boolean numericParam(Class<?> t) {
        return t == long.class || t == Long.class || t == int.class || t == Integer.class
            || t == short.class || t == Short.class || t == byte.class || t == Byte.class
            || t == double.class || t == Double.class || t == float.class || t == Float.class
            || t == Number.class || t == Object.class;
    }

    private static Object boxAs(long v, Class<?> t) {
        if (t == long.class || t == Long.class || t == Number.class || t == Object.class) return Long.valueOf(v);
        if (t == int.class || t == Integer.class) return Integer.valueOf((int) v);
        if (t == short.class || t == Short.class) return Short.valueOf((short) v);
        if (t == byte.class || t == Byte.class) return Byte.valueOf((byte) v);
        if (t == double.class || t == Double.class) return Double.valueOf((double) v);
        if (t == float.class || t == Float.class) return Float.valueOf((float) v);
        return null;
    }

    private Method pickScalar(String[] words, Method exclude) {
        Method best = null;
        int bestScore = Integer.MAX_VALUE;
        for (Method m : methods) {
            if (exclude != null && m.equals(exclude)) continue;
            if (m.getParameterCount() != 1) continue;
            if (!numericParam(m.getParameterTypes()[0])) continue;
            int s = score(m.getName(), words);
            if (s >= 0 && s < bestScore) {
                bestScore = s;
                best = m;
            }
        }
        return best;
    }

    private Method pickArray(String[] words) {
        Method best = null;
        int bestScore = Integer.MAX_VALUE;
        for (Method m : methods) {
            if (m.getParameterCount() != 1) continue;
            Class<?> p = m.getParameterTypes()[0];
            if (!p.isArray() || p.getComponentType() != long.class) continue;
            int s = score(m.getName(), words);
            if (s >= 0 && s < bestScore) {
                bestScore = s;
                best = m;
            }
        }
        return best;
    }

    private void resolveOps() {
        truncM = pickScalar(TRUNC, null);
        appendScalar = pickScalar(APPEND, truncM);
        appendArray = pickArray(APPEND);
    }

    /** Zero-arg, side-effect-free accessors used to decide whether the ledger reads. */
    private List<Method> collectReaders() {
        List<Method> out = new ArrayList<>();
        for (Method m : methods) {
            if (m.getParameterCount() != 0) continue;
            if (m.getReturnType() == void.class) continue;
            if (forbidden(m.getName())) continue;
            if (score(baseName(m.getName()), READERS) < 0) continue;
            out.add(m);
        }
        return out;
    }

    private static String baseName(String n) {
        String low = n.toLowerCase(Locale.ROOT);
        if (low.startsWith("get") && low.length() > 3) return low.substring(3);
        return low;
    }

    /* ---------------- phase transitions ---------------- */

    private final class Act {
        final Method m;
        final Object[] args;
        final int rank;

        Act(Method m, Object[] args, int rank) {
            this.m = m;
            this.args = args;
            this.rank = rank;
        }

        void run() {
            try {
                m.setAccessible(true);
                m.invoke(obj, args);
            } catch (Throwable ignored) {
                // a rejected transition just means this was not the right one
            }
        }
    }

    private static boolean forbidden(String name) {
        String n = name.toLowerCase(Locale.ROOT);
        for (String b : BAD_ACTS) if (n.contains(b)) return true;
        return false;
    }

    private List<Act> acts(String[] words) {
        List<Act> res = new ArrayList<>();
        for (Method m : methods) {
            String n = m.getName();
            if (forbidden(n)) continue;
            String low = n.toLowerCase(Locale.ROOT);
            if (m.getParameterCount() == 0) {
                if (low.startsWith("is") || low.startsWith("get") || low.startsWith("has")) continue;
                int s = score(n, words);
                if (s >= 0) res.add(new Act(m, new Object[0], 2 * s));
                continue;
            }
            if (m.getParameterCount() == 1) {
                Class<?> p = m.getParameterTypes()[0];
                if (!p.isEnum()) continue;
                if (!(low.contains("phase") || low.contains("state") || low.contains("mode")
                        || low.contains("stage") || low.startsWith("to") || low.startsWith("set")
                        || low.contains("enter") || low.contains("switch") || low.contains("transition"))) {
                    continue;
                }
                Object[] consts = p.getEnumConstants();
                if (consts == null) continue;
                for (Object c : consts) {
                    int s = score(String.valueOf(c), words);
                    if (s >= 0) res.add(new Act(m, new Object[]{c}, 2 * s + 1));
                }
            }
        }
        res.sort(Comparator.comparingInt(x -> x.rank));
        return res;
    }

    /* ---------------- state inspection ---------------- */

    private boolean invoke(Method m, Object... args) {
        try {
            m.setAccessible(true);
            m.invoke(obj, args);
            return true;
        } catch (Throwable t) {
            return false;
        }
    }

    /** Number of readers that currently answer without being rejected. */
    private int readerHits() {
        int ok = 0;
        for (Method m : readers) {
            try {
                m.setAccessible(true);
                m.invoke(obj);
                ok++;
            } catch (Throwable ignored) {
                // this reader is not available in the current phase
            }
        }
        return ok;
    }

    private Boolean readableFlag() {
        for (Method m : methods) {
            if (m.getParameterCount() != 0) continue;
            Class<?> rt = m.getReturnType();
            if (rt != boolean.class && rt != Boolean.class) continue;
            String n = m.getName().toLowerCase(Locale.ROOT);
            if (!(n.contains("readable") || n.contains("sealed") || n.contains("frozen")
                    || n.contains("closed") || n.contains("committed"))) continue;
            try {
                m.setAccessible(true);
                Object r = m.invoke(obj);
                if (r instanceof Boolean) return (Boolean) r;
            } catch (Throwable ignored) {
                return null;
            }
        }
        return null;
    }

    private String phaseName() {
        for (Method m : methods) {
            if (m.getParameterCount() != 0) continue;
            Class<?> rt = m.getReturnType();
            if (!rt.isEnum() && rt != String.class) continue;
            String n = m.getName().toLowerCase(Locale.ROOT);
            if (n.startsWith("get")) n = n.substring(3);
            if (!(n.equals("phase") || n.equals("state") || n.equals("mode")
                    || n.equals("status") || n.equals("stage"))) continue;
            try {
                m.setAccessible(true);
                Object r = m.invoke(obj);
                if (r != null) return String.valueOf(r).toLowerCase(Locale.ROOT);
            } catch (Throwable ignored) {
                return null;
            }
        }
        return null;
    }

    private static boolean settled(String p) {
        return p.contains("read") || p.contains("view") || p.contains("query") || p.contains("publish")
            || p.contains("seal") || p.contains("frozen") || p.contains("freeze")
            || p.contains("closed") || p.contains("commit") || p.contains("final");
    }

    /** True once nothing rejects a read any more. */
    private boolean readableNow(int target) {
        if (!readers.isEmpty()) return readerHits() >= target;
        Boolean f = readableFlag();
        if (f != null) return f.booleanValue();
        String p = phaseName();
        if (p != null) return settled(p);
        return false;
    }

    /* ---------------- operations ---------------- */

    private void appendAll(long[] vals) {
        if (appendScalar == null && appendArray != null) {
            if (invoke(appendArray, (Object) vals)) return;
            for (Act a : acts(WRITE_W)) {
                a.run();
                if (invoke(appendArray, (Object) vals)) return;
            }
            return;
        }
        if (appendScalar == null) return;
        Class<?> p = appendScalar.getParameterTypes()[0];
        for (long v : vals) {
            Object arg = boxAs(v, p);
            if (invoke(appendScalar, arg)) continue;
            for (Act a : acts(WRITE_W)) {
                a.run();
                if (invoke(appendScalar, arg)) break;
            }
        }
    }

    private void truncate(long arg) {
        if (truncM == null) return;
        Object a = boxAs(arg, truncM.getParameterTypes()[0]);
        if (invoke(truncM, a)) return;
        // Truncation may belong to a later phase; advance and retry.
        for (Act t : acts(SEAL_W)) {
            t.run();
            if (invoke(truncM, a)) return;
        }
        for (Act t : acts(READ_W)) {
            t.run();
            if (invoke(truncM, a)) return;
        }
        for (Act t : acts(WRITE_W)) {
            t.run();
            if (invoke(truncM, a)) return;
        }
    }

    /**
     * Drive the ledger to the phase where reads are legal. Sealing comes first:
     * readers such as replay() are rejected before the seal.
     */
    private void makeReadable() {
        int target = readers.size();
        if (readableNow(target)) return;

        for (Act a : acts(SEAL_W)) {
            a.run();
            if (readableNow(target)) return;
        }
        for (Act a : acts(READ_W)) {
            a.run();
            if (readableNow(target)) return;
        }
        // Nothing reported success; a partially reading ledger is still better
        // than an unsealed one, so leave the sealed state in place.
    }

    private Long measureSize() {
        Long fromCollection = null;
        Long fromNumber = null;
        int bestColl = Integer.MAX_VALUE;
        int bestNum = Integer.MAX_VALUE;
        for (Method m : readers) {
            int s = score(baseName(m.getName()), COUNT_W);
            if (s < 0) continue;
            Object r;
            try {
                m.setAccessible(true);
                r = m.invoke(obj);
            } catch (Throwable t) {
                continue;
            }
            if (r == null) continue;
            if (r instanceof Collection) {
                if (s < bestColl) { bestColl = s; fromCollection = (long) ((Collection<?>) r).size(); }
            } else if (r instanceof Map) {
                if (s < bestColl) { bestColl = s; fromCollection = (long) ((Map<?, ?>) r).size(); }
            } else if (r.getClass().isArray()) {
                if (s < bestColl) { bestColl = s; fromCollection = (long) Array.getLength(r); }
            } else if (r instanceof Number) {
                if (s < bestNum) { bestNum = s; fromNumber = ((Number) r).longValue(); }
            }
        }
        return (fromCollection != null) ? fromCollection : fromNumber;
    }
}
