//! The few `kernel32` functions the control block and the driver use, declared here so that neither needs a
//! bindings crate (the driver is a small DLL inside `vrserver.exe`). Signatures from the Windows SDK headers.

#![allow(non_snake_case, clippy::upper_case_acronyms)]

use std::ffi::c_void;

pub type HANDLE = isize;
pub const INVALID_HANDLE_VALUE: HANDLE = -1;

pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE: u32 = 0x10;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;
pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_RELEASE: u32 = 0x8000;

pub const FILE_MAP_WRITE: u32 = 0x0002;
pub const FILE_MAP_READ: u32 = 0x0004;

pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_ALREADY_EXISTS: u32 = 183;

pub const SYNCHRONIZE: u32 = 0x0010_0000;
pub const WAIT_OBJECT_0: u32 = 0;

pub const GET_MODULE_HANDLE_EX_FLAG_PIN: u32 = 0x1;
pub const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;

/// `MEMORY_BASIC_INFORMATION` (64-bit).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MEMORY_BASIC_INFORMATION {
    pub BaseAddress: *mut c_void,
    pub AllocationBase: *mut c_void,
    pub AllocationProtect: u32,
    pub PartitionId: u16,
    pub RegionSize: usize,
    pub State: u32,
    pub Protect: u32,
    pub Type: u32,
}

impl MEMORY_BASIC_INFORMATION {
    pub const fn zeroed() -> Self {
        Self {
            BaseAddress: std::ptr::null_mut(),
            AllocationBase: std::ptr::null_mut(),
            AllocationProtect: 0,
            PartitionId: 0,
            RegionSize: 0,
            State: 0,
            Protect: 0,
            Type: 0,
        }
    }
}

#[link(name = "kernel32")]
extern "system" {
    pub fn CreateFileMappingW(file: HANDLE, attributes: *const c_void, protect: u32, size_high: u32, size_low: u32, name: *const u16) -> HANDLE;
    pub fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> HANDLE;
    pub fn MapViewOfFile(mapping: HANDLE, access: u32, offset_high: u32, offset_low: u32, bytes: usize) -> *mut c_void;
    pub fn UnmapViewOfFile(base: *const c_void) -> i32;
    pub fn CloseHandle(handle: HANDLE) -> i32;
    pub fn GetLastError() -> u32;
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetTickCount64() -> u64;
    pub fn VirtualQuery(address: *const c_void, buffer: *mut MEMORY_BASIC_INFORMATION, length: usize) -> usize;
    pub fn VirtualProtect(address: *const c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    pub fn VirtualAlloc(address: *const c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    pub fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
    pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> HANDLE;
    pub fn WaitForSingleObject(handle: HANDLE, milliseconds: u32) -> u32;
    pub fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut HANDLE) -> i32;
}

/// `name` as a NUL-terminated UTF-16 string.
pub fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Milliseconds since Windows started, the same clock in every process.
pub fn tick_ms() -> u64 {
    // SAFETY: no arguments, cannot fail.
    unsafe { GetTickCount64() }
}

/// This process's id.
pub fn current_pid() -> u32 {
    // SAFETY: no arguments, cannot fail.
    unsafe { GetCurrentProcessId() }
}
