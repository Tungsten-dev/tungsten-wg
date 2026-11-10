package tungsten.runtime;

/** Mutable holder for a `Ref(true)` on a primitive (written back by the generated code). */
public final class LongRef {
    public long value;

    public LongRef(long value) {
        this.value = value;
    }
}
