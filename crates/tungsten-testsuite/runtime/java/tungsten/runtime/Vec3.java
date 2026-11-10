// Vendored from tungsten-frontends/tungsten-diff-harness/java_wrapper/runtime (unchanged).
package tungsten.runtime;

/**
 * Immutable 3-vector of doubles (SPEC section 6.1). Rust: tungsten-mc/src/mathf64.rs Vec3.
 *
 * <p>{@link #equals} follows Rust's {@code PartialEq} on {@code f64}: components compare with
 * {@code ==}, so {@code NaN != NaN} and {@code 0.0 == -0.0}. That breaks the {@code equals}
 * contract for NaN, which is fine because Vec3 is never used in hash collections.
 */
public final class Vec3 {
    public final double x, y, z;

    public Vec3(double x, double y, double z) {
        this.x = x;
        this.y = y;
        this.z = z;
    }

    public double get(int i) {
        return switch (i) {
            case 0 -> x;
            case 1 -> y;
            case 2 -> z;
            default -> throw new IndexOutOfBoundsException("Vec3 index " + i);
        };
    }

    // Rust: impl Add/Sub/Mul/Div for Vec3
    public Vec3 add(Vec3 o) { return new Vec3(x + o.x, y + o.y, z + o.z); }
    public Vec3 sub(Vec3 o) { return new Vec3(x - o.x, y - o.y, z - o.z); }
    public Vec3 mul(Vec3 o) { return new Vec3(x * o.x, y * o.y, z * o.z); }
    public Vec3 div(Vec3 o) { return new Vec3(x / o.x, y / o.y, z / o.z); }

    // Rust: impl Mul<f64>/Div<f64>/Mul<i32> for Vec3
    public Vec3 mul(double o) { return new Vec3(x * o, y * o, z * o); }
    public Vec3 div(double o) { return new Vec3(x / o, y / o, z / o); }
    public Vec3 mul(int o) { return new Vec3(x * (double) o, y * (double) o, z * (double) o); }

    // Rust: impl Add<Pos3>/Sub<Pos3>/Mul<Pos3> for Vec3
    public Vec3 add(Pos3 o) { return new Vec3(x + (double) o.x, y + (double) o.y, z + (double) o.z); }
    public Vec3 sub(Pos3 o) { return new Vec3(x - (double) o.x, y - (double) o.y, z - (double) o.z); }
    public Vec3 mul(Pos3 o) { return new Vec3(x * (double) o.x, y * (double) o.y, z * (double) o.z); }

    // Rust: Vec3::dot / fract / floor
    public double dot(Vec3 o) { return x * o.x + y * o.y + z * o.z; }
    public Vec3 fract() { return new Vec3(fract(x), fract(y), fract(z)); }
    public Vec3 floor() { return new Vec3(Math.floor(x), Math.floor(y), Math.floor(z)); }

    /** Rust's {@code f64::fract}: {@code self - self.trunc()}. */
    private static double fract(double v) {
        double trunc = v < 0.0 ? Math.ceil(v) : Math.floor(v);
        return v - trunc;
    }

    @Override
    public boolean equals(Object o) {
        return o instanceof Vec3 v && x == v.x && y == v.y && z == v.z;
    }

    @Override
    public int hashCode() {
        // + 0.0 folds -0.0 into 0.0, so values that compare equal hash equally.
        int h = Double.hashCode(x + 0.0);
        h = 31 * h + Double.hashCode(y + 0.0);
        return 31 * h + Double.hashCode(z + 0.0);
    }

    @Override
    public String toString() {
        return "Vec3(" + x + ", " + y + ", " + z + ")";
    }
}
