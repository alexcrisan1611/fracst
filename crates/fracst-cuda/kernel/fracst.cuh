// fracst pixel loop: perturbation with BLA skipping and rebasing.
//
// A port of fracst-core's `perturb.rs` (and the parts of `num.rs`,
// `floatexp.rs` and `bla.rs` it uses). Every floating-point operation is the
// one the CPU loop performs, in the same order, so frames come out
// bit-identical. That needs contraction off (NVRTC `--fmad=false`, host
// `-ffp-contract=off`): products and sums are only fused where the CPU loop
// fuses them, with an explicit fma().
//
// The file compiles both as CUDA (NVRTC) and as host C++. The host build
// exists so the port can be tested against the CPU loop on any machine.

#if defined(__CUDACC__) || defined(__CUDACC_RTC__)
#define FR_DEVICE 1
#define FR_FN __device__ __forceinline__
#define FR_STATIC __device__ __forceinline__ static
#else
#include <cmath>
#include <cstring>
#define FR_FN static inline
#define FR_STATIC static inline
#endif

typedef unsigned long long u64;
typedef long long i64;
typedef unsigned int u32;

// ---------------------------------------------------------------------------
// f64 helpers

FR_FN double bits_f64(u64 b) {
#ifdef FR_DEVICE
    return __longlong_as_double((long long)b);
#else
    double x;
    memcpy(&x, &b, 8);
    return x;
#endif
}

FR_FN u64 f64_bits(double x) {
#ifdef FR_DEVICE
    return (u64)__double_as_longlong(x);
#else
    u64 b;
    memcpy(&b, &x, 8);
    return b;
#endif
}

FR_FN double fr_fma(double a, double b, double c) {
#ifdef FR_DEVICE
    return __fma_rn(a, b, c);
#else
    return std::fma(a, b, c);
#endif
}

FR_FN double fr_abs(double x) { return bits_f64(f64_bits(x) & 0x7fffffffffffffffULL); }

FR_FN bool fr_is_nan(double x) { return x != x; }

// Rust's f64::max: the other operand when one is NaN.
FR_FN double fr_max(double a, double b) {
    if (fr_is_nan(a)) return b;
    if (fr_is_nan(b)) return a;
    return a > b ? a : b;
}

FR_FN bool fr_is_finite(double x) { return ((f64_bits(x) >> 52) & 0x7ff) != 0x7ff; }

FR_FN u32 biased_exponent(double x) { return (u32)((f64_bits(x) >> 52) & 0x7ff); }

// 2^k for -1022 <= k <= 1023, built from the bits as in Rust.
FR_FN double pow2(int k) { return bits_f64((u64)(i64)(k + 1023) << 52); }

FR_FN double fr_infinity() { return bits_f64(0x7ff0000000000000ULL); }

FR_FN u64 trailing_zeros(u64 x) {
#ifdef FR_DEVICE
    return x == 0 ? 64 : (u64)(__ffsll((long long)x) - 1);
#else
    if (x == 0) return 64;
    u64 n = 0;
    while (!(x & 1)) {
        x >>= 1;
        n++;
    }
    return n;
#endif
}

FR_FN u64 min_u64(u64 a, u64 b) { return a < b ? a : b; }

// ---------------------------------------------------------------------------
// FloatExp: m * 2^e with m == 0 or 1 <= |m| < 2

struct FExp {
    double m;
    int e;
    int pad;
};

#define FE_ZERO_EXP (-536870912) // i32::MIN / 4

FR_FN FExp fe_make(double m, int e) {
    FExp r;
    r.m = m;
    r.e = e;
    r.pad = 0;
    return r;
}

FR_FN FExp fe_zero() { return fe_make(0.0, FE_ZERO_EXP); }
FR_FN FExp fe_one() { return fe_make(1.0, 0); }

FR_FN FExp fe_new(double m, int e) {
    u64 bits = f64_bits(m);
    int biased = (int)((bits >> 52) & 0x7ff);
    if (biased == 0) {
        if (m == 0.0) return fe_zero();
        // Subnormal: scaled by 2^64 it is normal.
        m = m * 18446744073709551616.0;
        e = e - 64;
        bits = f64_bits(m);
        biased = (int)((bits >> 52) & 0x7ff);
    }
    return fe_make(bits_f64((bits & 0x800fffffffffffffULL) | 0x3ff0000000000000ULL), e + (biased - 1023));
}

