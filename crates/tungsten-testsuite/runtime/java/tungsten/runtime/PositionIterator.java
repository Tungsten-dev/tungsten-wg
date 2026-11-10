// Vendored from tungsten-frontends/tungsten-diff-harness/java_wrapper/runtime (unchanged).
package tungsten.runtime;

/**
 * Mutable cursor over a kernel's grid, one per kernel invocation (SPEC section 6.2).
 *
 * <p>Rust: tungsten-mc/src/mathf64.rs Iter3D::always_next, which yields
 * {@code pos = Pos3::new(x, y, z)} with the raw grid indices. The kernels apply
 * origin/position scaling themselves, so there's no floating-point work here.
 */
public final class PositionIterator {
    public Pos3 pos;
    public int ix, iy, iz, index;
    private final Config config;

    public PositionIterator(Config config) {
        this.config = config;
    }

    public void set(int ix, int iy, int iz, int index) {
        this.ix = ix;
        this.iy = iy;
        this.iz = iz;
        this.index = index;
        this.pos = new Pos3(ix, iy, iz);
    }
}
