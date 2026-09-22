// what android 4.0's libc (api 15) doesnt have yet but the app ends up calling anyway.
// the ndk only builds for 21 and up, so it links as if these were there. defined here, our own references
// bind to these instead (the android linker looks in the library itself first), and newer androids never ask libc either.
// make apk-legacy checks against packaging/android-15-symbols.txt that nothing else is missing
//
// who wants them: epoll_create1 tokio, getauxval aws-lc's cpu detection, posix_memalign some c in there,
// log2 egui, dl_iterate_phdr the unwinder that runs when something panics

use std::ffi::{c_int, c_ulong, c_void};
use std::sync::OnceLock;

/// api 21. the syscall is way older than that (linux 2.6.27)
#[unsafe(no_mangle)]
pub extern "C" fn epoll_create1(flags: c_int) -> c_int {
    unsafe { libc::syscall(libc::SYS_epoll_create1, flags) as c_int }
}

/// api 18. the kernel hands every process the same list in /proc/self/auxv, (type, value) word pairs
#[unsafe(no_mangle)]
pub extern "C" fn getauxval(kind: c_ulong) -> c_ulong {
    static AUXV: OnceLock<Vec<(c_ulong, c_ulong)>> = OnceLock::new();
    let auxv = AUXV.get_or_init(|| {
        let word = size_of::<c_ulong>();
        let bytes = std::fs::read("/proc/self/auxv").unwrap_or_default();
        let at = |c: &[u8]| c_ulong::from_ne_bytes(c.try_into().unwrap());
        bytes.chunks_exact(2 * word).map(|c| (at(&c[..word]), at(&c[word..]))).collect()
    });
    auxv.iter().find(|(k, _)| *k == kind).map_or(0, |(_, v)| *v)
}

/// api 16. memalign was there all along, and bionic's free takes what it gives out
#[unsafe(no_mangle)]
pub unsafe extern "C" fn posix_memalign(out: *mut *mut c_void, align: usize, size: usize) -> c_int {
    if !align.is_power_of_two() || !align.is_multiple_of(size_of::<*mut c_void>()) {
        return libc::EINVAL;
    }
    let p = unsafe { libc::memalign(align, size) };
    if p.is_null() && size != 0 {
        return libc::ENOMEM;
    }
    unsafe { *out = p };
    0
}

/// api 18. split off the exponent first so powers of two come out exact, callers round on those.
/// log (ln) is there, and doesnt get turned back into a log2 call
#[unsafe(no_mangle)]
pub extern "C" fn log2(x: f64) -> f64 {
    if !(x.is_normal() && x > 0.0) {
        // 0, negative, inf, nan and the tiny ones
        return x.ln() / std::f64::consts::LN_2;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i64 - 1023;
    // same mantissa with the exponent of 1.0, so in [1, 2)
    let mantissa = f64::from_bits((bits & !(0x7ff << 52)) | (1023 << 52));
    exponent as f64 + mantissa.ln() / std::f64::consts::LN_2
}

#[unsafe(no_mangle)]
pub extern "C" fn log2f(x: f32) -> f32 {
    log2(x as f64) as f32
}

/// api 21 on arm. the unwinder asks it where the code it walks through was loaded, a panic is the only time
/// it walks, and only through our own code. so this reports just this library: the elf header sits at the
/// start of the first segment, the program headers are where it says
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dl_iterate_phdr(
    callback: Option<unsafe extern "C" fn(*mut libc::dl_phdr_info, usize, *mut c_void) -> c_int>,
    data: *mut c_void,
) -> c_int {
    let Some(callback) = callback else { return 0 };
    let mut found: libc::Dl_info = unsafe { std::mem::zeroed() };
    if unsafe { libc::dladdr(dl_iterate_phdr as *const c_void, &mut found) } == 0 || found.dli_fbase.is_null() {
        return 0;
    }
    let base = found.dli_fbase as *const u8;
    // e_phoff and e_phnum, where the elf spec puts them (libc has no Elf32_Ehdr for android)
    #[cfg(target_pointer_width = "32")]
    let (phoff, phnum) = unsafe { ((base.add(28) as *const u32).read_unaligned() as usize, (base.add(44) as *const u16).read()) };
    #[cfg(target_pointer_width = "64")]
    let (phoff, phnum) = unsafe { ((base.add(32) as *const u64).read_unaligned() as usize, (base.add(56) as *const u16).read()) };

    let mut info: libc::dl_phdr_info = unsafe { std::mem::zeroed() };
    // the first segment starts at address 0, so where it got loaded is the offset for all of them
    info.dlpi_addr = base as _;
    info.dlpi_name = found.dli_fname;
    info.dlpi_phdr = unsafe { base.add(phoff) } as _;
    info.dlpi_phnum = phnum;
    unsafe { callback(&mut info, size_of::<libc::dl_phdr_info>(), data) }
}
