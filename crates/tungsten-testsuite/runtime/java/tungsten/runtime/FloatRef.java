package tungsten.runtime;

/** Mutable holder for a `Ref(true)` on a primitive (written back by the generated code). */
public final class FloatRef {
    public float value;

    public FloatRef(float value) {
        this.value = value;
    }
}
