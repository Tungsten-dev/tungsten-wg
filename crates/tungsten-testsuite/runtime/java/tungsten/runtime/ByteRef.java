package tungsten.runtime;

/** Mutable holder for a `Ref(true)` on a primitive (written back by the generated code). */
public final class ByteRef {
    public byte value;

    public ByteRef(byte value) {
        this.value = value;
    }
}