FR_FN FExp fe_from_f64(double x) { return fe_new(x, 0); }

FR_FN double fe_to_f64(FExp x) {
    if (x.e >= -1022) {
        if (x.e > 1023) return x.m * fr_infinity();
        return x.m * pow2(x.e);
    } else if (x.e < -1100) {
        return 0.0;
    } else {
        return x.m * pow2(x.e + 128) * pow2(-128);
    }
}

FR_FN bool fe_is_zero(FExp x) { return x.m == 0.0; }
FR_FN FExp fe_abs(FExp x) { return fe_make(fr_abs(x.m), x.e); }
FR_FN FExp fe_neg(FExp x) { return fe_make(-x.m, x.e); }
FR_FN FExp fe_ldexp(FExp x, int k) { return x.m == 0.0 ? x : fe_make(x.m, x.e + k); }

FR_FN FExp fe_add(FExp a, FExp o) {
    i64 d = (i64)a.e - (i64)o.e;
    if (d > 64) return a;
    if (d < -64) return o;
    if (d >= 0) return fe_new(a.m + o.m * pow2((int)-d), a.e);
    return fe_new(a.m * pow2((int)d) + o.m, o.e);
}

FR_FN FExp fe_mul(FExp a, FExp o) { return fe_new(a.m * o.m, a.e + o.e); }

// Rust's partial_cmp on f64: -1, 0, 1, or 2 when unordered.
FR_FN int f64_cmp(double a, double b) {
    if (a < b) return -1;
    if (a > b) return 1;
    if (a == b) return 0;
    return 2;
}

FR_FN int fe_cmp(FExp a, FExp o) {
    if (a.m == 0.0 || o.m == 0.0 || (a.m > 0.0) != (o.m > 0.0)) return f64_cmp(a.m, o.m);
    int mags = f64_cmp(fr_abs(a.m), fr_abs(o.m));
    if (mags == 2) return 2;
    int ord = a.e < o.e ? -1 : (a.e > o.e ? 1 : mags);
    return a.m > 0.0 ? ord : -ord;
}

FR_FN bool fe_lt(FExp a, FExp o) {
    if (a.m >= 0.0 && o.m > 0.0) return a.e < o.e || (a.e == o.e && a.m < o.m);
    return fe_cmp(a, o) == -1;
}

FR_FN bool fe_gt(FExp a, FExp o) { return fe_lt(o, a); }

// Real::max: `if o > self { o } else { self }`.
FR_FN FExp fe_max(FExp a, FExp o) { return fe_gt(o, a) ? o : a; }

// Real operations for templates: f64 or FExp.
FR_FN bool r_lt(double a, double b) { return a < b; }
FR_FN bool r_gt(double a, double b) { return a > b; }
FR_FN bool r_lt(FExp a, FExp b) { return fe_lt(a, b); }
FR_FN bool r_gt(FExp a, FExp b) { return fe_gt(a, b); }
FR_FN double r_to_f64(double x) { return x; }
FR_FN double r_to_f64(FExp x) { return fe_to_f64(x); }

// ---------------------------------------------------------------------------
// Complex types: C64 (plain f64) and CX (extended range, shared exponent)

struct C64 {
    double re, im;
};

// (re + i im) * 2^e, renormalised lazily when the larger mantissa leaves 2^+-480.
struct CX {
    double re, im;
    int e;
    int pad;
};

// A complex FloatExp pair, for conversions.
struct CFE {
    FExp re, im;
};

#define CX_LO (1023 - 480)
#define CX_HI (1023 + 480)
#define CX_ZERO_E (-536870912)

FR_FN C64 c64(double re, double im) {
    C64 r;
    r.re = re;
    r.im = im;
    return r;
}

FR_FN CX cx_make(double re, double im, int e) {
    CX r;
    r.re = re;
    r.im = im;
    r.e = e;
    r.pad = 0;
    return r;
}

FR_FN CX cx_zero() { return cx_make(0.0, 0.0, CX_ZERO_E); }

