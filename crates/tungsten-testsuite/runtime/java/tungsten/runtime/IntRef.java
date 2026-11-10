package tungsten.runtime;

/** Mutable holder for a `Ref(true)` on a primitive (written back by the generated code). */
public final class IntRef {
    public int value;

    public IntRef(int value) {
        this.value = value;
    }
}
