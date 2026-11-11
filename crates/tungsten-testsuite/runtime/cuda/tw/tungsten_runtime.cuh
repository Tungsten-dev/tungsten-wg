// Test runtime for generated CUDA code (crate README, "Things worth knowing"): vector operators, `tw::Array`, `Config`,
// `PositionIterator`, saturating casts, the toy tables (crate README, "Rules and contracts, Toy tables") and the records generated from
// `registry.toml` (`records.cuh`). The registry externs are in `profile_exact.cuh`.
//
// Everything is `__host__ __device__` and header-only, so the host driver can compute goldens with
// the same code the kernels run. Semantics follow `runtime/rust/`, not the vendored upstream.
#pragma once

#include <cstdio>
#include <cstdlib>

#include <cuda_runtime.h>

#define TW_HD __host__ __device__ constexpr
#define TW_HDI __host__ __device__ inline

#define TW_NAN_F64 (__builtin_nan(""))
#define TW_INF_F64 (__builtin_inf())
#define TW_NAN_F32 (__builtin_nanf(""))
#define TW_INF_F32 (__builtin_inff())

// Core layout and vector semantics follow ../cuda-density/src/tw/tungsten_runtime.cuh (the
// production runtime); the samplers, externs and records here are the registry's toy versions.
//
// ---- vector operators ----------------------------------------------------------------------
// `double3` and `int3` are the device types of `Vec3` and `Pos3`. Mixed operands give `double3`;
// integer vectors wrap through unsigned arithmetic.

namespace tw {
namespace detail {
TW_HD double3 d3(double3 v) { return v; }
TW_HD double3 d3(int3 v) { return double3{(double)v.x, (double)v.y, (double)v.z}; }
TW_HD double3 d3(double s) { return double3{s, s, s}; }
TW_HD double3 d3(int s) { return double3{(double)s, (double)s, (double)s}; }
TW_HD int3 i3(int3 v) { return v; }
TW_HD int3 i3(int s) { return int3{s, s, s}; }
TW_HD int wadd(int a, int b) { return (int)((unsigned)a + (unsigned)b); }
TW_HD int wsub(int a, int b) { return (int)((unsigned)a - (unsigned)b); }
TW_HD int wmul(int a, int b) { return (int)((unsigned)a * (unsigned)b); }
}  // namespace detail
}  // namespace tw

#define TW_DOUBLE_OP(op, A, B)                                                         \
    TW_HD double3 operator op(A a, B b) {                                              \
        const double3 x = tw::detail::d3(a), y = tw::detail::d3(b);                    \
        return double3{x.x op y.x, x.y op y.y, x.z op y.z};                            \
    }
#define TW_DOUBLE_OPS(op)                                                              \
    TW_DOUBLE_OP(op, double3, double3) TW_DOUBLE_OP(op, double3, int3)                 \
    TW_DOUBLE_OP(op, int3, double3) TW_DOUBLE_OP(op, double3, double)                  \
    TW_DOUBLE_OP(op, double, double3) TW_DOUBLE_OP(op, double3, int)                   \
    TW_DOUBLE_OP(op, int, double3) TW_DOUBLE_OP(op, int3, double)                      \
    TW_DOUBLE_OP(op, double, int3)
TW_DOUBLE_OPS(+)
TW_DOUBLE_OPS(-)
TW_DOUBLE_OPS(*)
TW_DOUBLE_OPS(/)

#define TW_INT_OP(op, f, A, B)                                                         \
    TW_HD int3 operator op(A a, B b) {                                                 \
        const int3 x = tw::detail::i3(a), y = tw::detail::i3(b);                       \
        return int3{tw::detail::f(x.x, y.x), tw::detail::f(x.y, y.y), tw::detail::f(x.z, y.z)}; \
    }
#define TW_INT_OPS(op, f)                                                              \
    TW_INT_OP(op, f, int3, int3) TW_INT_OP(op, f, int3, int) TW_INT_OP(op, f, int, int3)
