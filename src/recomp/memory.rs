//! MIPS 32-bit address space for the recompiled port.
//!
//! The PlayStation 2's Emotion Engine addresses a flat 32-bit space: 32 MB of
//! RDRAM at `0x00000000`, a 16 KB scratchpad at `0x70000000`, and memory-mapped
//! hardware registers above `0x10000000`. God Hand's own code and data live in
//! the low region.
//!
//! A native port cannot dereference MIPS addresses directly, because the host's
//! stack and heap sit at 64-bit addresses that `LUI`+`ORI` cannot express. So
//! the lifted code addresses memory through a second base register, `r12`,
//! pointing at a host-owned buffer, and a MIPS address becomes a byte offset
//! into that buffer.
//!
//! ```text
//! rbx -> MIPS register file (136 bytes)
//! r12 -> MIPS memory base (MEMORY_SIZE bytes)
//! ```
//!
//! ## What this covers and what it does not
//!
//! Covered: RDRAM and the scratchpad, which is where God Hand's code, data,
//! and heap live. Not covered: hardware registers (DMA, GS, VIF), which need
//! side effects rather than plain loads and stores. Those are trapped by the
//! caller before they reach this module.

use std::fmt;

/// Error type for memory-space operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// The requested size is not a multiple of the platform page size.
    UnalignedSize(usize),
    /// The platform refused to allocate the buffer.
    AllocationFailed(String),
    /// An address fell outside the mapped window.
    OutOfRange { addr: u32, limit: u32 },
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::UnalignedSize(sz) => {
                write!(f, "memory size {sz} is not page-aligned")
            }
            MemoryError::AllocationFailed(msg) => write!(f, "allocation failed: {msg}"),
            MemoryError::OutOfRange { addr, limit } => {
                write!(f, "MIPS address 0x{addr:08X} is outside the 0x{limit:08X} window")
            }
        }
    }
}

impl std::error::Error for MemoryError {}

/// Result alias for memory-space operations.
pub type MemoryResult<T> = Result<T, MemoryError>;

/// Size of the mapped MIPS window.
///
/// The window must cover every address God Hand's code can form, and the
/// scratchpad sits at `0x70000000` — 1.75 GB into the address space. A 128 MB
/// window would leave it unmapped, so the window is sized to 2 GB, which makes
/// the whole low half of the 32-bit space reachable.
///
/// Addresses above `0x80000000` are hardware registers and the kernel segment;
/// those need side-effecting handlers rather than a flat buffer.
pub const MEMORY_SIZE: usize = 2 * 1024 * 1024 * 1024;

/// A host-owned buffer standing in for the PS2's 32-bit address space.
pub struct Memory {
    /// Base address of the allocation. MIPS address `a` maps to `base + a`.
    ptr: *mut u8,
    /// Size of the allocation in bytes.
    size: usize,
}

// The buffer is written and read by recompiled code, so it must be movable
// between threads.
unsafe impl Send for Memory {}

impl Memory {
    /// Allocate and zero-fill the MIPS address space.
    pub fn new() -> MemoryResult<Memory> {
        Self::with_size(MEMORY_SIZE)
    }