FR_FN CX cx_renormalise(double re, double im, int e) {
    double m = fr_max(fr_abs(re), fr_abs(im));
    if (m == 0.0 || !fr_is_finite(m)) return m == 0.0 ? cx_zero() : cx_make(re, im, e);
    if (m < 2.2250738585072014e-308) {
        re = re * pow2(64);
        im = im * pow2(64);
        e = e - 64;
    }
    int k = (int)biased_exponent(fr_max(fr_abs(re), fr_abs(im))) - 1023;
    double s = pow2(-k);
    return cx_make(re * s, im * s, e + k);
}

FR_FN CX cx_new(double re, double im, int e) {
    u32 biased = biased_exponent(fr_max(fr_abs(re), fr_abs(im)));
    if (biased >= CX_LO && biased <= CX_HI) return cx_make(re, im, e);
    return cx_renormalise(re, im, e);
}

// Operations, overloaded on the complex type.

FR_FN C64 c_add(C64 a, C64 o) { return c64(a.re + o.re, a.im + o.im); }
FR_FN C64 c_mul(C64 a, C64 o) { return c64(a.re * o.re - a.im * o.im, a.re * o.im + a.im * o.re); }
FR_FN C64 c_mul2(C64 a) { return c64(a.re * 2.0, a.im * 2.0); }
FR_FN C64 c_scale(C64 a, double k) { return c64(a.re * k, a.im * k); }
FR_FN double c_norm(C64 a) { return a.re * a.re + a.im * a.im; }
FR_FN double c_fused_norm(C64 a) { return fr_fma(a.re, a.re, a.im * a.im); }
FR_FN C64 c_fused_mul_add(C64 a, C64 b, C64 c) {
    return c64(fr_fma(a.re, b.re, fr_fma(-a.im, b.im, c.re)), fr_fma(a.re, b.im, fr_fma(a.im, b.re, c.im)));
}
FR_FN C64 c_to_c64(C64 a) { return a; }
FR_FN CFE c_to_cfe(C64 a) {
    CFE r;
    r.re = fe_from_f64(a.re);
    r.im = fe_from_f64(a.im);
    return r;
}
FR_FN bool c_is_finite(C64 a) { return fr_is_finite(a.re) && fr_is_finite(a.im); }

FR_FN CX c_add(CX a, CX o) {
    // Mantissas lie within 2^+-480, so beyond 1021 bits apart the smaller
    // operand is below 2^-60 of the larger.
    int d = a.e - o.e;
    if (d >= 0) {
        if (d > 1021) return a;
        double s = pow2(-d);
        return cx_new(a.re + o.re * s, a.im + o.im * s, a.e);
    }
    if (d < -1021) return o;
    double s = pow2(d);
    return cx_new(a.re * s + o.re, a.im * s + o.im, o.e);
}
FR_FN CX c_mul(CX a, CX o) { return cx_new(a.re * o.re - a.im * o.im, a.re * o.im + a.im * o.re, a.e + o.e); }
FR_FN CX c_mul2(CX a) { return cx_make(a.re, a.im, a.e + 1); }
FR_FN CX c_scale(CX a, FExp k) { return cx_new(a.re * k.m, a.im * k.m, a.e + k.e); }
FR_FN FExp c_norm(CX a) { return fe_new(a.re * a.re + a.im * a.im, 2 * a.e); }
// Extended range never fuses (the trait defaults).
FR_FN FExp c_fused_norm(CX a) { return c_norm(a); }
FR_FN CX c_fused_mul_add(CX a, CX b, CX c) { return c_add(c_mul(a, b), c); }
FR_FN C64 c_to_c64(CX a) { return c64(fe_to_f64(fe_new(a.re, a.e)), fe_to_f64(fe_new(a.im, a.e))); }
FR_FN CFE c_to_cfe(CX a) {
    CFE r;
    r.re = fe_new(a.re, a.e);
    r.im = fe_new(a.im, a.e);
    return r;
}
FR_FN bool c_is_finite(CX a) { return fr_is_finite(a.re) && fr_is_finite(a.im); }

// Per-type constants and constructors.
template <class C> struct CT;

template <> struct CT<C64> {
    typedef double R;
    FR_STATIC C64 zero() { return c64(0.0, 0.0); }
    FR_STATIC C64 one() { return c64(1.0, 0.0); }
    FR_STATIC C64 from_real(double x) { return c64(x, 0.0); }
    FR_STATIC double r_from_f64(double x) { return x; }
    FR_STATIC C64 from_cfe(CFE c) { return c64(fe_to_f64(c.re), fe_to_f64(c.im)); }
};