TW_INT_OPS(+, wadd)
TW_INT_OPS(-, wsub)
TW_INT_OPS(*, wmul)
TW_HD int3 operator/(int3 a, int3 b) { return int3{a.x / b.x, a.y / b.y, a.z / b.z}; }
TW_HD int3 operator/(int3 a, int b) { return int3{a.x / b, a.y / b, a.z / b}; }
TW_HD int3 operator/(int a, int3 b) { return int3{a / b.x, a / b.y, a / b.z}; }

TW_HD double3 operator-(double3 a) { return double3{-a.x, -a.y, -a.z}; }
TW_HD int3 operator-(int3 a) {
    return int3{tw::detail::wsub(0, a.x), tw::detail::wsub(0, a.y), tw::detail::wsub(0, a.z)};
}

TW_HD bool operator==(double3 a, double3 b) { return a.x == b.x && a.y == b.y && a.z == b.z; }
TW_HD bool operator!=(double3 a, double3 b) { return !(a == b); }
TW_HD bool operator==(int3 a, int3 b) { return a.x == b.x && a.y == b.y && a.z == b.z; }
TW_HD bool operator!=(int3 a, int3 b) { return !(a == b); }

namespace tw {

/// Trivially copyable array; the layout of `T[N]`.
template <class T, int N>
struct Array {
    T v[N];
    TW_HD T& operator[](int i) { return v[i]; }
    TW_HD const T& operator[](int i) const { return v[i]; }
};
}  // namespace tw

template <class T, int N>
TW_HD bool operator==(const tw::Array<T, N>& a, const tw::Array<T, N>& b) {
    for (int i = 0; i < N; i++) {
        if (!(a.v[i] == b.v[i])) return false;
    }
    return true;
}
template <class T, int N>
TW_HD bool operator!=(const tw::Array<T, N>& a, const tw::Array<T, N>& b) {
    return !(a == b);
}