    /// Allocate a window of an explicit size.
    pub fn with_size(size: usize) -> MemoryResult<Memory> {
        #[cfg(windows)]
        {
            use std::ptr::null_mut;
            extern "system" {
                fn VirtualAlloc(
                    addr: *mut core::ffi::c_void,
                    size: usize,
                    alloc_type: u32,
                    protect: u32,
                ) -> *mut core::ffi::c_void;
            }
            const MEM_COMMIT: u32 = 0x1000;
            const MEM_RESERVE: u32 = 0x2000;
            const PAGE_READWRITE: u32 = 0x04;
            let ptr = unsafe {
                VirtualAlloc(null_mut(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
            };
            if ptr.is_null() {
                return Err(MemoryError::AllocationFailed(
                    "VirtualAlloc returned null".into(),
                ));
            }
            Ok(Memory {
                ptr: ptr as *mut u8,
                size,
            })
        }
        #[cfg(unix)]
        {
            extern "system" {
                fn mmap(
                    addr: *mut core::ffi::c_void,
                    len: usize,
                    prot: i32,
                    flags: i32,
                    fd: i32,
                    off: i64,
                ) -> *mut core::ffi::c_void;
            }
            const PROT_READ: i32 = 0x1;
            const PROT_WRITE: i32 = 0x2;
            const MAP_PRIVATE: i32 = 0x02;
            const MAP_ANONYMOUS: i32 = 0x20;
            let ptr = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    size,
                    PROT_READ | PROT_WRITE,
                    MAP_PRIVATE | MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if ptr as isize == -1 {
                return Err(MemoryError::AllocationFailed("mmap failed".into()));
            }
            Ok(Memory {
                ptr: ptr as *mut u8,
                size,
            })
        }
    }

    /// Base address of the buffer, for passing to recompiled code.
    pub fn base(&self) -> *mut u8 {
        self.ptr
    }

    /// Size of the mapped window.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Read a 32-bit word at a MIPS address.
    pub fn read_u32(&self, addr: u32) -> MemoryResult<u32> {
        let off = addr as usize;
        if off + 4 > self.size {
            return Err(MemoryError::OutOfRange {
                addr,
                limit: self.size as u32,
            });
        }
        Ok(unsafe { std::ptr::read_unaligned(self.ptr.add(off) as *const u32) })
    }

    /// Write a 32-bit word at a MIPS address.
    pub fn write_u32(&mut self, addr: u32, value: u32) -> MemoryResult<()> {
        let off = addr as usize;
        if off + 4 > self.size {
            return Err(MemoryError::OutOfRange {
                addr,
                limit: self.size as u32,
            });
        }
        unsafe { std::ptr::write_unaligned(self.ptr.add(off) as *mut u32, value) };
        Ok(())
    }

    /// Borrow the window as a byte slice.
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.size) }
    }

    /// Borrow the window as a mutable byte slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.size) }
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            extern "system" {
                fn VirtualFree(
                    addr: *mut core::ffi::c_void,
                    size: usize,
                    free_type: u32,
                ) -> i32;
            }
            const MEM_RELEASE: u32 = 0x8000;
            unsafe {
                VirtualFree(self.ptr as *mut core::ffi::c_void, 0, MEM_RELEASE);
            }
        }
        #[cfg(unix)]
        {
            extern "system" {
                fn munmap(addr: *mut core::ffi::c_void, len: usize) -> i32;
            }
            unsafe {
                munmap(self.ptr as *mut core::ffi::c_void, self.size);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_words() {
        let mut mem = Memory::new().expect("alloc");
        mem.write_u32(0x0010_0000, 0xDEAD_BEEF).expect("write");
        assert_eq!(mem.read_u32(0x0010_0000).expect("read"), 0xDEAD_BEEF);
    }

    #[test]
    fn scratchpad_region_is_mapped() {
        let mut mem = Memory::new().expect("alloc");
        // The scratchpad sits at 0x70000000, well inside the 128 MB window.
        mem.write_u32(0x7000_0000, 42).expect("write");
        assert_eq!(mem.read_u32(0x7000_0000).expect("read"), 42);
    }

    #[test]
    fn rejects_addresses_outside_window() {
        let mem = Memory::new().expect("alloc");
        let err = mem.read_u32(0xFFFF_FFFC).expect_err("should be out of range");
        assert!(matches!(err, MemoryError::OutOfRange { .. }));
    }

    #[test]
    fn buffer_starts_zeroed() {
        let mem = Memory::new().expect("alloc");
        assert_eq!(mem.read_u32(0).expect("read"), 0);
        assert_eq!(mem.as_slice()[MEMORY_SIZE - 1], 0);
    }

    #[test]
    fn size_is_the_documented_window() {
        let mem = Memory::new().expect("alloc");
        assert_eq!(mem.size(), 2 * 1024 * 1024 * 1024);
    }
}