template <> struct CT<CX> {
    typedef FExp R;
    FR_STATIC CX zero() { return cx_zero(); }
    FR_STATIC CX one() { return cx_make(1.0, 0.0, 0); }
    FR_STATIC CX from_real(FExp x) { return x.m == 0.0 ? cx_zero() : cx_make(x.m, 0.0, x.e); }
    FR_STATIC FExp r_from_f64(double x) { return fe_from_f64(x); }
    FR_STATIC double part(double m, int k, int e) { return (m == 0.0 || k - e < -1000) ? 0.0 : m * pow2(k - e); }
    FR_STATIC CX from_cfe(CFE c) {
        double mr = c.re.m, mi = c.im.m;
        int er = c.re.e, ei = c.im.e;
        int e;
        if (mr == 0.0 && mi == 0.0) return cx_zero();
        if (mi == 0.0) {
            e = er;
        } else if (mr == 0.0) {
            e = ei;
        } else {
            e = er > ei ? er : ei;
        }
        return cx_new(part(mr, er, e), part(mi, ei, e), e);
    }
};

// ---------------------------------------------------------------------------
// Data shared with the host (layouts mirror crates/fracst-cuda/src/layout.rs)

// A BLA step for the f64 phase.
struct BlaF {
    double ar, ai, br, bi, r2;
    u32 l, pad;
};

// A BLA step for the extended-range phase.
struct BlaX {
    double ar, ai, br, bi, r2m;
    int ae, be, r2e;
    u32 l;
};

// An orbit value prepared for the extended-range phase.
struct DeepZ {
    double re, im, norm;
    int e, pad;
};

struct PixelOut {
    double zr, zi, dr, di;
    u64 n, steps, deep_steps;
    u32 escaped, pad;
};

struct Params {
    // Pointers (device pointers on the GPU, host pointers in the host build).
    u64 orbit;      // C64[mref + 1]
    u64 deep_orbit; // DeepZ[mref + 1], deep passes only
    u64 bla64;      // BlaF, all levels one after another
    u64 levels64;   // (start, len) per level
    u64 bladeep;    // BlaX
    u64 levelsdeep;
    u64 dcs;      // C64 per pixel
    u64 deep_dcs; // CX per pixel, deep passes only
    u64 out;      // PixelOut per pixel
    u64 counter;  // u64: next pixel to claim
    u64 cancel;   // int: nonzero to stop claiming pixels
    u64 count;
    u64 mref;
    u64 maxiter;
    u64 interior_start;
    u64 nlevels64;
    u64 nlevelsdeep;
    u64 periodic;
    double max_r2_64;
    double spacing64;
    double bailout2_64;
    double switch2_64;
    double interior_radius2;
    FExp max_r2_deep;
    FExp spacing_deep;
    FExp bailout2_deep;
    FExp switch2_deep;
};

// ---------------------------------------------------------------------------
// BLA tables

template <class C> struct BlaV {
    C a, b;
    typename CT<C>::R r2;
    u32 l;
};

FR_FN BlaV<C64> bla_entry(const Params& p, C64*, u64 k, u64 j) {
    const u64* levels = (const u64*)p.levels64;
    const BlaF& f = ((const BlaF*)p.bla64)[levels[2 * k] + j];
    BlaV<C64> v;
    v.a = c64(f.ar, f.ai);
    v.b = c64(f.br, f.bi);
    v.r2 = f.r2;
    v.l = f.l;
    return v;
}

FR_FN BlaV<CX> bla_entry(const Params& p, CX*, u64 k, u64 j) {
    const u64* levels = (const u64*)p.levelsdeep;
    const BlaX& f = ((const BlaX*)p.bladeep)[levels[2 * k] + j];
    BlaV<CX> v;
    v.a = cx_make(f.ar, f.ai, f.ae);
    v.b = cx_make(f.br, f.bi, f.be);
    v.r2 = fe_make(f.r2m, f.r2e);
    v.l = f.l;
    return v;
}