namespace tw {

TW_HD double get(double3 v, int i) { return i == 0 ? v.x : i == 1 ? v.y : v.z; }
TW_HD int get(int3 v, int i) { return i == 0 ? v.x : i == 1 ? v.y : v.z; }

/// An lvalue for a temporary; lives until the end of the full expression.
template <class T>
TW_HD T& lval(T&& x) {
    return x;
}

TW_HD unsigned long long align_up(unsigned long long v, unsigned long long a) { return (v + a - 1) / a * a; }

template <class T>
TW_HDI T* buffer(char* base, unsigned long long offset) {
    return reinterpret_cast<T*>(base + offset);
}

// ---- float -> int: saturating, NaN -> 0 (Rust `as`) -----------------------------------------
TW_HD int f2i32(int x) { return x; }
TW_HD int f2i32(double x) {
    return x != x ? 0 : x >= 2147483647.0 ? 2147483647 : x <= -2147483648.0 ? (-2147483647 - 1) : (int)x;
}
TW_HD long long f2i64(double x) {
    return x != x ? 0
           : x >= 9223372036854775808.0  ? 9223372036854775807LL
           : x <= -9223372036854775808.0 ? (-9223372036854775807LL - 1)
                                         : (long long)x;
}
// The backend contract: `f2i8` clamps (Java's `(byte)` would wrap).
TW_HD signed char f2i8(double x) {
    return x != x ? (signed char)0 : x >= 127.0 ? (signed char)127 : x <= -128.0 ? (signed char)-128 : (signed char)(int)x;
}
TW_HD int f2i32(float x) { return f2i32((double)x); }
TW_HD long long f2i64(float x) { return f2i64((double)x); }
TW_HD signed char f2i8(float x) { return f2i8((double)x); }

// ---- position and configuration -------------------------------------------------------------
struct Config {
    double3 origin;
    double3 origin_scale;
    double3 position_scale;
    int3 dimensions;
};

struct PositionIterator {
    int3 pos;
    int idx;
};

/// Same `pos` as the Rust `iter_3d` item: the local coordinate, `x` fastest.
TW_HD PositionIterator make_position(const Config& c, int3 local) {
    return PositionIterator{local, local.x + c.dimensions.x * (local.y + c.dimensions.y * local.z)};
}
TW_HD int index(PositionIterator p) { return p.idx; }

// ---- bounds checks (`BufferBounds::PerBuffer`, `debug_checks`) ------------------------------
// An out-of-range index prints and traps, which the host sees as a sticky error.
TW_HDI int check_index(int i, int n, const char* where) {
    if (i < 0 || i >= n) {
        std::printf("tw: index %d out of bounds for length %d in %s\n", i, n, where);
#ifdef __CUDA_ARCH__
        __trap();
#else
        std::abort();
#endif
    }
    return i;
}

// ---- toy tables (crate README, "Rules and contracts, Toy tables") -----------------------------------------------------------------
TW_HD unsigned long long splitmix64(unsigned long long x) {
    unsigned long long z = x + 0x9E3779B97F4A7C15ULL;
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
    return z ^ (z >> 31);
}

struct PerlinNoiseSampler {
    unsigned char perm[256];
};

struct InterpolatedNoiseSampler {
    unsigned long long state;
};

/// Fisher-Yates over `0..256`, drawing `splitmix64(state + k * gamma - gamma)` for the k-th step.
TW_HDI PerlinNoiseSampler make_permutation_table(long long seed, long long id_lo, long long id_hi,
                                                 long long subident_index, long long sub_lo, long long sub_hi) {
    unsigned long long st = splitmix64((unsigned long long)seed ^ (unsigned long long)id_lo);
    st = splitmix64(st ^ (unsigned long long)id_hi);
    st = splitmix64(st ^ (unsigned long long)subident_index);
    st = splitmix64(st ^ (unsigned long long)sub_lo);
    st = splitmix64(st ^ (unsigned long long)sub_hi);
    PerlinNoiseSampler t{};
    for (int i = 0; i < 256; i++) t.perm[i] = (unsigned char)i;
    unsigned long long s = st;
    for (int i = 255; i >= 1; i--) {
        s += 0x9E3779B97F4A7C15ULL;
        const unsigned long long r = splitmix64(s - 0x9E3779B97F4A7C15ULL);
        const int j = (int)(r % (unsigned long long)(i + 1));
        const unsigned char tmp = t.perm[i];
        t.perm[i] = t.perm[j];
        t.perm[j] = tmp;
    }
    return t;
}

TW_HDI InterpolatedNoiseSampler make_base3d_perm_table(long long seed) {
    return InterpolatedNoiseSampler{splitmix64((unsigned long long)seed)};
}

/// FNV-1a over the bytes of the NUL-terminated UTF-8 string, seeded with the i64.
TW_HDI long long make_salt(long long seed, const char* name) {
    unsigned long long h = (unsigned long long)seed ^ 0xcbf29ce484222325ULL;
    for (const unsigned char* p = (const unsigned char*)name; *p; ++p) {
        h ^= (unsigned long long)*p;
        h *= 0x100000001b3ULL;
    }
    return (long long)h;
}

TW_HDI long long make_bits(double x) {
    long long out;
    __builtin_memcpy(&out, &x, sizeof(out));
    return out;
}

TW_HDI double make_scale(double base, bool flip) { return flip ? -base : base; }
TW_HDI bool make_flag(bool on, long long n) { return on && n > 0; }
TW_HDI signed char make_i8(long long v) { return (signed char)v; }
TW_HDI int make_i32(long long v) { return (int)v; }
TW_HDI float make_f32(double v) { return (float)v; }

}  // namespace tw

#include "records.cuh"

/// Evaluates each argument once and returns the index.
#define TW_CHECK_INDEX(i, n, where) ::tw::check_index((i), (n), (where))
