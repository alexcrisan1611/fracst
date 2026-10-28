// CUDA entry points: persistent threads that claim pixels one at a time, so
// lanes whose pixel finished early pick up new work instead of idling while
// the rest of their warp iterates. Pixels are claimed in order, which keeps
// neighbouring pixels (with similar orbits) together in a warp.
//
// Compiled by NVRTC after fracst.cuh.

template <bool DEEP, bool DE, bool INSIDE> __device__ void claim_pixels(const Params& p) {
    unsigned long long* counter = (unsigned long long*)p.counter;
    volatile int* cancel = (volatile int*)p.cancel;
    PixelOut* out = (PixelOut*)p.out;
    for (;;) {
        u64 i = atomicAdd(counter, 1ULL);
        if (i >= p.count || *cancel) return;
        out[i] = iterate_pixel<DEEP, DE, INSIDE>(p, i);
    }
}

#define FRACST_KERNELS(DE, INSIDE, SUFFIX)                                                                  \
    extern "C" __global__ void __launch_bounds__(256) fracst_f64_##SUFFIX(Params p) {                       \
        claim_pixels<false, DE, INSIDE>(p);                                                                 \
    }                                                                                                       \
    extern "C" __global__ void __launch_bounds__(256) fracst_deep_##SUFFIX(Params p) {                      \
        claim_pixels<true, DE, INSIDE>(p);                                                                  \
    }

// Suffix: distance estimation, interior detection.
FRACST_KERNELS(false, false, 00)
FRACST_KERNELS(false, true, 01)
FRACST_KERNELS(true, false, 10)
FRACST_KERNELS(true, true, 11)
