package tungsten.runtime;

/** Mutable holder for a `Ref(true)` on a primitive (written back by the generated code). */
public final class BoolRef {
    public boolean value;

    public BoolRef(boolean value) {
        this.value = value;
    }
}