FR_FN u64 bla_levels(const Params& p, C64*) { return p.nlevels64; }
FR_FN u64 bla_levels(const Params& p, CX*) { return p.nlevelsdeep; }
FR_FN u64 bla_len(const Params& p, C64*, u64 k) { return ((const u64*)p.levels64)[2 * k + 1]; }
FR_FN u64 bla_len(const Params& p, CX*, u64 k) { return ((const u64*)p.levelsdeep)[2 * k + 1]; }
FR_FN double bla_max_r2(const Params& p, C64*) { return p.max_r2_64; }
FR_FN FExp bla_max_r2(const Params& p, CX*) { return p.max_r2_deep; }

// BlaTable::lookup: test the shortest step first, then search down from the longest.
template <class C>
FR_FN bool bla_lookup(const Params& p, u64 m, typename CT<C>::R dzn, u64 max_steps, BlaV<C>& out) {
    C* tag = 0;
    if (m == 0 || !r_lt(dzn, bla_max_r2(p, tag))) return false;
    u64 i = m - 1;
    u64 usable = min_u64(trailing_zeros(i), bla_levels(p, tag));
    if (usable == 0) return false;
    if ((i >> 1) >= bla_len(p, tag, 0)) return false;
    BlaV<C> shortest = bla_entry(p, tag, 0, i >> 1);
    if (!r_lt(dzn, shortest.r2) || (u64)shortest.l > max_steps) return false;
    for (u64 k = usable - 1; k >= 1; --k) {
        u64 j = i >> (k + 1);
        if (j < bla_len(p, tag, k)) {
            BlaV<C> b = bla_entry(p, tag, k, j);
            if (r_lt(dzn, b.r2) && (u64)b.l <= max_steps) {
                out = b;
                return true;
            }
        }
    }
    out = shortest;
    return true;
}

// BlaTable::lookup_longest: the same step, searching from the longest down.
template <class C>
FR_FN bool bla_lookup_longest(const Params& p, u64 m, typename CT<C>::R dzn, u64 max_steps, BlaV<C>& out) {
    C* tag = 0;
    if (m == 0 || !r_lt(dzn, bla_max_r2(p, tag))) return false;
    u64 i = m - 1;
    u64 usable = min_u64(trailing_zeros(i), bla_levels(p, tag));
    for (u64 k = usable; k-- > 0;) {
        u64 j = i >> (k + 1);
        if (j < bla_len(p, tag, k)) {
            BlaV<C> b = bla_entry(p, tag, k, j);
            if (r_lt(dzn, b.r2) && (u64)b.l <= max_steps) {
                out = b;
                return true;
            }
        }
    }
    return false;
}

// ---------------------------------------------------------------------------
// Orbit access

FR_FN C64 z_at(const Params& p, C64*, u64 m) {
    const double* o = (const double*)p.orbit;
    return c64(o[2 * m], o[2 * m + 1]);
}

FR_FN CX z_at(const Params& p, CX*, u64 m) {
    const DeepZ& d = ((const DeepZ*)p.deep_orbit)[m];
    return cx_make(d.re, d.im, d.e);
}

FR_FN double zn_at(const Params& p, u64 m) { return ((const DeepZ*)p.deep_orbit)[m].norm; }

// ---------------------------------------------------------------------------
// The pixel loop (perturb::run)

// |dz/dz_1|^2 rescaling in f64, which can leave the range of doubles.
#define DW_STEP 300

// |Z_m|^2 above which an orbit value dwarfs any extended-range dz (|dz| <= 2^-480).
FR_FN double ordinary_norm() { return bits_f64((u64)(1023 - 900) << 52); }

template <class C> struct Phase {
    C dc;
    typename CT<C>::R spacing, bailout2, switch2;
};

template <class C> struct State {
    C dz, der, dw;
    int dw_exp;
    bool dw_active;
    u32 dw_below;
    u64 dw_start, m, n, steps;
};

enum { EXIT_ESCAPED = 0, EXIT_STOPPED = 1, EXIT_SWITCH = 2 };

struct Exit {
    int kind;
    C64 z;
};

FR_FN Exit exit_with(int kind) {
    Exit x;
    x.kind = kind;
    x.z = c64(0.0, 0.0);
    return x;
}

