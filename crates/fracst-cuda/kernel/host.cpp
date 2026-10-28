// The kernel's pixel loop compiled for the host CPU, so the port can be
// checked against fracst-core's loop without a GPU (feature `host-kernel`).

#include "fracst.cuh"

template <bool DEEP> static void run_range(const Params& p, int de, int inside, u64 begin, u64 end) {
    PixelOut* out = (PixelOut*)p.out;
    for (u64 i = begin; i < end; i++) {
        if (de) {
            out[i] = inside ? iterate_pixel<DEEP, true, true>(p, i) : iterate_pixel<DEEP, true, false>(p, i);
        } else {
            out[i] = inside ? iterate_pixel<DEEP, false, true>(p, i) : iterate_pixel<DEEP, false, false>(p, i);
        }
    }
}

extern "C" void fracst_host_iterate(const Params* p, int deep, int de, int inside, u64 begin, u64 end) {
    if (deep) {
        run_range<true>(*p, de, inside, begin, end);
    } else {
        run_range<false>(*p, de, inside, begin, end);
    }
}

extern "C" u64 fracst_host_params_size() { return sizeof(Params); }
extern "C" u64 fracst_host_bla_x_size() { return sizeof(BlaX); }
extern "C" u64 fracst_host_pixel_out_size() { return sizeof(PixelOut); }
