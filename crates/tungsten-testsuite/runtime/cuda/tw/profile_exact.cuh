// Registry externs for the exact profile (SPEC §5.1): bitwise equal to the Rust runtime when built
// with --fmad=false --prec-div=true --prec-sqrt=true --ftz=false. Semantics of `min`/`max` and the
// bit conversions are Java's, as in runtime/rust/externs.rs.
#pragma once

#ifndef TUNGSTEN_PROFILE_EXACT
#error "profile_exact.cuh requires TUNGSTEN_PROFILE_EXACT (density.cu defines it)"
#endif

namespace tw {

#define TW_X __host__ __device__ inline

TW_X double sqrt(double a) { return ::sqrt(a); }
TW_X double floor(double a) { return ::floor(a); }
TW_X double ceil(double a) { return ::ceil(a); }
TW_X double trunc(double a) { return ::trunc(a); }
TW_X double abs(double a) { return ::fabs(a); }
TW_X double copysign(double a, double b) { return ::copysign(a, b); }
TW_X double fma(double a, double b, double c) { return ::fma(a, b, c); }

TW_X double min(double a, double b) {
    if (a != a || b != b) return TW_NAN_F64;
    if (a == 0.0 && b == 0.0) return __builtin_signbit(a) ? a : b;
    return a < b ? a : b;
}
TW_X double max(double a, double b) {
    if (a != a || b != b) return TW_NAN_F64;
    if (a == 0.0 && b == 0.0) return __builtin_signbit(a) ? b : a;
    return a > b ? a : b;
}

TW_X float sqrt_f32(float a) { return ::sqrtf(a); }
TW_X float floor_f32(float a) { return ::floorf(a); }
TW_X float ceil_f32(float a) { return ::ceilf(a); }
TW_X float trunc_f32(float a) { return ::truncf(a); }
TW_X float abs_f32(float a) { return ::fabsf(a); }
TW_X float copysign_f32(float a, float b) { return ::copysignf(a, b); }
TW_X float fma_f32(float a, float b, float c) { return ::fmaf(a, b, c); }
TW_X float min_f32(float a, float b) { return (float)min((double)a, (double)b); }
TW_X float max_f32(float a, float b) { return (float)max((double)a, (double)b); }

TW_X int float_to_bits(float a) {
    if (a != a) return 0x7fc00000;
    int r;
    __builtin_memcpy(&r, &a, 4);
    return r;
}
TW_X long long double_to_bits(double a) {
    if (a != a) return 0x7ff8000000000000LL;
    long long r;
    __builtin_memcpy(&r, &a, 8);
    return r;
}
TW_X float bits_to_float(int a) {
    float r;
    __builtin_memcpy(&r, &a, 4);
    return r;
}
TW_X double bits_to_double(long long a) {
    double r;
    __builtin_memcpy(&r, &a, 8);
    return r;
}

TW_X int perm_get(const PerlinNoiseSampler* t, int i) { return (int)t->perm[i & 255]; }

TW_X double sampler_sample(const InterpolatedNoiseSampler* s, double3 v) {
    const unsigned long long a = (unsigned long long)(unsigned)f2i32(::floor(v.x));
    const unsigned long long b = (unsigned long long)(unsigned)f2i32(::floor(v.y));
    const unsigned long long c = (unsigned long long)(unsigned)f2i32(::floor(v.z));
    unsigned long long h = splitmix64(s->state ^ a);
    h = splitmix64(h ^ b);
    h = splitmix64(h ^ c);
    return (double)(h >> 11) * (1.0 / 9007199254740992.0);
}

/// Fails for negative input. A device trap ends the process (sticky error), so the driver cannot
/// recover the case; error cases are never run on CUDA (SPEC §6.3), and this is a safety net.
TW_X int test_domain(int x) {
#ifdef __CUDA_ARCH__
    if (x < 0) __trap();
#endif
    return x;
}

#undef TW_X

}  // namespace tw