// State::start: the state after the first iteration (z_1 = c, so dz_1 = dc).
template <class C>
FR_FN State<C> state_start(const Params& p, C dc, typename CT<C>::R spacing, bool dw_active, u64 maxiter) {
    State<C> s;
    s.dz = dc;
    s.der = CT<C>::from_real(spacing);
    s.dw = CT<C>::one();
    s.dw_exp = 0;
    s.dw_active = dw_active;
    s.dw_below = 0;
    s.dw_start = maxiter > 2 * p.interior_start ? p.interior_start : 0;
    s.m = 1;
    s.n = 1;
    s.steps = 0;
    return s;
}

// State::cast: convert between representations, moving powers of two of dw into dw_exp.
template <class D, class C> FR_FN State<D> state_cast(const State<C>& s) {
    CFE dw = c_to_cfe(s.dw);
    FExp mag = fe_max(fe_abs(dw.re), fe_abs(dw.im));
    int shift = 0;
    if (!fe_is_zero(mag)) {
        int over = mag.e - 500;
        if (over < 0) over = 0;
        shift = (over + DW_STEP - 1) / DW_STEP * DW_STEP;
    }
    dw.re = fe_ldexp(dw.re, -shift);
    dw.im = fe_ldexp(dw.im, -shift);
    State<D> r;
    r.dz = CT<D>::from_cfe(c_to_cfe(s.dz));
    r.der = CT<D>::from_cfe(c_to_cfe(s.der));
    r.dw = CT<D>::from_cfe(dw);
    r.dw_exp = s.dw_exp + shift;
    r.dw_active = s.dw_active;
    r.dw_below = s.dw_below;
    r.dw_start = s.dw_start;
    r.m = s.m;
    r.n = s.n;
    r.steps = s.steps;
    return r;
}

template <class C, bool FMA> FR_FN typename CT<C>::R norm_of(C a) { return FMA ? c_fused_norm(a) : c_norm(a); }

template <class C, bool FMA> FR_FN C mul_add(C a, C b, C c) {
    return FMA ? c_fused_mul_add(a, b, c) : c_add(c_mul(a, b), c);
}

