// Driver side of the wire protocol (crate README, "Protocol") for generated CUDA drivers. Host only.
#pragma once

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <unistd.h>
#include <vector>

namespace tw::proto {

struct Reader {
    const unsigned char* p;
    template <class T>
    T take() {
        T v;
        std::memcpy(&v, p, sizeof(T));
        p += sizeof(T);
        return v;
    }
    bool b() { return take<unsigned char>() != 0; }
    signed char i8() { return take<signed char>(); }
    int i32() { return take<int>(); }
    unsigned u32() { return take<unsigned>(); }
    long long i64() { return take<long long>(); }
    unsigned long long u64() { return take<unsigned long long>(); }
    float f32() { return take<float>(); }
    double f64() { return take<double>(); }
};

/// Starts with the `0x00` tag of an `Ok` response.
struct Writer {
    std::vector<unsigned char> buf{0};
    template <class T>
    void put(T v) {
        unsigned char b[sizeof(T)];
        std::memcpy(b, &v, sizeof(T));
        buf.insert(buf.end(), b, b + sizeof(T));
    }
    void b(bool x) { put<unsigned char>(x ? 1 : 0); }
    void i8(signed char x) { put(x); }
    void i32(int x) { put(x); }
    void i64(long long x) { put(x); }
    void f32(float x) { put(x); }
    void f64(double x) { put(x); }
};

inline bool write_all(const unsigned char* d, size_t n) {
    return std::fwrite(d, 1, n, stdout) == n && std::fflush(stdout) == 0;
}

/// Reads requests from stdin until EOF and answers each on stdout. A request is `request_len`
/// bytes, which end with the `steps` count, followed by that many steps of `step_len` bytes. `run`
/// returns false with a message on a CUDA error. Such an error is sticky, so the process answers
/// with a failure and exits; the harness resumes in a new process (crate README, "Processes").
inline int serve(size_t request_len, size_t step_len, bool (*run)(Reader&, Writer&, std::string&)) {
    std::vector<unsigned char> req(request_len ? request_len : 1);
    for (;;) {
        size_t got = request_len ? std::fread(req.data(), 1, request_len, stdin) : 0;
        if (got != request_len) return 0;
        unsigned steps = 0;
        if (request_len >= 4) std::memcpy(&steps, req.data() + request_len - 4, 4);
        const size_t tail = (size_t)steps * step_len;
        req.resize(request_len + tail + 1);
        if (tail && std::fread(req.data() + request_len, 1, tail, stdin) != tail) return 0;
        Reader r{req.data()};
        Writer w;
        std::string err;
        if (run(r, w, err)) {
            if (!write_all(w.buf.data(), w.buf.size())) return 0;
        } else {
            std::vector<unsigned char> f{1, 3};
            const unsigned len = (unsigned)err.size();
            const unsigned char* l = reinterpret_cast<const unsigned char*>(&len);
            f.insert(f.end(), l, l + 4);
            f.insert(f.end(), err.begin(), err.end());
            write_all(f.data(), f.size());
            _exit(3);
        }
        req.resize(request_len ? request_len : 1);
    }
}

}  // namespace tw::proto
