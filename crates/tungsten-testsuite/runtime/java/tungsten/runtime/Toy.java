package tungsten.runtime;

/** splitmix64 helpers of the toy tables (SPEC 5.3). */
final class Toy {
    private Toy() {}

    static final long GAMMA = 0x9E3779B97F4A7C15L;

    static long splitmix64(long x) {
        long z = x + GAMMA;
        z = (z ^ (z >>> 30)) * 0xBF58476D1CE4E5B9L;
        z = (z ^ (z >>> 27)) * 0x94D049BB133111EBL;
        return z ^ (z >>> 31);
    }

    static long fold(long first, long... rest) {
        long acc = first;
        for (long a : rest) acc = splitmix64(acc ^ a);
        return acc;
    }
}
