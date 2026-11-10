// Vendored from tungsten-frontends/tungsten-diff-harness/java_wrapper/runtime (unchanged).
package tungsten.runtime;

/**
 * Immutable integer grid position. Rust: tungsten-mc/src/mathf64.rs Pos3, which holds
 * {@code i32} components (SPEC section 6.1 says "same shape as Vec3", but the Rust reference is
 * integer-valued). Integer arithmetic wraps, like Rust in release builds.
 */
public final class Pos3 {
    public final int x, y, z;

    public Pos3(int x, int y, int z) {
        this.x = x;
        this.y = y;
        this.z = z;
    }

    // Rust: impl Mul<Vec3> for Pos3
    public Vec3 mul(Vec3 o) { return new Vec3((double) x * o.x, (double) y * o.y, (double) z * o.z); }

    // Rust: impl Mul/Add/Sub for Pos3, Mul<i32>, Add<i32>
    public Pos3 mul(Pos3 o) { return new Pos3(x * o.x, y * o.y, z * o.z); }
    public Pos3 mul(int o) { return new Pos3(x * o, y * o, z * o); }
    public Pos3 add(Pos3 o) { return new Pos3(x + o.x, y + o.y, z + o.z); }
    public Pos3 add(int o) { return new Pos3(x + o, y + o, z + o); }
    public Pos3 sub(Pos3 o) { return new Pos3(x - o.x, y - o.y, z - o.z); }

    // Mixed Pos3/Vec3 -> Vec3 (SPEC section 8.5). Rust has no such impls; these mirror the
    // Vec3-on-the-left versions with the int component converted first.
    public Vec3 add(Vec3 o) { return new Vec3((double) x + o.x, (double) y + o.y, (double) z + o.z); }
    public Vec3 sub(Vec3 o) { return new Vec3((double) x - o.x, (double) y - o.y, (double) z - o.z); }

    @Override
    public boolean equals(Object o) {
        return o instanceof Pos3 p && x == p.x && y == p.y && z == p.z;
    }

    @Override
    public int hashCode() {
        return (31 * x + y) * 31 + z;
    }

    @Override
    public String toString() {
        return "Pos3(" + x + ", " + y + ", " + z + ")";
    }
}
