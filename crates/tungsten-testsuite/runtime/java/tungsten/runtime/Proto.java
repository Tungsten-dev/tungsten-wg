package tungsten.runtime;

import java.io.*;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;

/** Driver side of the wire protocol (SPEC 7.4) and the request loop; mirrors runtime/rust/proto.rs. */
public final class Proto {
    private Proto() {}

    public static final class Reader {
        private final ByteBuffer b;

        Reader(byte[] buf) {
            b = ByteBuffer.wrap(buf).order(ByteOrder.LITTLE_ENDIAN);
        }

        public boolean bool() { return b.get() != 0; }
        public byte i8() { return b.get(); }
        public int i32() { return b.getInt(); }
        public long i64() { return b.getLong(); }
        public long u64() { return b.getLong(); }
        public float f32() { return Float.intBitsToFloat(b.getInt()); }
        public double f64() { return Double.longBitsToDouble(b.getLong()); }
    }

    public static final class Writer {
        private final ByteArrayOutputStream out = new ByteArrayOutputStream();
        private final ByteBuffer tmp = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN);

        public Writer() { out.write(0); }

        private void flush(int n) {
            out.write(tmp.array(), 0, n);
            tmp.clear();
        }

        public void bool(boolean x) { out.write(x ? 1 : 0); }
        public void i8(byte x) { out.write(x); }
        public void i32(int x) { tmp.putInt(x); flush(4); }
        public void i64(long x) { tmp.putLong(x); flush(8); }
        public void f32(float x) { tmp.putInt(Float.floatToRawIntBits(x)); flush(4); }
        public void f64(double x) { tmp.putLong(Double.doubleToRawLongBits(x)); flush(8); }
        public byte[] finish() { return out.toByteArray(); }
    }

    public interface Runner {
        byte[] run(Reader r);
    }

    static int classify(Throwable t) {
        String msg = String.valueOf(t.getMessage());
        if (t instanceof ArithmeticException) return 0;
        if (t instanceof ArrayIndexOutOfBoundsException || t instanceof IndexOutOfBoundsException) return 1;
        if (msg.contains(Helpers.DOMAIN)) return 2;
        return 3;
    }

    // A failing constant initialiser surfaces as ExceptionInInitializerError once, then as
    // NoClassDefFoundError on every later use (SPEC 7.3). Both report the original failure.
    private static Throwable initFailure;

    static Throwable unwrap(Throwable t) {
        if (t instanceof ExceptionInInitializerError && t.getCause() != null) {
            initFailure = t.getCause();
            return initFailure;
        }
        if (t instanceof NoClassDefFoundError && initFailure != null) return initFailure;
        return t;
    }

    static byte[] failure(Throwable t) {
        t = unwrap(t);
        String msg = t.getClass().getName() + ": " + t.getMessage();
        Throwable c = t.getCause();
        if (c != null) msg += " (cause: " + c + ")";
        byte[] m = msg.getBytes(StandardCharsets.UTF_8);
        ByteBuffer b = ByteBuffer.allocate(6 + m.length).order(ByteOrder.LITTLE_ENDIAN);
        b.put((byte) 1).put((byte) classify(t)).putInt(m.length).put(m);
        return b.array();
    }

    /** Reads requests of {@code requestLen} bytes from stdin until EOF, answering each on stdout. */
    public static void serve(int requestLen, Runner run) {
        DataInputStream in = new DataInputStream(new BufferedInputStream(System.in));
        PrintStream out = new PrintStream(new FileOutputStream(FileDescriptor.out), false);
        System.setOut(System.err);
        try {
            while (true) {
                byte[] buf = new byte[requestLen];
                try {
                    in.readFully(buf);
                } catch (EOFException e) {
                    return;
                }
                byte[] reply;
                try {
                    reply = run.run(new Reader(buf));
                } catch (Throwable t) {
                    reply = failure(t);
                }
                out.write(reply, 0, reply.length);
                out.flush();
            }
        } catch (IOException e) {
            // pipe closed
        }
    }
}
