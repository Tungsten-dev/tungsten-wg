package tungsten.runtime;

/** Registry externs (registry.toml) and the toy tables, called by generated code. */
public final class Helpers {
    private Helpers() {}

    public static PerlinNoiseSampler make_permutation_table(long seed, long idLo, long idHi, long subIdx, long subLo, long subHi) {
        return new PerlinNoiseSampler(Toy.fold(seed, idLo, idHi, subIdx, subLo, subHi));
    }

    public static InterpolatedNoiseSampler make_base3d_perm_table(long seed) {
        return new InterpolatedNoiseSampler(Toy.splitmix64(seed));
    }

    public static int perm_get(PerlinNoiseSampler t, int i) { return t.lookup(i); }
    public static double sampler_sample(InterpolatedNoiseSampler s, Vec3 v) { return s.sample(v); }

    public static final String DOMAIN = "tw:ExternDomain";
    public static int test_domain(int x) {
        if (x < 0) throw new RuntimeException(DOMAIN);
        return x;
    }

    public static double sqrt(double a) { return Math.sqrt(a); }
    public static double floor(double a) { return Math.floor(a); }
    public static double ceil(double a) { return Math.ceil(a); }
    public static double trunc(double a) { return a < 0 ? Math.ceil(a) : Math.floor(a); }
    public static double abs(double a) { return Math.abs(a); }
    public static double min(double a, double b) { return Math.min(a, b); }
    public static double max(double a, double b) { return Math.max(a, b); }
    public static double copysign(double a, double b) { return Math.copySign(a, b); }
    public static double fma(double a, double b, double c) { return Math.fma(a, b, c); }

    public static float sqrt_f32(float a) { return (float) Math.sqrt((double) a); }
    public static float floor_f32(float a) { return (float) Math.floor((double) a); }
    public static float ceil_f32(float a) { return (float) Math.ceil((double) a); }
    public static float trunc_f32(float a) { return (float) trunc((double) a); }
    public static float abs_f32(float a) { return Math.abs(a); }
    public static float min_f32(float a, float b) { return Math.min(a, b); }
    public static float max_f32(float a, float b) { return Math.max(a, b); }
    public static float copysign_f32(float a, float b) { return Math.copySign(a, b); }
    public static float fma_f32(float a, float b, float c) { return Math.fma(a, b, c); }

    public static int float_to_bits(float a) { return Float.floatToIntBits(a); }
    public static long double_to_bits(double a) { return Double.doubleToLongBits(a); }
    public static float bits_to_float(int a) { return Float.intBitsToFloat(a); }
    public static double bits_to_double(long a) { return Double.longBitsToDouble(a); }
}
