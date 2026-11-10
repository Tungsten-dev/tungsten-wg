package tungsten.runtime;

/** Toy sampler (crate README, "Rules and contracts, Toy tables"); mirrors runtime/rust/utilsf64.rs. */
public final class InterpolatedNoiseSampler {
    public final long state;

    public InterpolatedNoiseSampler(long state) {
        this.state = state;
    }

    private static long f2i(double x) {
        return ((long) (int) x) & 0xffffffffL;
    }

    public double sample(Vec3 v) {
        long h = Toy.fold(state, f2i(Math.floor(v.x)), f2i(Math.floor(v.y)), f2i(Math.floor(v.z)));
        return (double) (h >>> 11) * (1.0 / 9007199254740992.0);
    }
}
