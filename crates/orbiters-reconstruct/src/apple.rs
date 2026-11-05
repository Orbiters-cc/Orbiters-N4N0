//! What Apple's systems say about memory. A Mac's GPU has no memory of its own: its buffers, the keyframes and every
//! other program share the same RAM, and what one program takes comes off what the others have.

/// This computer's memory, bytes.
pub fn physical_memory() -> Option<u64> {
    let mut bytes = 0u64;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: a NUL-terminated name, an out-parameter and its size; nothing is written to the system.
    let failed = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&raw mut bytes).cast(), &mut size, std::ptr::null_mut(), 0) };
    (failed == 0 && bytes > 0).then_some(bytes)
}

/// Memory the system gives up without touching anything in use now, bytes: free, inactive and purgeable pages. It does
/// not count what compressing other programs' memory would free as well, so a run takes less than it could rather than
/// pushing them out. It leaves the compressor's own pages in, as Activity Monitor does: sysinfo's `available_memory`
/// subtracts them and said 0 with 4 GB of inactive pages on a 16 GB Mac that was compiling.
pub fn available_memory() -> Option<u64> {
    let mut stats: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: the out-parameter and its size in words are what HOST_VM_INFO64 asks for.
    #[allow(deprecated)]
    let result = unsafe { libc::host_statistics64(libc::mach_host_self(), libc::HOST_VM_INFO64, (&raw mut stats).cast(), &mut count) };
    if result != libc::KERN_SUCCESS {
        return None;
    }
    // SAFETY: a read of a constant of the system.
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok()?;
    Some((u64::from(stats.free_count) + u64::from(stats.inactive_count) + u64::from(stats.purgeable_count)) * page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_says_how_much_memory_it_has() {
        let total = physical_memory().expect("hw.memsize");
        let available = available_memory().expect("host_statistics64");
        assert!(total >= 4 << 30 && total < 1 << 44, "{total}");
        assert!(available < total, "{available} of {total}");
    }
}
