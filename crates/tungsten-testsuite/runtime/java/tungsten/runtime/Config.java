// Vendored from tungsten-frontends/tungsten-diff-harness/java_wrapper/runtime (unchanged).
package tungsten.runtime;

/** Per-kernel iteration setup (SPEC section 6.2). Rust: tungsten-mc/src/utilsf64.rs Config. */
public final class Config {
    public final Vec3 origin, origin_scale, position_scale;
    public final int dx, dy, dz;

    public Config(Vec3 origin, Vec3 origin_scale, Vec3 position_scale, int dx, int dy, int dz) {
        this.origin = origin;
        this.origin_scale = origin_scale;
        this.position_scale = position_scale;
        this.dx = dx;
        this.dy = dy;
        this.dz = dz;
    }
}