// Iterate until escape, maxiter, interior, or (with UP/DOWN) |dz|^2 crossing
// switch2. DEEP enables the extended-range fast path (see perturb.rs).
template <class C, bool DE, bool INSIDE, bool UP, bool DOWN, bool DEEP, bool FMA>
FR_FN Exit run(const Params& p, const Phase<C>& ph, State<C>& st, u64 maxiter) {
    typedef typename CT<C>::R R;
    C* tag = 0;
    const u64 mref = p.mref;
    const bool periodic = p.periodic != 0;
    C dz = st.dz, der = st.der, dw = st.dw;
    int dw_exp = st.dw_exp;
    bool dw_active = st.dw_active;
    u32 dw_below = st.dw_below;
    const u64 dw_start = st.dw_start;
    u64 m = st.m, n = st.n, steps = st.steps;
    const R ir2 = CT<C>::r_from_f64(p.interior_radius2);
    const R dw_big = CT<C>::r_from_f64(pow2(600));
    const R dw_small = CT<C>::r_from_f64(pow2(-600));
    C zm = z_at(p, tag, m);
    // Below a quarter of the bailout, |Z_m + dz| stays inside the escape radius.
    const double zn_escape = DEEP ? r_to_f64(ph.bailout2) * 0.25 : 0.0;
    Exit exit;
    for (;;) {
        R dzn = norm_of<C, FMA>(dz);
        bool fast = false;
        if (DEEP && m != mref && !r_gt(dzn, ph.switch2)) {
            double zmn = zn_at(p, m);
            fast = zmn >= ordinary_norm() && zmn <= zn_escape;
        }
        C z = zm;
        if (!fast) {
            z = c_add(zm, dz);
            R zn = norm_of<C, FMA>(z);
            if (r_gt(zn, ph.bailout2)) {
                exit = exit_with(EXIT_ESCAPED);
                exit.z = c_to_c64(z);
                break;
            }
            if (r_lt(zn, dzn) || m == mref) {
                if (INSIDE && periodic && m == mref) {
                    // End of a period of the reference cycle.
                    if (dw_active) {
                        dw_below = (dw_exp <= 0 && r_lt(c_norm(dw), ir2)) ? dw_below + 1 : 0;
                        if (dw_below >= 2) {
                            exit = exit_with(EXIT_STOPPED);
                            break;
                        }
                    } else if (n >= dw_start) {
                        dw_active = true;
                        dw = CT<C>::one();
                        dw_exp = 0;
                    }
                }
                dz = z;
                dzn = zn;
                m = 0;
                zm = CT<C>::zero();
                z = dz;
            }
        }
        if (n >= maxiter) {
            exit = exit_with(EXIT_STOPPED);
            break;
        }
        if ((UP && r_gt(dzn, ph.switch2)) || (DOWN && r_lt(dzn, ph.switch2))) {
            exit = exit_with(EXIT_SWITCH);
            break;
        }
        if (DEEP && INSIDE && dw_active) {
            if (!periodic && r_lt(c_norm(dw), ir2)) {
                exit = exit_with(EXIT_STOPPED);
                break;
            }
        } else if (INSIDE && dw_active) {
            R dwn = c_norm(dw);
            if (!periodic && dw_exp == 0 && r_lt(dwn, ir2)) {
                exit = exit_with(EXIT_STOPPED);
                break;
            }
            if (r_gt(dwn, dw_big)) {
                dw = c_scale(dw, CT<C>::r_from_f64(pow2(-300)));
                dw_exp += DW_STEP;
            } else if (dw_exp > 0 && r_lt(dwn, dw_small)) {
                dw = c_scale(dw, CT<C>::r_from_f64(pow2(300)));
                dw_exp -= DW_STEP;
            }
        }
        steps += 1;
        BlaV<C> b;
        bool found = DEEP ? bla_lookup_longest<C>(p, m, dzn, maxiter - n, b) : bla_lookup<C>(p, m, dzn, maxiter - n, b);
        if (found) {
            dz = mul_add<C, FMA>(b.a, dz, c_mul(b.b, ph.dc));
            if (DE) der = mul_add<C, FMA>(b.a, der, c_scale(b.b, ph.spacing));
            if (INSIDE && dw_active) dw = c_mul(b.a, dw);
            m += b.l;
            n += b.l;
        } else {
            if (DE || (INSIDE && dw_active)) {
                C z2 = c_mul2(fast ? c_add(zm, dz) : z);
                if (DE) der = mul_add<C, FMA>(z2, der, CT<C>::from_real(ph.spacing));
                if (INSIDE && dw_active) dw = c_mul(z2, dw);
            }
            dz = mul_add<C, FMA>(c_add(c_mul2(zm), dz), dz, ph.dc);
            m += 1;
            n += 1;
        }
        zm = z_at(p, tag, m);
    }
    st.dz = dz;
    st.der = der;
    st.dw = dw;
    st.dw_exp = dw_exp;
    st.dw_active = dw_active;
    st.dw_below = dw_below;
    st.m = m;
    st.n = n;
    st.steps = steps;
    return exit;
}

template <class C> FR_FN PixelOut finish(const State<C>& st, Exit exit, u64 deep_steps) {
    PixelOut o;
    bool escaped = exit.kind == EXIT_ESCAPED;
    o.zr = escaped ? exit.z.re : 0.0;
    o.zi = escaped ? exit.z.im : 0.0;
    C64 der = c_to_c64(st.der);
    o.dr = der.re;
    o.di = der.im;
    o.n = st.n;
    o.steps = st.steps;
    o.deep_steps = deep_steps;
    o.escaped = escaped ? 1 : 0;
    o.pad = 0;
    return o;
}

FR_FN Phase<C64> phase64(const Params& p, C64 dc) {
    Phase<C64> ph;
    ph.dc = dc;
    ph.spacing = p.spacing64;
    ph.bailout2 = p.bailout2_64;
    ph.switch2 = p.switch2_64;
    return ph;
}

FR_FN Phase<CX> phase_deep(const Params& p, CX dc) {
    Phase<CX> ph;
    ph.dc = dc;
    ph.spacing = p.spacing_deep;
    ph.bailout2 = p.bailout2_deep;
    ph.switch2 = p.switch2_deep;
    return ph;
}

