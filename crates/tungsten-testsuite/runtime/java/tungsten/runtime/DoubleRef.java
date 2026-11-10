package tungsten.runtime;

/** Mutable holder for a `Ref(true)` on a primitive (written back by the generated code). */
public final class DoubleRef {
    public double value;

    public DoubleRef(double value) {
        this.value = value;
    }
}
