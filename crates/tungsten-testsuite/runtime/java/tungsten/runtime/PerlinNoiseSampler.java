package tungsten.runtime;

/** Toy permutation table (SPEC 5.3); mirrors runtime/rust/utilsf64.rs. */
public final class PerlinNoiseSampler {
    public final byte[] perm;

    public PerlinNoiseSampler(long state) {
        perm = new byte[256];
        for (int i = 0; i < 256; i++) perm[i] = (byte) i;
        long s = state;
        for (int i = 255; i >= 1; i--) {
            s += Toy.GAMMA;
            long r = Toy.splitmix64(s - Toy.GAMMA);
            int j = (int) Long.remainderUnsigned(r, i + 1L);
            byte t = perm[i];
            perm[i] = perm[j];
            perm[j] = t;
        }
    }

    public int lookup(int i) {
        return perm[i & 255] & 0xff;
    }
}