// perturb::iterate_f64 (FMA path): one pixel entirely in f64.
template <bool DE, bool INSIDE> FR_FN PixelOut iterate_f64(const Params& p, C64 dc) {
    const Phase<C64> ph = phase64(p, dc);
    const bool periodic = p.periodic != 0;
    const u64 maxiter = p.maxiter;
    u64 steps = 0;
    if (INSIDE && !periodic && maxiter > 2 * p.interior_start) {
        State<C64> st = state_start<C64>(p, dc, ph.spacing, false, maxiter);
        Exit exit = run<C64, DE, false, false, false, false, true>(p, ph, st, p.interior_start);
        if (exit.kind != EXIT_STOPPED) return finish(st, exit, 0);
        steps = st.steps;
    }
    State<C64> st = state_start<C64>(p, dc, ph.spacing, !periodic, maxiter);
    st.steps = steps;
    Exit exit = run<C64, DE, INSIDE, false, false, false, true>(p, ph, st, maxiter);
    return finish(st, exit, 0);
}

// A deep pixel's state in whichever representation it currently uses.
struct Hybrid {
    bool deep;
    State<CX> d;
    State<C64> s;
};

FR_FN u64 hybrid_steps(const Hybrid& h) { return h.deep ? h.d.steps : h.s.steps; }

FR_FN PixelOut hybrid_finish(const Hybrid& h, Exit exit, u64 deep_steps) {
    return h.deep ? finish(h.d, exit, deep_steps) : finish(h.s, exit, deep_steps);
}

// perturb::run_hybrid: alternate between extended range and f64.
template <bool DE, bool INSIDE>
FR_FN Exit run_hybrid(const Params& p, const Phase<CX>& deep, const Phase<C64>& shallow, Hybrid& h, u64 maxiter,
                      u64& deep_steps) {
    for (;;) {
        if (h.deep) {
            u64 before = h.d.steps;
            Exit exit = run<CX, DE, INSIDE, true, false, true, false>(p, deep, h.d, maxiter);
            deep_steps += h.d.steps - before;
            if (exit.kind != EXIT_SWITCH) return exit;
            h.s = state_cast<C64>(h.d);
            h.deep = false;
        } else {
            Exit exit = run<C64, DE, INSIDE, false, true, false, false>(p, shallow, h.s, maxiter);
            if (exit.kind != EXIT_SWITCH) return exit;
            State<CX> d = state_cast<CX>(h.s);
            // Extended range holds any magnitude: fold the rescaling exponent in.
            d.dw = c_scale(d.dw, fe_ldexp(fe_one(), d.dw_exp));
            d.dw_exp = 0;
            h.d = d;
            h.deep = true;
        }
    }
}

// perturb::iterate_deep: one pixel of a deep zoom.
template <bool DE, bool INSIDE> FR_FN PixelOut iterate_deep(const Params& p, CX dc_deep, C64 dc) {
    const Phase<CX> deep = phase_deep(p, dc_deep);
    const Phase<C64> shallow = phase64(p, dc);
    const bool periodic = p.periodic != 0;
    const u64 maxiter = p.maxiter;
    u64 deep_steps = 0;
    u64 steps = 0;
    Hybrid h;
    if (INSIDE && !periodic && maxiter > 2 * p.interior_start) {
        h.deep = true;
        h.d = state_start<CX>(p, dc_deep, deep.spacing, false, maxiter);
        Exit exit = run_hybrid<DE, false>(p, deep, shallow, h, p.interior_start, deep_steps);
        if (exit.kind != EXIT_STOPPED) return hybrid_finish(h, exit, deep_steps);
        steps = hybrid_steps(h);
    }
    h.deep = true;
    h.d = state_start<CX>(p, dc_deep, deep.spacing, !periodic, maxiter);
    h.d.steps = steps;
    Exit exit = run_hybrid<DE, INSIDE>(p, deep, shallow, h, maxiter, deep_steps);
    return hybrid_finish(h, exit, deep_steps);
}

// One pixel of either kind of pass.
template <bool DEEP, bool DE, bool INSIDE> FR_FN PixelOut iterate_pixel(const Params& p, u64 i) {
    const C64 dc = ((const C64*)p.dcs)[i];
    if (DEEP) return iterate_deep<DE, INSIDE>(p, ((const CX*)p.deep_dcs)[i], dc);
    return iterate_f64<DE, INSIDE>(p, dc);
}
