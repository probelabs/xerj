public class Solution {
    public static Ledger upTo(long[] vals, long seq) {
        Ledger l = Ledger.open();
        for (long v : vals) {
            l.append(v);
        }
        l.checkpoint(seq);
        l.seal();
        return l;
    }
}
