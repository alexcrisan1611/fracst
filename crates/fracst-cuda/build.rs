fn main() {
    println!("cargo:rerun-if-changed=kernel");
    // The host build of the kernel, for testing without a GPU.
    {
        #[cfg(feature = "host-kernel")]
        {
            let mut build = cc::Build::new();
            build.cpp(true).std("c++17").file("kernel/host.cpp").opt_level(2);
            if !build.get_compiler().is_like_msvc() {
                // As on the GPU (--fmad=false): fuse only where the source says fma().
                build.flag("-ffp-contract=off");
            }
            build.compile("fracst_host_kernel");
        }
    }
}
